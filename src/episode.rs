//! Running a roster of actors, and recording what they saw.
//!
//! An [`episode`] runs each actor as a task of its own and waits for them all
//! to finish. It owns the wiring: [`Channels`] is built here, once the whole
//! roster is known, and lent to each actor for the duration of its run.
//!
//! # Topology
//!
//! Messages flow between every pair of actors, an actor included: an actor
//! can queue work for itself as readily as it sends to a peer, which is how a
//! perception loop defers a step rather than doing it all at once. Shutdown
//! has the same shape, so [`Channels::stop`] can stop a peer *or* ask the
//! actor's own run to end.
//!
//! # The log
//!
//! Every actor in an episode shares the one [`Sink`] that episode was given,
//! and decides for itself what is worth recording. The episode supplies the
//! rest: each [`Entry`] is tagged with the actor that made it and the time it
//! was made.
//!
//! A payload is kept as serialized structure rather than rendered text, so a
//! sink can write columns, frames, or rows and a reader is not left parsing
//! prose. [`Stderr`], [`Memory`], and [`Discard`] cover printing a run,
//! inspecting one from a test, and recording nothing.
//!
//! ```
//! use free_agent::episode::{Entry, Memory, Sink};
//!
//! let sink = Memory::new();
//! sink.write(Entry::new("Alice", &"started")?)?;
//!
//! let entries = sink.entries();
//! assert_eq!(entries[0].actor, "Alice");
//! assert_eq!(entries[0].payload, "started");
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! Logging is not best effort. A sink that fails fails the actor that was
//! writing to it, and so fails the episode: a run nobody could record is not
//! one whose result can be trusted.

use crate::actor::{Actor, ActorId, Message};
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Write, stderr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{self, JoinSet};
use tokio_util::sync::CancellationToken;

/// Runs one episode: starts a task per actor and waits for them all to
/// finish. `timeout` is a last-ditch bound — on expiry whoever is still
/// running is cut off and the episode returns.
///
/// Every actor shares `log`. What to record is the actor's decision; the
/// episode only guarantees that somewhere to record it exists, and flushes it
/// once the run is over.
///
/// The actors run on the Tokio runtime this is awaited on, and none of them
/// outlives the call: however it returns, and even if it is dropped half
/// way, any actor still running is cut off at its next `await`.
///
/// # Errors
///
/// Fails before anything runs if two actors share an id, since an id is how
/// a message or a stop signal finds its actor.
///
/// Fails as soon as any actor fails: one that returns an error, one that
/// panics, or one whose log could not be written. Its peers are not waited
/// for. Also fails if `timeout` expires with actors still running. A flush
/// that fails on the way out of either is added as context rather than
/// replacing the reason the run failed.
///
/// # Examples
///
/// ```
/// use free_agent::actor::{Acting, Actor, async_trait};
/// use free_agent::episode::{Channels, Discard, episode};
/// use std::time::Duration;
///
/// /// Does nothing but end, so the roster finishes on its own.
/// struct Fleeting;
///
/// #[async_trait]
/// impl Actor<String> for Fleeting {
///     async fn wake(&mut self, _channels: &mut Channels<String>) -> anyhow::Result<Acting> {
///         Ok(Acting::Done)
///     }
/// }
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     episode(
///         [("Alice", Box::new(Fleeting) as Box<dyn Actor<String>>)],
///         Discard,
///         Some(Duration::from_secs(5)),
///     )
///     .await
/// }
/// ```
pub async fn episode<M: Message>(
    actors: impl IntoIterator<Item = (impl Into<ActorId>, Box<dyn Actor<M>>)>,
    log: impl Sink + 'static,
    timeout: Option<Duration>,
) -> Result<()> {
    // Shared with every actor, so it must outlive them all.
    let log: Arc<dyn Sink> = Arc::new(log);
    // Ids must be unique: they are how a message or a stop signal finds its
    // actor.
    let mut roster: HashMap<ActorId, Box<dyn Actor<M>>> = HashMap::new();
    for (id, actor) in actors {
        let id = id.into();
        if roster.contains_key(&id) {
            return Err(anyhow!("duplicate actor id {id}"));
        }
        roster.insert(id, actor);
    }
    let mut channels = wire(roster.keys().cloned(), log.clone());
    let num_actors = roster.len();

    // Dropping the set aborts whatever is left in it, so no way out of this
    // function leaves an actor running.
    let mut running = JoinSet::new();
    // A task that panics says which task it was, not which actor.
    let mut names = HashMap::with_capacity(num_actors);
    for (id, mut actor) in roster {
        let mut channels = channels
            .remove(&id)
            .ok_or_else(|| anyhow!("no channels wired for {id}"))?;
        // The channels move in with the actor and drop when its run ends,
        // which is how its peers learn it is no longer receiving.
        let task = running.spawn(async move { actor.perceive(&mut channels).await });
        names.insert(task.id(), id);
    }

    let outcome = match timeout {
        Some(limit) => {
            match tokio::time::timeout(limit, all_finished(&mut running, &names)).await {
                Ok(outcome) => outcome,
                Err(_) => Err(anyhow!(
                    "episode timed out with {} of {num_actors} actors still running",
                    running.len()
                )),
            }
        }
        None => all_finished(&mut running, &names).await,
    };
    // Whoever is still running after a failure or a timeout is cut off, not
    // waited for: nothing more they produce can be trusted, and an actor that
    // would not finish is not one to wait on.
    running.abort_all();

    // A run that failed is the one whose log is worth most, so flush on the
    // way out of every ending. A failed flush is context, not the headline.
    match (outcome, log.flush()) {
        (Ok(()), flushed) => flushed,
        (Err(failure), Ok(())) => Err(failure),
        (Err(failure), Err(e)) => {
            Err(failure.context(format!("the log could not be flushed either: {e:#}")))
        }
    }
}

/// Waits until every actor has finished, or until the first one fails.
///
/// An actor that fails — including one whose log could not be written —
/// fails the episode there and then. Nothing the others go on to produce
/// could be trusted, so there is nothing to gain by waiting for them.
async fn all_finished(
    running: &mut JoinSet<Result<()>>,
    names: &HashMap<task::Id, ActorId>,
) -> Result<()> {
    while let Some(finished) = running.join_next_with_id().await {
        match finished {
            Ok((_, Ok(()))) => {}
            Ok((task, Err(e))) => {
                return Err(e.context(format!("actor {} failed", names[&task])));
            }
            Err(e) => {
                let how = if e.is_panic() {
                    "panicked"
                } else {
                    "was cancelled"
                };
                return Err(anyhow!("actor {} {how}", names[&e.id()]));
            }
        }
    }
    Ok(())
}

/// A message that expects an answer, and the means of giving one.
///
/// A plain [`Channels::send`] delivers its payload bare on the inbox, because
/// nothing is owed in return. A [`Channels::request`] delivers this instead,
/// on its own channel, so a responder can tell the two apart and knows where
/// the answer goes.
///
/// The reply channel holds exactly one message and belongs to this request
/// alone. Dropping an envelope without answering is allowed — the requester
/// sees the channel close and reports that it will get no answer, rather than
/// waiting out its timeout.
#[derive(Debug)]
pub struct Envelope<M: Message> {
    from: ActorId,
    payload: M,
    /// This question's own reply channel, used once.
    reply_to: oneshot::Sender<M>,
}

impl<M: Message> Envelope<M> {
    /// Who asked.
    pub fn from(&self) -> &ActorId {
        &self.from
    }

    /// What was asked.
    pub fn payload(&self) -> &M {
        &self.payload
    }

    /// Takes the question out of the envelope, leaving the means of replying.
    pub fn open(self) -> (M, Reply<M>) {
        (
            self.payload,
            Reply {
                to: self.from,
                channel: self.reply_to,
            },
        )
    }

    /// Answers the request.
    ///
    /// # Errors
    ///
    /// Fails if the requester is no longer waiting — it timed out, was
    /// stopped, or has finished.
    pub fn reply(self, message: M) -> Result<()> {
        let (_, reply) = self.open();
        reply.send(message)
    }
}

/// The right to answer one request, separated from the question itself so a
/// responder can carry it while it does the work.
#[derive(Debug)]
pub struct Reply<M: Message> {
    to: ActorId,
    channel: oneshot::Sender<M>,
}

impl<M: Message> Reply<M> {
    /// Who is waiting on this.
    pub fn to(&self) -> &ActorId {
        &self.to
    }

    /// Answers the request.
    ///
    /// # Errors
    ///
    /// Fails if the requester is no longer waiting.
    pub fn send(self, message: M) -> Result<()> {
        self.channel
            .send(message)
            .map_err(|_| anyhow!("{} is no longer waiting for an answer", self.to))
    }
}

/// Everything an actor needs to talk to the episode and to its peers.
///
/// The actor owns these: they move into its task. The episode keeps no end
/// of any channel for itself, so that when an actor finishes its receivers
/// drop, and a peer that goes on writing to it is told it is gone.
#[derive(Debug)]
pub struct Channels<M: Message> {
    /// Who this actor is. Worth reading rather than remembering: it is what
    /// peers address, and what the episode stamps on anything logged here.
    pub id: ActorId,
    /// Messages peers sent this actor, oldest first. Answers to questions it
    /// asked do not come here — each comes back from the
    /// [`request`](Self::request) that asked. An actor holds a sender to its
    /// own inbox, so this never closes while the actor runs: waiting for it
    /// to is waiting forever.
    pub inbox: mpsc::UnboundedReceiver<M>,
    outbox: HashMap<ActorId, mpsc::UnboundedSender<M>>,
    /// Questions from peers, each carrying the means of answering it. Kept
    /// apart from `inbox` so a plain send stays a plain `M` and a responder
    /// can tell which messages owe an answer.
    pub requests: mpsc::UnboundedReceiver<Envelope<M>>,
    asks: HashMap<ActorId, mpsc::UnboundedSender<Envelope<M>>>,
    /// This actor's own stop signal, ready to select on alongside the inbox:
    /// `channels.stop.cancelled()` completes once the actor has been asked
    /// to stop, whether it asked itself or a peer did. Once asked it stays
    /// asked, however late the actor looks.
    pub stop: CancellationToken,
    /// Stop signals for every actor, including this one, so an actor may stop
    /// itself as well as stop a peer.
    stops: HashMap<ActorId, CancellationToken>,
    /// Shared by every actor in the episode.
    log: Arc<dyn Sink>,
}

impl<M: Message> Channels<M> {
    /// Sends `message` to a peer.
    ///
    /// Delivery is asynchronous: this returns once the message is queued, not
    /// once it is read. Sending to an id that is not in this episode is an
    /// error, as is sending to a peer that has already finished — the message
    /// is dropped and the unreachable actor is named.
    ///
    /// # Errors
    ///
    /// Fails if `actor` is not a peer in this episode, or has already
    /// finished and stopped receiving.
    pub fn send(&self, message: M, actor: &ActorId) -> Result<()> {
        self.outbox
            .get(actor)
            .ok_or_else(|| anyhow!("no outbound channel to {actor}"))?
            // `SendError` hands the message back, and reporting it would force
            // `M: Sync` on every domain type. The identity of the unreachable
            // actor is the part worth keeping; the payload is dropped.
            .send(message)
            .map_err(|_| anyhow!("{actor} is no longer receiving messages"))?;
        Ok(())
    }

    /// Asks a peer a question and waits for the answer.
    ///
    /// The question reaches the peer on its [`requests`](Self::requests)
    /// channel as an [`Envelope`], carrying a reply channel belonging to this
    /// call alone. That is all the correlation there is to do: an answer can
    /// only come back to the request it answers.
    ///
    /// Only this request waits. The rest of the episode runs on, and so does
    /// anything this actor joined the request with.
    ///
    /// # Asking several peers at once
    ///
    /// Join the requests, and every question is in flight before any answer
    /// is waited on. Three peers taking a second each cost a second in
    /// total, not three.
    ///
    /// ```no_run
    /// # use free_agent::actor::ActorId;
    /// # use free_agent::episode::Channels;
    /// use futures::future::join_all;
    ///
    /// # async fn f(channels: &Channels<String>, peers: &[ActorId]) {
    /// let answers = join_all(
    ///     peers
    ///         .iter()
    ///         .map(|peer| channels.request("who dies tonight?".to_string(), peer)),
    /// )
    /// .await;
    ///
    /// // One answer for each peer, in the order they were asked.
    /// for (peer, answer) in peers.iter().zip(answers) {}
    /// # }
    /// ```
    ///
    /// # Giving up
    ///
    /// A request is a future like any other, so it is bounded the way any
    /// other is: wrap it in [`tokio::time::timeout`], or give several one
    /// [`timeout_at`](tokio::time::timeout_at) deadline to share. A request
    /// that is given up on is simply dropped, and the peer finds out if it
    /// tries to answer.
    ///
    /// # Errors
    ///
    /// Fails if `actor` is not a peer in this episode, has already finished,
    /// or drops the question without answering.
    ///
    /// # Examples
    ///
    /// The responder reads its own channel and answers through the envelope:
    ///
    /// ```
    /// # use free_agent::actor::{Acting, Actor, ActorId, async_trait};
    /// # use free_agent::episode::Channels;
    /// # use anyhow::Result;
    /// # struct Answerer;
    /// #[async_trait]
    /// impl Actor<String> for Answerer {
    ///     async fn perceive(&mut self, channels: &mut Channels<String>) -> Result<()> {
    ///         while let Some(question) = channels.requests.recv().await {
    ///             let asked = question.payload().clone();
    ///             question.reply(format!("you said {asked}"))?;
    ///         }
    ///         Ok(())
    ///     }
    /// }
    /// ```
    pub async fn request(&self, message: M, actor: &ActorId) -> Result<M> {
        // One slot, used once: this channel exists for this question only.
        let (reply_to, answer) = oneshot::channel();
        self.asks
            .get(actor)
            .ok_or_else(|| anyhow!("no outbound channel to {actor}"))?
            .send(Envelope {
                from: self.id.clone(),
                payload: message,
                reply_to,
            })
            .map_err(|_| anyhow!("{actor} is no longer receiving messages"))?;

        answer.await.map_err(|_| anyhow!("{actor} will not answer"))
    }

    /// Records an observation, tagged with this actor's id and the time.
    ///
    /// What is worth logging is the actor's own judgment; the episode only
    /// supplies somewhere to put it. Any [`Serialize`] value will do, and it
    /// reaches the sink as structure rather than as text.
    ///
    /// # Errors
    ///
    /// Fails if the payload cannot be serialized, or if the episode's sink
    /// refuses it. Either ends this actor and fails the episode.
    pub fn log(&self, payload: &impl Serialize) -> Result<()> {
        self.log.write(Entry::new(self.id.clone(), payload)?)
    }

    /// Asks `actor` to stop, which may be this actor itself.
    ///
    /// The signal is a request, not a kill: the recipient notices it the next
    /// time it looks at [`Channels::stop`], and decides what to do before
    /// returning. Stopping an actor that has already finished, or one already
    /// asked to stop, does nothing and is not a failure.
    ///
    /// # Errors
    ///
    /// Fails only if `actor` is not in this episode.
    pub fn stop(&self, actor: &ActorId) -> Result<()> {
        self.stops
            .get(actor)
            .ok_or_else(|| anyhow!("no stop channel to {actor}"))?
            .cancel();
        Ok(())
    }
}

/// Builds the channel set for each actor: fully-connected message and
/// shutdown topologies, both including the self-edge.
fn wire<M: Message>(
    actor_ids: impl IntoIterator<Item = impl Into<ActorId>>,
    log: Arc<dyn Sink>,
) -> HashMap<ActorId, Channels<M>> {
    let mut outbox = HashMap::new();
    let mut asks = HashMap::new();
    let mut stops = HashMap::new();
    let mut receiving = Vec::new();
    for id in actor_ids {
        let id: ActorId = id.into();
        let (send, inbox) = mpsc::unbounded_channel();
        // Requests travel their own path, so a plain send stays a plain `M`.
        let (ask, requests) = mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        outbox.insert(id.clone(), send);
        asks.insert(id.clone(), ask);
        stops.insert(id.clone(), stop.clone());
        receiving.push((id, inbox, requests, stop));
    }

    // Every actor can reach every actor, so each is handed the same three
    // maps. The originals drop on the way out of here, leaving the actors'
    // copies as the only senders there are.
    receiving
        .into_iter()
        .map(|(id, inbox, requests, stop)| {
            let channels = Channels {
                id: id.clone(),
                inbox,
                outbox: outbox.clone(),
                requests,
                asks: asks.clone(),
                stop,
                stops: stops.clone(),
                log: log.clone(),
            };
            (id, channels)
        })
        .collect()
}

/// One logged observation: what an actor had to say, and the two facts the
/// episode knows about it that the actor itself should not have to supply.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The actor that recorded this, supplied by the episode rather than by
    /// whoever wrote the payload.
    pub actor: ActorId,
    /// When it was recorded. Entries from different actors interleave freely,
    /// so this is what puts a run back in order.
    pub at: SystemTime,
    /// Whatever the developer chose to log, serialized. Keeping it structured
    /// rather than rendered is what lets a sink write columns instead of text.
    pub payload: serde_json::Value,
}

impl Entry {
    /// Stamps a payload with its author and the current time.
    ///
    /// Actors do not normally call this: it is what
    /// [`Channels::log`] does on their behalf.
    /// Build one directly to write to a sink from outside an episode.
    ///
    /// # Errors
    ///
    /// Fails if `payload` cannot be serialized.
    pub fn new(actor: impl Into<ActorId>, payload: &impl Serialize) -> Result<Self> {
        Ok(Self {
            actor: actor.into(),
            at: SystemTime::now(),
            payload: serde_json::to_value(payload)?,
        })
    }
}

/// Where an episode's log entries go. Every actor in an episode shares one
/// sink, and the runtime may run them on different threads, so `write` takes
/// `&self` and an implementation keeps whatever locking it needs to itself.
///
/// `flush` exists because a buffering sink — a file, a socket, a parquet row
/// group — would otherwise lose entries when an episode ends. A sink that
/// writes through can leave it as the default.
///
/// `Debug` is required so that anything holding a sink stays printable: where
/// an episode's output went is part of how it was configured, and worth seeing
/// when a run misbehaves.
pub trait Sink: Send + Sync + std::fmt::Debug {
    /// Records one entry. Called from whichever thread the logging actor is
    /// running on, so an implementation that needs exclusive access brings
    /// its own.
    ///
    /// The actor waits on this without yielding to its peers, so it should
    /// be quick.
    ///
    /// # Errors
    ///
    /// Whatever recording failed: a closed socket, a full disk. An error here
    /// ends the actor that was writing, and with it the episode.
    fn write(&self, entry: Entry) -> Result<()>;

    /// Commits anything held back. The episode calls this as it ends, on the
    /// way out of a successful run and a failed or timed-out one alike.
    ///
    /// The default does nothing, which is right for a sink that writes each
    /// entry through as it arrives.
    ///
    /// # Errors
    ///
    /// Whatever committing failed. The episode reports it rather than
    /// returning success for a run it could not finish recording.
    fn flush(&self) -> Result<()> {
        Ok(())
    }
}

/// A shared sink is a sink, so a caller can keep a handle to one it hands to
/// an episode — to read a `Memory` afterward, or to log from outside the run.
impl<S: Sink + ?Sized> Sink for Arc<S> {
    fn write(&self, entry: Entry) -> Result<()> {
        (**self).write(entry)
    }

    fn flush(&self) -> Result<()> {
        (**self).flush()
    }
}

/// Writes one JSON object per line to standard error.
///
/// The simplest sink that is still structured: a reader can parse the lines
/// back into entries rather than scraping prose. Each line is rendered whole
/// before it reaches the stream, so concurrent actors never split one
/// another's output.
///
/// ```text
/// {"actor":"Alice","at":1790892090.58119,"payload":{"heard_from":"Carol"}}
/// ```
#[derive(Debug)]
pub struct Stderr;

impl Stderr {
    /// One entry as the line it will be written as.
    fn line(entry: &Entry) -> String {
        let at = entry
            .at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        serde_json::json!({"actor": entry.actor, "at": at, "payload": entry.payload}).to_string()
    }
}

impl Sink for Stderr {
    fn write(&self, entry: Entry) -> Result<()> {
        // Render first, then write once while holding the stream lock.
        // Formatting into the stream would let a concurrent actor interleave
        // its own output partway through this line, and the format promises
        // one entry per line.
        let mut line = Self::line(&entry);
        line.push('\n');
        stderr().lock().write_all(line.as_bytes())?;
        Ok(())
    }
}

/// Keeps every entry in memory.
///
/// Useful to a test that wants to assert on what an episode logged, and to a
/// caller that means to write a whole run out at once. Hand an episode an
/// [`Arc<Memory>`](Arc) to keep a handle you can read afterward.
///
/// ```
/// use free_agent::episode::{Entry, Memory, Sink};
/// use std::sync::Arc;
///
/// let log = Arc::new(Memory::new());
/// log.write(Entry::new("Alice", &"something happened")?)?;
/// assert_eq!(log.entries().len(), 1);
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Debug, Default)]
pub struct Memory {
    entries: Mutex<Vec<Entry>>,
}

impl Memory {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every entry recorded so far, oldest first.
    ///
    /// This is a snapshot: actors still running may add to the log after it
    /// returns, so read it once the episode is over if you mean to see all of
    /// it.
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().clone()
    }
}

impl Sink for Memory {
    fn write(&self, entry: Entry) -> Result<()> {
        self.entries.lock().unwrap().push(entry);
        Ok(())
    }
}

/// Discards everything.
///
/// For an episode whose actors log nothing, or one whose log does not matter
/// — a test about shutdown, say. Writes always succeed, so a `Discard` never
/// fails a run.
#[derive(Debug)]
pub struct Discard;

impl Sink for Discard {
    fn write(&self, _entry: Entry) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{Acting, Actor, ActorId, Message, async_trait};
    use anyhow::{Result, anyhow};
    use futures::future::{join_all, try_join_all};
    use serde::{Deserialize, Serialize};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, SystemTime};
    use tokio::time::{Instant, sleep, timeout, timeout_at};

    /// A domain object, not a string: what actors will really exchange.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Greeting {
        from: ActorId,
        salutation: String,
    }

    /// An actor with no state and no behavior of its own: it waits to be
    /// stopped. Every episode needs a roster, and some tests care only about
    /// how the episode ends, not about what its actors do.
    struct Idle;

    impl Idle {
        /// An idle actor, ready to join an episode's roster.
        fn actor<M: Message>() -> Box<dyn Actor<M>> {
            Box::new(Self)
        }
    }

    impl<M: Message> Actor<M> for Idle {}

    /// Stops itself as soon as it starts, then runs the default loop.
    struct StopsItself;

    impl StopsItself {
        /// An actor that stops itself as soon as it starts.
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for StopsItself {
        async fn wake(&mut self, channels: &mut Channels<Greeting>) -> Result<Acting> {
            channels.stop(&channels.id)?;
            Ok(Acting::Continue)
        }
    }

    /// Stops every actor in the episode, itself included.
    struct StopsEveryone;

    impl StopsEveryone {
        /// An actor that stops the whole episode.
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for StopsEveryone {
        async fn wake(&mut self, channels: &mut Channels<Greeting>) -> Result<Acting> {
            for peer in channels.stops.keys() {
                channels.stop(peer)?;
            }
            Ok(Acting::Continue)
        }
    }

    /// Waits to be stopped without selecting on its inbox at all.
    struct Wedged;

    impl Wedged {
        /// An actor that only ever waits to be stopped.
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Wedged {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            channels.stop.cancelled().await;
            Ok(())
        }
    }

    /// Greets a peer on startup, then records the greetings it receives.
    struct Greeter {
        greet: Option<ActorId>,
        heard: Arc<Mutex<Vec<Greeting>>>,
        /// Whether to wait for a greeting of its own before finishing. An
        /// inbox never disconnects while its actor runs, so an actor that
        /// expects nothing must say when it is done.
        awaits_reply: bool,
    }

    impl Greeter {
        /// A greeter that opens with `peer`, if any, and reports what it hears
        /// into `heard`. It finishes once it has heard one greeting.
        fn actor(peer: Option<&str>, heard: Arc<Mutex<Vec<Greeting>>>) -> Box<dyn Actor<Greeting>> {
            Box::new(Self {
                greet: peer.map(str::to_string),
                heard,
                awaits_reply: true,
            })
        }

        /// A greeter that speaks and leaves without waiting to be greeted
        /// back.
        fn speaks_and_leaves(peer: &str) -> Box<dyn Actor<Greeting>> {
            Box::new(Self {
                greet: Some(peer.to_string()),
                heard: Arc::default(),
                awaits_reply: false,
            })
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Greeter {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            if let Some(peer) = self.greet.clone() {
                channels.send(
                    Greeting {
                        from: channels.id.clone(),
                        salutation: "hello".to_string(),
                    },
                    &peer,
                )?;
                if !self.awaits_reply {
                    // Spoke its piece and expects no reply, so it is done.
                    // Waiting would be waiting forever: an inbox stays open as
                    // long as its own actor holds a sender to it.
                    channels.stop(&channels.id)?;
                }
            }
            loop {
                tokio::select! {
                    greeting = channels.inbox.recv() => match greeting {
                        Some(greeting) => {
                            self.heard.lock().unwrap().push(greeting);
                            // Heard a greeting, which is all this actor waits
                            // for.
                            channels.stop(&channels.id)?;
                        }
                        None => return Ok(()),
                    },
                    () = channels.stop.cancelled() => return Ok(()),
                }
            }
        }
    }

    /// Logs one observation on startup, then stops itself.
    struct Logs(&'static str);

    impl Logs {
        fn actor(note: &'static str) -> Box<dyn Actor<Greeting>> {
            Box::new(Self(note))
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Logs {
        async fn wake(&mut self, channels: &mut Channels<Greeting>) -> Result<Acting> {
            channels.log(&self.0)?;
            channels.stop(&channels.id)?;
            Ok(Acting::Continue)
        }
    }

    /// Replies to the first greeting it hears, then stops. A second behavior,
    /// distinct from `Greeter`: it speaks only in response.
    struct Echoer;

    impl Echoer {
        /// An actor that answers the first greeting it hears.
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Echoer {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            loop {
                tokio::select! {
                    delivery = channels.inbox.recv() => match delivery {
                        Some(greeting) => {
                            channels.send(
                                Greeting {
                                    from: channels.id.clone(),
                                    salutation: format!("re: {}", greeting.salutation),
                                },
                                &greeting.from,
                            )?;
                            channels.stop(&channels.id)?;
                        }
                        None => return Ok(()),
                    },
                    () = channels.stop.cancelled() => return Ok(()),
                }
            }
        }
    }

    /// Fails if `episode` has not finished within `patience`. The episode's
    /// own timeout is the thing under test in places, so it cannot always be
    /// the bound that keeps a broken run from hanging the whole test binary.
    async fn finishes_within<T>(patience: Duration, episode: impl Future<Output = T>) -> T {
        timeout(patience, episode)
            .await
            .expect("episode did not finish")
    }

    #[tokio::test]
    async fn actors_that_stop_themselves_need_no_timeout() {
        // Each actor signals its own stop, so the episode ends without a bound.
        // `None` is the claim under test, so the patience here is the test's
        // own, imposed from outside rather than passed to the episode.
        let result = finishes_within(
            Duration::from_secs(5),
            episode(
                [
                    ("Alice", StopsItself::actor()),
                    ("Bob", StopsItself::actor()),
                    ("Carol", StopsItself::actor()),
                ],
                Discard,
                None,
            ),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);
    }

    #[tokio::test]
    async fn one_actor_can_stop_the_others() {
        // Alice stops everyone, including herself. The timeout is only a
        // backstop: reaching it means the stop signals did not arrive, and the
        // episode reports that rather than hanging the test run.
        let result = episode(
            [
                ("Alice", StopsEveryone::actor()),
                ("Bob", Idle::actor()),
                ("Carol", Idle::actor()),
            ],
            Discard,
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_actors_deadlock_without_a_timeout() {
        // The default loop waits to be told to stop, and an actor holds its
        // own inbox open, so an idle actor has no reason to ever return. With
        // nobody to stop them, only the timeout ends the run.
        let result = episode(
            [
                // `Idle` serves any message type, and an all-idle roster sends
                // nothing that would imply one, so name it here.
                ("Alice", Idle::actor::<Greeting>()),
                ("Bob", Idle::actor()),
            ],
            Discard,
            Some(Duration::from_millis(50)),
        )
        .await;
        assert!(
            result.is_err(),
            "expected a timeout error, got {:?}",
            result
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_wedged_actor_is_stopped_by_the_timeout() {
        // Alice never stops on her own; the timeout must end her.
        let result = episode(
            [("Alice", Wedged::actor())],
            Discard,
            Some(Duration::from_millis(50)),
        )
        .await;
        assert!(
            result.is_err(),
            "expected a timeout error, got {:?}",
            result
        );
    }

    /// Logs on a timer for as long as it is left running, and ignores every
    /// request to stop.
    struct Chatters;

    impl Chatters {
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Chatters {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            loop {
                channels.log(&"still here")?;
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    /// A timeout ends the actors as well as the wait. One that would have
    /// gone on forever gets no further once the episode has returned.
    #[tokio::test(start_paused = true)]
    async fn an_actor_cut_off_by_the_timeout_does_not_run_on() {
        let log = Arc::new(Memory::new());
        let result = episode(
            [("Alice", Chatters::actor())],
            log.clone(),
            Some(Duration::from_millis(50)),
        )
        .await;
        assert!(
            result.is_err(),
            "expected a timeout error, got {:?}",
            result
        );
        let written = log.entries().len();
        assert!(written > 0, "Alice should have logged before the timeout");

        // Long enough for several more entries, had she been left running.
        sleep(Duration::from_millis(100)).await;
        assert_eq!(log.entries().len(), written);
    }

    /// Runs Alice greeting Bob, and returns what Bob heard.
    async fn greeting_episode() -> Vec<Greeting> {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let result = episode(
            [
                ("Alice", Greeter::speaks_and_leaves("Bob")),
                ("Bob", Greeter::actor(None, heard.clone())),
            ],
            Discard,
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);
        let heard = heard.lock().unwrap();
        heard.clone()
    }

    #[tokio::test]
    async fn an_actor_keeps_its_own_state_across_the_run() {
        // Alice greets Bob, Bob hears it: a per-actor field carries the result
        // out, and the two actors have different perception loops.
        assert_eq!(greeting_episode().await.len(), 1);
    }

    /// An actor that queues work for itself, then stops once it has drained
    /// what it queued.
    struct Deferring {
        steps: usize,
        done: Arc<Mutex<Vec<Greeting>>>,
    }

    impl Deferring {
        fn actor(steps: usize, done: Arc<Mutex<Vec<Greeting>>>) -> Box<dyn Actor<Greeting>> {
            Box::new(Self { steps, done })
        }

        fn note(&self, step: usize) -> Greeting {
            Greeting {
                from: "self".to_string(),
                salutation: format!("step {step}"),
            }
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Deferring {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            channels.send(self.note(0), &channels.id)?;
            loop {
                tokio::select! {
                    note = channels.inbox.recv() => match note {
                        Some(note) => {
                            self.done.lock().unwrap().push(note);
                            let next = self.done.lock().unwrap().len();
                            if next < self.steps {
                                channels.send(self.note(next), &channels.id)?;
                            } else {
                                channels.stop(&channels.id)?;
                            }
                        }
                        None => return Ok(()),
                    },
                    () = channels.stop.cancelled() => return Ok(()),
                }
            }
        }
    }

    /// An episode of one is a legitimate roster. With no peers at all, the
    /// self-edge is the only channel an actor has.
    #[tokio::test]
    async fn an_episode_can_hold_a_single_actor() {
        let done = Arc::new(Mutex::new(Vec::new()));
        let result = finishes_within(
            Duration::from_secs(5),
            episode(
                [("Alone", Deferring::actor(3, done.clone()))],
                Discard,
                None,
            ),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);
        // Each step was queued by the actor itself and came back around.
        let notes: Vec<String> = done
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.salutation.clone())
            .collect();
        assert_eq!(notes, ["step 0", "step 1", "step 2"]);
    }

    /// Sends a peer some notes, in order, and leaves.
    struct Dictates {
        to: ActorId,
        notes: Vec<&'static str>,
    }

    impl Dictates {
        fn actor(to: &str, notes: &[&'static str]) -> Box<dyn Actor<Greeting>> {
            Box::new(Self {
                to: to.to_string(),
                notes: notes.to_vec(),
            })
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Dictates {
        async fn wake(&mut self, channels: &mut Channels<Greeting>) -> Result<Acting> {
            for note in &self.notes {
                channels.send(
                    Greeting {
                        from: channels.id.clone(),
                        salutation: note.to_string(),
                    },
                    &self.to,
                )?;
            }
            Ok(Acting::Done)
        }
    }

    /// Takes down what it is sent, slowly, and is done once it has as many
    /// notes as it expects.
    struct TakesNotes {
        expects: usize,
        taken: Arc<Mutex<Vec<String>>>,
    }

    impl TakesNotes {
        fn actor(expects: usize, taken: Arc<Mutex<Vec<String>>>) -> Box<dyn Actor<Greeting>> {
            Box::new(Self { expects, taken })
        }
    }

    #[async_trait]
    impl Actor<Greeting> for TakesNotes {
        async fn act(
            &mut self,
            message: Greeting,
            _channels: &mut Channels<Greeting>,
        ) -> Result<Acting> {
            sleep(Duration::from_millis(10)).await;
            let mut taken = self.taken.lock().unwrap();
            taken.push(message.salutation);
            Ok(if taken.len() < self.expects {
                Acting::Continue
            } else {
                Acting::Done
            })
        }
    }

    /// An actor that only says how to `act` is handed its messages one at a
    /// time, oldest first. Those that arrive while it is busy with one wait
    /// their turn in the inbox; none is lost and none overtakes another.
    #[tokio::test(start_paused = true)]
    async fn an_actor_is_handed_its_messages_one_at_a_time() {
        let taken = Arc::new(Mutex::new(Vec::new()));
        let result = finishes_within(
            Duration::from_secs(5),
            episode(
                [
                    ("Alice", Dictates::actor("Bob", &["one", "two", "three"])),
                    ("Bob", TakesNotes::actor(3, taken.clone())),
                ],
                Discard,
                None,
            ),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(*taken.lock().unwrap(), ["one", "two", "three"]);
    }

    /// Runs a roster with a backstop timeout and a discarded log.
    async fn actor_episode_with(
        actors: impl IntoIterator<Item = (&'static str, Box<dyn Actor<Greeting>>)>,
    ) -> Result<()> {
        episode(actors, Discard, Some(Duration::from_secs(5))).await
    }

    /// Answers every request it is handed, until it is stopped.
    struct Responder;

    impl Responder {
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Responder {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            loop {
                tokio::select! {
                    request = channels.requests.recv() => match request {
                        Some(request) => {
                            let asked = request.payload().salutation.clone();
                            request.reply(Greeting {
                                from: channels.id.clone(),
                                salutation: format!("re: {asked}"),
                            })?;
                        }
                        None => return Ok(()),
                    },
                    () = channels.stop.cancelled() => return Ok(()),
                }
            }
        }
    }

    /// Asks `peer` one question and records the answer.
    struct Asks {
        peer: ActorId,
        answer: Arc<Mutex<Option<Greeting>>>,
    }

    impl Asks {
        fn actor(peer: &str, answer: Arc<Mutex<Option<Greeting>>>) -> Box<dyn Actor<Greeting>> {
            Box::new(Self {
                peer: peer.to_string(),
                answer,
            })
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Asks {
        async fn wake(&mut self, channels: &mut Channels<Greeting>) -> Result<Acting> {
            let answer = channels
                .request(
                    Greeting {
                        from: channels.id.clone(),
                        salutation: "ping".to_string(),
                    },
                    &self.peer,
                )
                .await?;
            *self.answer.lock().unwrap() = Some(answer);
            channels.stop(&self.peer)?;
            Ok(Acting::Done)
        }
    }

    /// Answers slowly, so a fan-out that waited serially would take as long
    /// as the sum of its answers.
    struct SlowResponder;

    impl SlowResponder {
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for SlowResponder {
        async fn perceive(&mut self, channels: &mut Channels<Greeting>) -> Result<()> {
            loop {
                tokio::select! {
                    request = channels.requests.recv() => match request {
                        Some(request) => {
                            sleep(Duration::from_millis(100)).await;
                            request.reply(Greeting {
                                from: channels.id.clone(),
                                salutation: "voted".to_string(),
                            })?;
                        }
                        None => return Ok(()),
                    },
                    () = channels.stop.cancelled() => return Ok(()),
                }
            }
        }
    }

    /// Polls several peers at once and gathers their answers: the Werewolf
    /// night, where the environment waits for every living werewolf.
    struct Gathers {
        peers: Vec<ActorId>,
        votes: Arc<Mutex<HashMap<ActorId, String>>>,
    }

    impl Gathers {
        fn actor(
            peers: &[&str],
            votes: Arc<Mutex<HashMap<ActorId, String>>>,
        ) -> Box<dyn Actor<Greeting>> {
            Box::new(Self {
                peers: peers.iter().map(|p| p.to_string()).collect(),
                votes,
            })
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Gathers {
        async fn wake(&mut self, channels: &mut Channels<Greeting>) -> Result<Acting> {
            // Joined, so every question is in flight before any answer is
            // waited on.
            let answers = try_join_all(self.peers.iter().map(|peer| {
                channels.request(
                    Greeting {
                        from: channels.id.clone(),
                        salutation: "who dies tonight?".to_string(),
                    },
                    peer,
                )
            }))
            .await?;

            for (who, answer) in self.peers.iter().zip(answers) {
                self.votes
                    .lock()
                    .unwrap()
                    .insert(who.clone(), answer.salutation);
            }
            for peer in &self.peers {
                channels.stop(peer)?;
            }
            Ok(Acting::Done)
        }
    }

    #[tokio::test]
    async fn a_request_returns_the_peer_s_answer() {
        let answer = Arc::new(Mutex::new(None));
        let result = actor_episode_with([
            ("Asker", Asks::actor("Answerer", answer.clone())),
            ("Answerer", Responder::actor()),
        ])
        .await;
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(
            answer.lock().unwrap().take().unwrap().salutation,
            "re: ping"
        );
    }

    /// Joining the requests is what makes a fan-out parallel: three peers
    /// that each take 100ms cost about 100ms together, not 300ms.
    #[tokio::test(start_paused = true)]
    async fn a_fan_out_waits_once_for_the_slowest() {
        let votes = Arc::new(Mutex::new(HashMap::new()));
        let began = Instant::now();
        let result = actor_episode_with([
            (
                "Environment",
                Gathers::actor(&["Wolf1", "Wolf2", "Wolf3"], votes.clone()),
            ),
            ("Wolf1", SlowResponder::actor()),
            ("Wolf2", SlowResponder::actor()),
            ("Wolf3", SlowResponder::actor()),
        ])
        .await;
        assert!(result.is_ok(), "{:?}", result);
        let took = began.elapsed();

        let votes = votes.lock().unwrap();
        assert_eq!(votes.len(), 3, "{votes:?}");
        for wolf in ["Wolf1", "Wolf2", "Wolf3"] {
            assert_eq!(votes[wolf], "voted");
        }
        // Serial waiting would have cost 300ms.
        assert!(took < Duration::from_millis(250), "took {took:?}");
    }

    /// A peer that will not answer in time must not hold the asker forever:
    /// a request can be bounded like any other future.
    #[tokio::test(start_paused = true)]
    async fn a_question_can_be_given_up_on() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = &channels["Alice"];

        let began = Instant::now();
        let answer = timeout(
            Duration::from_millis(50),
            alice.request(
                Greeting {
                    from: "Alice".to_string(),
                    salutation: "anyone?".to_string(),
                },
                &"Bob".to_string(),
            ),
        )
        .await;
        assert!(answer.is_err(), "Bob never answered, yet: {answer:?}");
        assert!(began.elapsed() >= Duration::from_millis(50));
    }

    /// One deadline shared across several questions costs the time until the
    /// deadline, not that much time for each.
    #[tokio::test(start_paused = true)]
    async fn a_deadline_can_be_shared_across_questions() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = &channels["Alice"];
        let bob = "Bob".to_string();

        let began = Instant::now();
        let deadline = began + Duration::from_millis(50);
        let answers = join_all((0..3).map(|n| {
            timeout_at(
                deadline,
                alice.request(
                    Greeting {
                        from: "Alice".to_string(),
                        salutation: format!("question {n}"),
                    },
                    &bob,
                ),
            )
        }))
        .await;

        assert_eq!(answers.len(), 3);
        assert!(answers.iter().all(|answer| answer.is_err()), "{answers:?}");
        // Three waits, one deadline: well under 150ms.
        assert!(began.elapsed() < Duration::from_millis(120));
    }

    /// A peer may drop a question unanswered. The asker is told so at once,
    /// rather than left to wait out whatever deadline it set.
    #[tokio::test]
    async fn a_dropped_question_is_not_waited_on() {
        let mut channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.remove("Alice").unwrap();
        let mut bob = channels.remove("Bob").unwrap();
        let to_bob = "Bob".to_string();

        let (answer, ()) = tokio::join!(
            alice.request(
                Greeting {
                    from: "Alice".to_string(),
                    salutation: "anyone?".to_string(),
                },
                &to_bob,
            ),
            async { drop(bob.requests.recv().await) },
        );
        let message = format!("{:#}", answer.unwrap_err());
        assert!(message.contains("will not answer"), "{message}");
    }

    /// Asking an actor that is not in the episode fails rather than hanging.
    #[tokio::test]
    async fn a_request_to_an_unknown_actor_is_an_error() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = &channels["Alice"];
        let result = alice
            .request(
                Greeting {
                    from: "Alice".to_string(),
                    salutation: "hi".to_string(),
                },
                &"Nobody".to_string(),
            )
            .await;
        assert!(result.is_err());
    }

    /// Two actor types in one roster, each doing its own kind of work: the
    /// `Greeter` opens, the `Echoer` answers. The reply is evidence that both
    /// behaviors ran, so neither type can be dropped without failing.
    #[tokio::test]
    async fn an_episode_can_run_two_different_actor_types() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let result = episode(
            [
                ("Alice", Greeter::actor(Some("Bob"), heard.clone())),
                ("Bob", Echoer::actor()),
            ],
            Discard,
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);

        // Alice hears Bob's reply, not her own greeting echoed verbatim.
        let heard = heard.lock().unwrap();
        assert_eq!(
            *heard,
            [Greeting {
                from: "Bob".to_string(),
                salutation: "re: hello".to_string(),
            }]
        );
    }

    /// A sink that refuses every entry, for testing what an episode does when
    /// its log cannot be written.
    #[derive(Debug)]
    struct Broken;

    impl Sink for Broken {
        fn write(&self, _entry: Entry) -> Result<()> {
            Err(anyhow!("sink is broken"))
        }
    }

    /// Gives up with an error as soon as it starts.
    struct Fails;

    impl Fails {
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Fails {
        async fn wake(&mut self, _channels: &mut Channels<Greeting>) -> Result<Acting> {
            Err(anyhow!("out of ideas"))
        }
    }

    /// An actor's failure is the episode's, reported as it happens. The
    /// peers are not waited for, so a roster that would otherwise idle
    /// forever cannot bury the reason under a timeout.
    #[tokio::test]
    async fn a_failing_actor_fails_the_episode_without_waiting_for_its_peers() {
        // No timeout: reaching one would mean the failure went unreported.
        let result = finishes_within(
            Duration::from_secs(5),
            episode(
                [("Alice", Fails::actor()), ("Bob", Idle::actor())],
                Discard,
                None,
            ),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("Alice"), "{message}");
        assert!(message.contains("out of ideas"), "{message}");
    }

    /// Panics an actor deliberately; the episode must not report success.
    struct Panics;

    impl Panics {
        fn actor() -> Box<dyn Actor<Greeting>> {
            Box::new(Self)
        }
    }

    #[async_trait]
    impl Actor<Greeting> for Panics {
        async fn perceive(&mut self, _channels: &mut Channels<Greeting>) -> Result<()> {
            panic!("actor gave up");
        }
    }

    /// A panic is reported as it happens, so the episode names the actor
    /// instead of waiting for a timeout to notice something is missing.
    #[tokio::test]
    async fn a_panicking_actor_fails_the_episode_by_name() {
        // No timeout: reaching one would mean the panic went unreported.
        let result = finishes_within(
            Duration::from_secs(5),
            episode([("Alice", Panics::actor())], Discard, None),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("Alice"), "{message}");
        assert!(message.contains("panicked"), "{message}");
    }

    /// One actor's panic does not leave its peers waiting out the clock.
    #[tokio::test]
    async fn a_panic_does_not_strand_the_other_actors() {
        let result = finishes_within(
            Duration::from_secs(5),
            episode(
                [("Alice", Panics::actor()), ("Bob", Idle::actor())],
                Discard,
                None,
            ),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("Alice"), "{message}");
    }

    /// A sink that accepts every entry but cannot commit them.
    #[derive(Debug)]
    struct FlushFails;

    impl Sink for FlushFails {
        fn write(&self, _entry: Entry) -> Result<()> {
            Ok(())
        }

        fn flush(&self) -> Result<()> {
            Err(anyhow!("flush exploded"))
        }
    }

    /// Why the run failed outranks a failure to flush. Reporting only the
    /// flush would hide the panic behind a plumbing error.
    #[tokio::test]
    async fn a_failing_flush_does_not_hide_a_panic() {
        let result = finishes_within(
            Duration::from_secs(5),
            episode([("Alice", Panics::actor())], FlushFails, None),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("Alice"), "{message}");
        assert!(message.contains("panicked"), "{message}");
        assert!(message.contains("flush exploded"), "{message}");
    }

    /// The same for a timeout: the run's own failure is the headline.
    #[tokio::test(start_paused = true)]
    async fn a_failing_flush_does_not_hide_a_timeout() {
        let result = episode(
            [("Alice", Wedged::actor())],
            FlushFails,
            Some(Duration::from_millis(50)),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("timed out"), "{message}");
        assert!(message.contains("flush exploded"), "{message}");
    }

    /// Ids address messages and stop signals, so a repeat has no meaning. A
    /// roster that collapsed one away would run fewer actors than it names and
    /// still report success.
    #[tokio::test]
    async fn a_duplicate_actor_id_is_refused() {
        let log = Arc::new(Memory::new());
        let result = episode(
            [
                ("Alice", Logs::actor("first alice")),
                ("Alice", Logs::actor("second alice")),
                ("Bob", Logs::actor("bob was here")),
            ],
            log.clone(),
            Some(Duration::from_secs(5)),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("Alice"), "{message}");
        assert!(message.contains("duplicate"), "{message}");
        // Refused before anything ran, so nothing was recorded.
        assert!(log.entries().is_empty(), "{:?}", log.entries());
    }

    /// Logging is not best-effort: if the sink cannot record what an actor
    /// saw, the run is not trustworthy and the episode says so.
    #[tokio::test]
    async fn a_failing_sink_brings_the_episode_down() {
        let result = episode(
            [
                ("Alice", Logs::actor("alice was here")),
                ("Bob", Logs::actor("bob was here")),
            ],
            Broken,
            Some(Duration::from_secs(5)),
        )
        .await;
        let message = format!("{:?}", result.unwrap_err());
        assert!(message.contains("sink is broken"), "{message}");
    }

    /// Every actor writes to the one place the episode was given, and the
    /// episode supplies the two facts the actor should not have to: who wrote
    /// the entry and when.
    #[tokio::test]
    async fn actors_log_to_the_episode_s_sink() {
        let log = Arc::new(Memory::new());
        let before = SystemTime::now();
        let result = episode(
            [
                ("Alice", Logs::actor("alice was here")),
                ("Bob", Logs::actor("bob was here")),
            ],
            log.clone(),
            Some(Duration::from_secs(5)),
        )
        .await;
        assert!(result.is_ok(), "{:?}", result);

        let entries = log.entries();
        assert_eq!(entries.len(), 2, "{entries:?}");
        // Actors finish in any order, so compare as a set.
        let mut tagged: Vec<_> = entries
            .iter()
            .map(|e| (e.actor.clone(), e.payload.as_str().unwrap().to_string()))
            .collect();
        tagged.sort();
        assert_eq!(
            tagged,
            [
                ("Alice".to_string(), "alice was here".to_string()),
                ("Bob".to_string(), "bob was here".to_string()),
            ]
        );
        assert!(entries.iter().all(|e| e.at >= before));
    }

    #[test]
    fn an_actor_can_message_a_peer() {
        let mut channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.remove("Alice").unwrap();
        let mut bob = channels.remove("Bob").unwrap();
        let greeting = Greeting {
            from: "Alice".to_string(),
            salutation: "hi".to_string(),
        };

        alice
            .outbox
            .get("Bob")
            .unwrap()
            .send(greeting.clone())
            .unwrap();
        assert_eq!(bob.inbox.try_recv().unwrap(), greeting);
    }

    /// The point of the generalization: a domain object arrives as itself, with
    /// its fields intact, not as a string the receiver has to parse.
    #[tokio::test]
    async fn actors_exchange_domain_objects_not_strings() {
        assert_eq!(
            greeting_episode().await,
            [Greeting {
                from: "Alice".to_string(),
                salutation: "hello".to_string(),
            }]
        );
    }

    /// A message type need not resemble a greeting at all: any serde type will
    /// do, including a bare primitive.
    #[test]
    fn an_episode_can_carry_any_serde_type() {
        let mut channels = wire::<u64>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.remove("Alice").unwrap();
        let mut bob = channels.remove("Bob").unwrap();

        alice.outbox.get("Bob").unwrap().send(42).unwrap();
        assert_eq!(bob.inbox.try_recv().unwrap(), 42);
    }

    /// Messages are serde types, so an actor can hand one to a serializer on its
    /// way out of the process without the framework knowing the domain.
    #[test]
    fn an_actor_can_reach_itself_and_its_peers() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.get("Alice").unwrap();
        assert!(alice.outbox.contains_key("Bob"));
        // Message topology includes the self-edge, as shutdown does.
        assert!(alice.outbox.contains_key("Alice"));
    }

    /// An actor can queue work for itself: the message lands in its own inbox
    /// and comes back around through the perception loop.
    #[test]
    fn an_actor_can_send_itself_a_message() {
        let mut channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let mut alice = channels.remove("Alice").unwrap();
        let note = Greeting {
            from: "Alice".to_string(),
            salutation: "remember this".to_string(),
        };

        alice.send(note.clone(), &"Alice".to_string()).unwrap();
        assert_eq!(alice.inbox.try_recv().unwrap(), note);
    }

    #[test]
    fn an_actor_can_signal_shutdown_to_itself_and_to_peers() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.get("Alice").unwrap();
        // Shutdown topology includes the self-edge.
        assert!(alice.stops.contains_key("Alice"));
        assert!(alice.stops.contains_key("Bob"));
    }

    /// A stop reaches the actor it names and nobody else, and once asked it
    /// stays asked: however late the actor looks, the request is still there.
    #[test]
    fn a_stop_reaches_only_the_actor_it_names() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.get("Alice").unwrap();
        let bob = channels.get("Bob").unwrap();

        alice.stop(&"Bob".to_string()).unwrap();
        assert!(bob.stop.is_cancelled());
        assert!(!alice.stop.is_cancelled());
        assert!(alice.stop(&"Nobody".to_string()).is_err());
    }

    #[test]
    fn sending_to_an_unknown_actor_is_an_error() {
        let channels = wire::<Greeting>(["Alice", "Bob"], Arc::new(Discard));
        let alice = channels.get("Alice").unwrap();
        let result = alice.send(
            Greeting {
                from: "Alice".to_string(),
                salutation: "hi".to_string(),
            },
            &"Nobody".to_string(),
        );
        assert!(result.is_err());
    }

    // Tests for the log, which lives in this module too.

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Observed {
        saw: String,
    }

    #[test]
    fn an_entry_carries_its_author_and_payload() {
        let entry = Entry::new(
            "Alice",
            &Observed {
                saw: "a peer".to_string(),
            },
        )
        .unwrap();
        assert_eq!(entry.actor, "Alice");
        assert_eq!(entry.payload["saw"], "a peer");
    }

    #[test]
    fn an_entry_is_stamped_with_the_time_it_was_made() {
        let before = SystemTime::now();
        let entry = Entry::new("Alice", &"something").unwrap();
        let after = SystemTime::now();
        assert!(entry.at >= before && entry.at <= after);
    }

    /// The payload keeps its structure, so a sink can write columns rather
    /// than re-parsing text.
    #[test]
    fn a_payload_survives_as_structured_data() {
        let observed = Observed {
            saw: "a peer".to_string(),
        };
        let entry = Entry::new("Alice", &observed).unwrap();
        let back: Observed = serde_json::from_value(entry.payload).unwrap();
        assert_eq!(back, observed);
    }

    #[test]
    fn a_memory_sink_keeps_what_it_is_given() {
        let sink = Memory::new();
        sink.write(Entry::new("Alice", &"first").unwrap()).unwrap();
        sink.write(Entry::new("Bob", &"second").unwrap()).unwrap();

        let entries = sink.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].actor, "Alice");
        assert_eq!(entries[1].actor, "Bob");
    }

    /// Actors may log from different threads, so a sink must take concurrent
    /// writes without the callers coordinating.
    #[test]
    fn a_sink_accepts_writes_from_many_threads() {
        let sink = Arc::new(Memory::new());
        let writers: Vec<_> = ["Alice", "Bob", "Carol"]
            .map(|name| {
                let sink = sink.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        sink.write(Entry::new(name, &name).unwrap()).unwrap();
                    }
                })
            })
            .into_iter()
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(sink.entries().len(), 300);
    }

    /// A sink is held as a trait object, so the episode can take any of them.
    #[test]
    fn a_sink_can_be_used_as_a_trait_object() {
        let sink: Box<dyn Sink> = Box::new(Memory::new());
        sink.write(Entry::new("Alice", &"logged").unwrap()).unwrap();
        sink.flush().unwrap();
    }

    /// A sink is printable, so anything holding one — `Channels`, an episode's
    /// configuration — stays printable too.
    #[test]
    fn a_sink_is_debug_even_behind_a_trait_object() {
        let sink: Arc<dyn Sink> = Arc::new(Memory::new());
        sink.write(Entry::new("Alice", &"logged").unwrap()).unwrap();
        assert!(format!("{sink:?}").contains("Memory"));
        assert_eq!(format!("{Discard:?}"), "Discard");
    }

    /// Each entry is rendered whole before it reaches the stream, so a line
    /// is never split by a concurrent writer.
    #[test]
    fn a_stderr_line_is_one_entry() {
        let entry = Entry::new(
            "Alice",
            &Observed {
                saw: "a peer".to_string(),
            },
        )
        .unwrap();
        let line = Stderr::line(&entry);
        assert!(!line.contains('\n'), "{line}");

        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["actor"], "Alice");
        assert_eq!(parsed["payload"]["saw"], "a peer");
    }

    #[test]
    fn a_discarding_sink_accepts_everything() {
        let sink = Discard;
        sink.write(Entry::new("Alice", &"ignored").unwrap())
            .unwrap();
        sink.flush().unwrap();
    }
}
