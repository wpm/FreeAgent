//! The framework itself. Every item here is re-exported at the crate root,
//! and the crate documentation, which is the project README, is the place
//! to start.

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug, Display};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};
use tokio_util::sync::CancellationToken;

/// The name an actor goes by within its episode. Names are unique within
/// an episode and are how actors address one another.
pub type ActorId = String;

/// Anything actors can say to each other.
///
/// This is implemented automatically for every type that can be cloned,
/// printed for debugging, sent between threads, and serialized both ways.
/// Serialization is not used to pass messages within a process; it is
/// required so that what actors say can be logged, replayed, and carried
/// beyond the process. A plain data type with the serde derives satisfies
/// all of it.
pub trait Message: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static {}
impl<T: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static> Message for T {}

/// Where a request goes: a named actor, or `None` for the asking actor
/// itself. See [`Context::request`] for what a request to oneself means.
pub type Recipient = Option<ActorId>;

/// Ties an answer to the question it answers. Unique within an episode;
/// a question put to several recipients has one id.
pub type RequestId = u64;

/// One thing said on the wire, as recorded by a [`Log`].
///
/// A round trip is two events with the same [`request`](Event::request)
/// and the names swapped: the asker's [`Said::Asked`] and the recipient's
/// [`Said::Replied`]. The record is complete and uninterpreted: every
/// message, exactly as sent, stamped with the instant it was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event<M> {
    /// When this was recorded, by the wall clock.
    pub at: SystemTime,
    /// Which question this belongs to.
    pub request: RequestId,
    /// Who put it on the wire. For an unanswered request, who failed to.
    pub from: ActorId,
    /// Who it was for.
    pub to: ActorId,
    /// What was said.
    pub said: Said<M>,
}

/// What an [`Event`] records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Said<M> {
    /// A question, recorded as the asker sends it.
    Asked(M),
    /// An answer, recorded as the recipient sends it. An
    /// [`Unanswered`](Reply::Unanswered) reply is recorded instead by
    /// the asker, at the moment it gives up waiting.
    Replied(Reply<M>),
}

/// The record of everything said on the wire, as it is being written.
///
/// Make one with [`Log::new`] and hand it to [`episode`]. Every request
/// and every reply in the episode then arrives on the receiver as an
/// [`Event`], in the order recorded. Nothing waits on the receiver, so a
/// slow consumer costs memory rather than time. The events can drive a
/// display as they arrive or be written out, one JSON line each, as the
/// record of the episode.
#[derive(Debug)]
pub struct Log<M> {
    events: UnboundedSender<Event<M>>,
}

impl<M> Clone for Log<M> {
    fn clone(&self) -> Self {
        Log {
            events: self.events.clone(),
        }
    }
}

impl<M: Message> Log<M> {
    /// A log and the receiver its events arrive on.
    pub fn new() -> (Log<M>, UnboundedReceiver<Event<M>>) {
        let (events, received) = unbounded_channel();
        (Log { events }, received)
    }

    fn record(&self, request: RequestId, from: ActorId, to: ActorId, said: Said<M>) {
        let event = Event {
            at: SystemTime::now(),
            request,
            from,
            to,
            said,
        };
        // Nobody listening any more is not the actors' problem.
        let _ = self.events.send(event);
    }
}

/// The answers an asker is still waiting for: one channel per recipient.
type Pending<M> = Vec<(ActorId, oneshot::Receiver<Option<M>>)>;

/// One actor's question to another, as it waits in the recipient's inbox:
/// what was asked, who asked it, and the channel the answer goes back on.
///
/// A policy never handles one of these. The recipient's actor takes the
/// question out of its inbox and hands it to [`Policy::reply`] with
/// the asker's name, then sends whatever that returns down the channel,
/// where the asker's [`Context::request`] is waiting for it.
struct Request<M> {
    id: RequestId,
    from: ActorId,
    message: M,
    reply_to: oneshot::Sender<Option<M>>,
}

/// What came back from one recipient of a request.
///
/// A request to several recipients yields one of these per recipient. See
/// [`Context::request`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply<M> {
    /// The recipient answered with a message.
    Message(M),
    /// The recipient handled the request and had nothing to say: its
    /// [`Policy::reply`] returned `None`.
    Acknowledge,
    /// No answer came before the asker's patience ran out, or the
    /// recipient had already shut down.
    Unanswered,
}

/// The part of an actor that a step is allowed to touch: who it is, how
/// it reaches the other actors, and how it stops.
///
/// An actor is busy while one of its steps runs. Its policy is the code
/// taking the step, and its inbox is what the loop reads from between
/// steps, so neither can be handed to the step without letting it call
/// itself or steal the next request. What remains holds still while the
/// actor acts, and that is what a step gets.
pub struct Context<M> {
    id: ActorId,
    outbox: Outbox<M>,
    shutdown: CancellationToken,
    log: Option<Log<M>>,
    /// Shared by every actor in the episode, so ids are unique within it.
    next_request: Arc<AtomicU64>,
}

impl<M: Message> Context<M> {
    /// This actor's name, as the other actors know it.
    pub fn id(&self) -> &ActorId {
        &self.id
    }

    /// Shut this actor down.
    ///
    /// The step that calls this still runs to completion, and a reply it
    /// returns is still delivered. The actor then stops without taking in
    /// anything else waiting for it. In an episode, an actor shutting
    /// itself down affects no one else; the episode ends once every actor
    /// has done so.
    pub fn shutdown(&self) {
        self.shutdown.cancel()
    }

    /// Ask every recipient the same thing and await their answers.
    ///
    /// The request goes out as soon as this is called. Awaiting the result
    /// waits for the answers, one [`Reply`] per recipient, with `patience`
    /// bounding how long: a recipient that has not answered by then, or
    /// that had already shut down, is [`Reply::Unanswered`]. With no
    /// patience the wait is unbounded, except by the actor being shut
    /// down.
    ///
    /// The future borrows nothing from the caller, so it can be held, sent
    /// elsewhere, or dropped. Dropping it sends the request and forgets
    /// about the answers. That is how an actor leaves a note for itself: a request
    /// to `None` lands in its own inbox, to be handled in a later step.
    /// Awaiting such a request can never succeed, since the actor cannot
    /// take a step while it is waiting for one to finish. See the module
    /// documentation on [deadlock](crate#deadlock).
    ///
    /// # Errors
    ///
    /// Awaiting the result fails if any recipient is one this actor cannot
    /// reach, either because no actor of that name is in the episode or
    /// because the topology does not allow the link. Nothing is sent to
    /// anyone in that case.
    pub fn request(
        &self,
        to: &HashSet<Recipient>,
        message: M,
        patience: Option<Duration>,
    ) -> impl Future<Output = Result<Vec<(ActorId, Reply<M>)>>> + Send + 'static + use<M> {
        let dispatched = self.dispatch(to, message);
        let deadline = patience.map(|patience| Instant::now() + patience);
        let asker = self.id.clone();
        let log = self.log.clone();
        async move {
            let (id, pending) = dispatched?;
            let mut answers = Vec::new();
            for (from, reply) in pending {
                let answer = match deadline {
                    Some(deadline) => timeout_at(deadline, reply).await.ok(),
                    None => Some(reply.await),
                };
                let answer = match answer {
                    Some(Ok(Some(message))) => Reply::Message(message),
                    Some(Ok(None)) => Reply::Acknowledge,
                    _ => Reply::Unanswered,
                };
                if matches!(answer, Reply::Unanswered) {
                    // Nothing crossed the wire, so only the asker can record it.
                    if let Some(log) = &log {
                        log.record(
                            id,
                            from.clone(),
                            asker.clone(),
                            Said::Replied(Reply::Unanswered),
                        );
                    }
                }
                answers.push((from, answer));
            }
            Ok(answers)
        }
    }

    /// Send one request per recipient, returning the request's id and the
    /// channel each answer will arrive on.
    fn dispatch(&self, to: &HashSet<Recipient>, message: M) -> Result<(RequestId, Pending<M>)> {
        let unknown: Vec<_> = to
            .iter()
            .filter(|to| self.outbox.sender(to).is_none())
            .map(|to| self.name(to))
            .collect();
        if !unknown.is_empty() {
            bail!("{} cannot reach {}", self.id, unknown.join(", "));
        }

        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let mut pending = Vec::with_capacity(to.len());
        for to in to {
            let (reply_to, reply) = oneshot::channel();
            let request = Request {
                id,
                from: self.id.clone(),
                message: message.clone(),
                reply_to,
            };
            self.record(id, self.name(to), Said::Asked(message.clone()));
            // A send fails only when the recipient has shut down, which the
            // asker learns of as an unanswered request.
            let _ = self.outbox.sender(to).expect("checked above").send(request);
            pending.push((self.name(to), reply));
        }
        Ok((id, pending))
    }

    fn name(&self, to: &Recipient) -> ActorId {
        to.clone().unwrap_or_else(|| self.id.clone())
    }

    /// Record something this actor put on the wire, if anyone is listening.
    fn record(&self, request: RequestId, to: ActorId, said: Said<M>) {
        if let Some(log) = &self.log {
            log.record(request, self.id.clone(), to, said);
        }
    }
}

/// The channels through which an actor reaches every actor it may address,
/// itself included.
struct Outbox<M> {
    /// Reaches the actor's own inbox. An actor may be alone in the
    /// universe and still have things to say to itself, so this keeps the
    /// inbox open no matter who else is around.
    loopback: UnboundedSender<Request<M>>,
    peers: HashMap<ActorId, UnboundedSender<Request<M>>>,
}

impl<M: Message> Outbox<M> {
    fn new(
        loopback: UnboundedSender<Request<M>>,
        peers: HashMap<ActorId, UnboundedSender<Request<M>>>,
    ) -> Self {
        Outbox { loopback, peers }
    }

    fn sender(&self, to: &Recipient) -> Option<&UnboundedSender<Request<M>>> {
        match to {
            Some(to) => self.peers.get(to),
            None => Some(&self.loopback),
        }
    }
}

/// What an actor does with the requests it receives.
///
/// Implementations put `#[async_trait]` on their impl block and write the
/// methods as `async fn`. Every method receives a [`Context`], through
/// which the policy can ask other actors things and shut its own actor
/// down.
///
/// Actors only ever communicate by request: one asks through
/// [`Context::request`] and awaits the answers, the other answers through
/// [`reply`](Policy::reply). Each actor takes one step at a time,
/// so an actor that is waiting for answers is not answering anyone else
/// meanwhile.
///
/// # Examples
///
/// A reactive policy that answers every question the same way.
///
/// ```
/// use anyhow::Result;
/// use async_trait::async_trait;
/// use free_agent::{ActorId, Context, Policy};
///
/// struct YesMan;
///
/// #[async_trait]
/// impl Policy for YesMan {
///     type Message = String;
///
///     async fn reply(
///         &mut self,
///         _from: ActorId,
///         _message: String,
///         _context: &Context<String>,
///     ) -> Result<Option<String>> {
///         Ok(Some("yes".to_string()))
///     }
/// }
/// ```
#[async_trait]
pub trait Policy {
    /// The type of message this policy sends and receives. Every actor in
    /// an episode shares one message type.
    type Message: Message;

    /// Make an opening move once the actor is running, before it has heard
    /// anything.
    ///
    /// Most policies are reactive and do nothing, which is the default. In
    /// a setup with one environment and many agents, the environment's
    /// opening move is what sets everyone else going.
    ///
    /// # Errors
    ///
    /// An error ends this actor and, within an episode, the episode.
    async fn start(&mut self, _context: &Context<Self::Message>) -> Result<()> {
        Ok(())
    }

    /// Answer a request from the actor named `from`.
    ///
    /// Returning `Some` sends that message back as the answer. Returning
    /// `None` acknowledges the request without saying anything; the asker
    /// sees [`Reply::Acknowledge`].
    ///
    /// # Errors
    ///
    /// An error ends this actor and, within an episode, the episode.
    async fn reply(
        &mut self,
        from: ActorId,
        message: Self::Message,
        context: &Context<Self::Message>,
    ) -> Result<Option<Self::Message>>;
}

/// A boxed policy is a policy, so an episode can mix policies of different
/// types behind `Box<dyn Policy<Message = M> + Send>`.
#[async_trait]
impl<M: Message> Policy for Box<dyn Policy<Message = M> + Send> {
    type Message = M;

    async fn start(&mut self, context: &Context<M>) -> Result<()> {
        (**self).start(context).await
    }

    async fn reply(
        &mut self,
        from: ActorId,
        request: M,
        context: &Context<M>,
    ) -> Result<Option<M>> {
        (**self).reply(from, request, context).await
    }
}

/// The thing that acts: a policy driven by its inbox, with a context
/// through which it reaches the world.
///
/// The context is not a separate thing from the actor. It is the actor as
/// its own policy is allowed to see it, kept apart only so that a running
/// step cannot reach the machinery that is running it.
struct Actor<P: Policy> {
    policy: P,
    inbox: UnboundedReceiver<Request<P::Message>>,
    context: Context<P::Message>,
}

impl<P: Policy + Send> Actor<P> {
    fn new(
        id: ActorId,
        policy: P,
        inbox: UnboundedReceiver<Request<P::Message>>,
        outbox: Outbox<P::Message>,
        shutdown: CancellationToken,
        log: Option<Log<P::Message>>,
        next_request: Arc<AtomicU64>,
    ) -> Self {
        let context = Context {
            id,
            outbox,
            shutdown,
            log,
            next_request,
        };
        Actor {
            policy,
            inbox,
            context,
        }
    }

    /// Make the policy's opening move, then hand it every request as it
    /// comes, until the shutdown token fires or a step fails.
    ///
    /// Shutdown takes effect at once, before any request still waiting in
    /// the inbox, except that a step which has already completed gets its
    /// reply delivered.
    async fn run(mut self) -> Result<()> {
        let shutdown = self.context.shutdown.clone();
        let Some(opened) = unless_stopped(&shutdown, self.policy.start(&self.context)).await else {
            return Ok(());
        };
        opened?;
        loop {
            let request = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return Ok(()),
                request = self.inbox.recv() => request,
            };
            let Some(Request {
                id,
                from,
                message,
                reply_to,
            }) = request
            else {
                return Ok(());
            };
            let step = self.policy.reply(from.clone(), message, &self.context);
            let Some(outcome) = unless_stopped(&shutdown, step).await else {
                return Ok(());
            };
            let answer = outcome?;
            let said = match &answer {
                Some(message) => Reply::Message(message.clone()),
                None => Reply::Acknowledge,
            };
            self.context.record(id, from, Said::Replied(said));
            // The asker may have given up waiting.
            let _ = reply_to.send(answer);
        }
    }
}

/// Run a step, unless the shutdown token fires first.
async fn unless_stopped<T>(
    shutdown: &CancellationToken,
    step: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => None,
        outcome = step => Some(outcome),
    }
}

/// Who may talk to whom: each actor maps to the set of actors it can
/// address.
///
/// Links are one-way. An actor absent from the map can address no one but
/// itself, which every actor can always do. Passing no topology to
/// [`episode`] lets everybody talk to everybody.
pub type Topology = HashMap<ActorId, HashSet<ActorId>>;

/// How an [`episode`] came to an end.
#[derive(Debug, PartialEq, Eq)]
pub enum Ending {
    /// Every actor shut itself down.
    Finished,
    /// The time limit passed, and the actors still running were shut
    /// down.
    TimedOut,
}

/// Run a set of actors together until every one of them has shut itself
/// down or the time limit passes, whichever comes first.
///
/// Each pair names an actor and gives it a policy. Every actor is created
/// and wired up before any of them runs, so an opening move can reach
/// every actor its topology allows, whether or not that actor has started
/// yet. With no topology, everybody can talk to everybody.
///
/// Policies of different types can share an episode by boxing them as
/// `Box<dyn Policy<Message = M> + Send>`, as the module example does.
///
/// With a [`Log`], every request and reply in the episode is also sent
/// to the log's receiver as an [`Event`]; without one, nothing is
/// recorded.
///
/// # Errors
///
/// Fails before any actor runs if two actors share a name or the topology
/// names an actor that is not in the episode. Fails after running if any
/// actor's step returned an error: the first failure shuts the remaining
/// actors down, and the error names every actor that failed and why.
pub async fn episode<P: Policy + Send + 'static>(
    policies: impl IntoIterator<Item = (ActorId, P)>,
    topology: Option<Topology>,
    time_limit: Duration,
    log: Option<Log<P::Message>>,
) -> Result<Ending> {
    let mut senders = HashMap::new();
    let mut inboxes = HashMap::new();
    let mut policies_by_id = Vec::new();
    for (id, policy) in policies {
        let (sender, inbox) = unbounded_channel();
        if senders.insert(id.clone(), sender).is_some() {
            bail!("two actors are named {id}");
        }
        inboxes.insert(id.clone(), inbox);
        policies_by_id.push((id, policy));
    }
    let mut named = topology
        .iter()
        .flatten()
        .flat_map(|(from, tos)| std::iter::once(from).chain(tos));
    if let Some(unknown) = named.find(|id| !senders.contains_key(*id)) {
        bail!("the topology names an actor {unknown} that is not in the episode");
    }

    let shutdown = CancellationToken::new();
    let next_request = Arc::new(AtomicU64::new(0));
    let mut actors = JoinSet::new();
    for (id, policy) in policies_by_id {
        let peers = match &topology {
            None => senders.clone(),
            Some(topology) => topology
                .get(&id)
                .into_iter()
                .flatten()
                .map(|to| (to.clone(), senders[to].clone()))
                .collect(),
        };
        let outbox = Outbox::new(senders[&id].clone(), peers);
        let inbox = inboxes.remove(&id).expect("every actor has an inbox");
        // A child token lets the actor stop itself without stopping the
        // others, while the episode's own token still stops everyone.
        let actor = Actor::new(
            id.clone(),
            policy,
            inbox,
            outbox,
            shutdown.child_token(),
            log.clone(),
            next_request.clone(),
        );
        actors.spawn(async move { (id, actor.run().await) });
    }

    let ending = match timeout(time_limit, wait_for_all(&mut actors, &shutdown)).await {
        Ok(failures) => (Ending::Finished, failures),
        Err(_) => {
            shutdown.cancel();
            (Ending::TimedOut, wait_for_all(&mut actors, &shutdown).await)
        }
    };
    match ending {
        (ending, failures) if failures.is_empty() => Ok(ending),
        (_, failures) => Err(Failed(failures).into()),
    }
}

/// Wait for every actor to stop, collecting the failures. The first
/// failure shuts the remaining actors down, since an episode with a broken
/// actor in it cannot be trusted to finish on its own.
async fn wait_for_all(
    actors: &mut JoinSet<(ActorId, Result<()>)>,
    shutdown: &CancellationToken,
) -> Vec<(ActorId, anyhow::Error)> {
    let mut failures = Vec::new();
    while let Some(outcome) = actors.join_next().await {
        let failure = match outcome {
            Ok((_, Ok(()))) => continue,
            Ok((id, Err(error))) => (id, error),
            Err(panicked) => ("?".to_string(), panicked.into()),
        };
        failures.push(failure);
        shutdown.cancel();
    }
    failures
}

/// The actors whose runs ended in an error, and why.
#[derive(Debug)]
struct Failed(Vec<(ActorId, anyhow::Error)>);

impl Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} actor(s) failed", self.0.len())?;
        for (id, error) in &self.0 {
            write!(f, "\n  {id}: {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failed {}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::{Instant, sleep};

    type Replies = Vec<(ActorId, Reply<String>)>;

    /// A policy made of one answering function, for actors that only ever
    /// answer.
    struct Answerer<F>(F);

    #[async_trait]
    impl<F, Fut> Policy for Answerer<F>
    where
        F: FnMut(ActorId, String) -> Fut + Send,
        Fut: Future<Output = Option<String>> + Send,
    {
        type Message = String;

        async fn reply(
            &mut self,
            from: ActorId,
            message: String,
            _context: &Context<String>,
        ) -> Result<Option<String>> {
            Ok(self.0(from, message).await)
        }
    }

    /// Answers every request by saying who asked and what.
    fn echo() -> impl Policy<Message = String> + Send {
        Answerer(
            |from: ActorId, message: String| async move { Some(format!("{from} said {message}")) },
        )
    }

    /// Handles every request without a word.
    fn mute() -> impl Policy<Message = String> + Send {
        Answerer(|_: ActorId, _: String| async { None })
    }

    /// Takes a while over every request before echoing it back.
    fn slow(delay: Duration) -> impl Policy<Message = String> + Send {
        Answerer(move |_: ActorId, message: String| async move {
            sleep(delay).await;
            Some(message)
        })
    }

    /// Never finishes handling a request.
    fn ponderer() -> impl Policy<Message = String> + Send {
        Answerer(|_: ActorId, _: String| std::future::pending::<Option<String>>())
    }

    /// Asks a set of actors "hello" on startup, reports whatever comes back
    /// to the test, and quits. Answers any request with an exclamation.
    struct Asker {
        to: HashSet<Recipient>,
        patience: Option<Duration>,
        report: UnboundedSender<Replies>,
    }

    #[async_trait]
    impl Policy for Asker {
        type Message = String;

        async fn start(&mut self, context: &Context<String>) -> Result<()> {
            let replies = context
                .request(&self.to, "hello".to_string(), self.patience)
                .await?;
            self.report.send(replies)?;
            context.shutdown();
            Ok(())
        }

        async fn reply(
            &mut self,
            _from: ActorId,
            message: String,
            _context: &Context<String>,
        ) -> Result<Option<String>> {
            Ok(Some(message + "!"))
        }
    }

    /// An asker of the given recipients, plus the channel its report
    /// arrives on.
    fn asking(
        to: HashSet<Recipient>,
        patience: Option<Duration>,
    ) -> (Asker, UnboundedReceiver<Replies>) {
        let (report, reported) = unbounded_channel();
        let asker = Asker {
            to,
            patience,
            report,
        };
        (asker, reported)
    }

    /// An asker of the named actors.
    fn asker<const N: usize>(
        to: [&str; N],
        patience: Option<Duration>,
    ) -> (Asker, UnboundedReceiver<Replies>) {
        asking(recipients(to), patience)
    }

    fn recipients<const N: usize>(names: [&str; N]) -> HashSet<Recipient> {
        names.map(|name| Some(name.to_string())).into()
    }

    /// A cast of named actors with policies of different types, boxed so
    /// they can share an episode.
    macro_rules! cast {
        ($(($name:expr, $policy:expr)),* $(,)?) => {
            vec![$((
                $name.to_string(),
                Box::new($policy) as Box<dyn Policy<Message = _> + Send>,
            )),*]
        };
    }

    async fn sorted(reported: &mut UnboundedReceiver<Replies>) -> Replies {
        let mut replies = reported.recv().await.expect("the asker reports once");
        replies.sort_by(|a, b| a.0.cmp(&b.0));
        replies
    }

    fn said(by: &str, what: &str) -> (ActorId, Reply<String>) {
        (by.to_string(), Reply::Message(what.to_string()))
    }

    const PATIENCE: Duration = Duration::from_secs(60);

    #[tokio::test(start_paused = true)]
    async fn request_brings_back_each_recipients_reply() {
        let (asker, mut reported) = asker(["echo", "mute"], None);
        let actors = cast![("asker", asker), ("echo", echo()), ("mute", mute())];

        episode(actors, None, PATIENCE, None).await.unwrap();

        assert_eq!(
            sorted(&mut reported).await,
            vec![
                said("echo", "asker said hello"),
                ("mute".to_string(), Reply::Acknowledge),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn request_waits_as_long_as_it_takes_without_a_patience() {
        let (asker, mut reported) = asker(["slow"], None);
        let actors = cast![("asker", asker), ("slow", slow(Duration::from_secs(10)))];
        let began = Instant::now();

        episode(actors, None, PATIENCE, None).await.unwrap();

        assert_eq!(sorted(&mut reported).await, vec![said("slow", "hello")]);
        assert!(began.elapsed() >= Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn request_marks_a_recipient_unanswered_when_patience_runs_out() {
        let (asker, mut reported) = asker(["slow", "echo"], Some(Duration::from_secs(5)));
        let actors = cast![
            ("asker", asker),
            ("slow", slow(Duration::from_secs(10))),
            ("echo", echo()),
        ];

        episode(actors, None, PATIENCE, None).await.unwrap();

        assert_eq!(
            sorted(&mut reported).await,
            vec![
                said("echo", "asker said hello"),
                ("slow".to_string(), Reply::Unanswered),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn request_fails_for_an_unknown_recipient() {
        let (asker, _reported) = asker(["ghost"], None);
        let actors = [("asker".to_string(), asker)];

        let error = episode(actors, None, PATIENCE, None).await.unwrap_err();

        let report = error.to_string();
        assert!(
            report.contains("asker") && report.contains("ghost"),
            "{report}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_to_self_cannot_be_answered_while_the_asker_waits_for_it() {
        let (asker, mut reported) = asking(HashSet::from([None]), Some(Duration::from_secs(5)));
        let actors = [("asker".to_string(), asker)];

        episode(actors, None, PATIENCE, None).await.unwrap();

        assert_eq!(
            sorted(&mut reported).await,
            vec![("asker".to_string(), Reply::Unanswered)]
        );
    }

    /// Sends itself a note on startup without waiting for an answer, and
    /// reports what it hears to the test.
    struct Scheduler {
        heard: UnboundedSender<(ActorId, String)>,
    }

    #[async_trait]
    impl Policy for Scheduler {
        type Message = String;

        async fn start(&mut self, context: &Context<String>) -> Result<()> {
            // The request is sent as soon as it is made; the dropped future
            // only means nobody waits for the answer.
            drop(context.request(&HashSet::from([None]), "later".to_string(), None));
            Ok(())
        }

        async fn reply(
            &mut self,
            from: ActorId,
            message: String,
            context: &Context<String>,
        ) -> Result<Option<String>> {
            self.heard.send((from, message))?;
            context.shutdown();
            Ok(None)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_to_self_without_waiting_is_handled_in_a_later_step() {
        let (heard, mut hearing) = unbounded_channel();
        let actors = [("scheduler".to_string(), Scheduler { heard })];

        let ending = episode(actors, None, PATIENCE, None).await.unwrap();

        assert_eq!(ending, Ending::Finished);
        assert_eq!(
            hearing.recv().await,
            Some(("scheduler".to_string(), "later".to_string()))
        );
    }

    /// Serves a counter to one other actor and keeps returning whatever
    /// comes back until it reaches the limit, then quits.
    struct Server {
        other: ActorId,
        limit: u32,
    }

    #[async_trait]
    impl Policy for Server {
        type Message = u32;

        async fn start(&mut self, context: &Context<u32>) -> Result<()> {
            let mut count = 0;
            while count < self.limit {
                let replies = context
                    .request(&recipients([&self.other]), count, None)
                    .await?;
                match replies.as_slice() {
                    [(_, Reply::Message(next))] => count = *next,
                    other => bail!("the receiver stopped answering: {other:?}"),
                }
            }
            context.shutdown();
            Ok(())
        }

        async fn reply(
            &mut self,
            _from: ActorId,
            _count: u32,
            _context: &Context<u32>,
        ) -> Result<Option<u32>> {
            Ok(None)
        }
    }

    /// Answers every count with the next one, and quits once the next one
    /// reaches the limit.
    struct Receiver {
        limit: u32,
    }

    #[async_trait]
    impl Policy for Receiver {
        type Message = u32;

        async fn reply(
            &mut self,
            _from: ActorId,
            count: u32,
            context: &Context<u32>,
        ) -> Result<Option<u32>> {
            let next = count + 1;
            if next >= self.limit {
                context.shutdown();
            }
            Ok(Some(next))
        }
    }

    /// A server and a receiver volleying a counter up to the limit.
    fn volley(limit: u32) -> Vec<(ActorId, Box<dyn Policy<Message = u32> + Send>)> {
        let server = Server {
            other: "receiver".to_string(),
            limit,
        };
        cast![("server", server), ("receiver", Receiver { limit })]
    }

    #[tokio::test(start_paused = true)]
    async fn episode_ends_once_every_actor_has_shut_itself_down() {
        let began = Instant::now();

        let ending = episode(volley(3), None, PATIENCE, None).await.unwrap();

        assert_eq!(ending, Ending::Finished);
        assert!(began.elapsed() < PATIENCE);
    }

    #[tokio::test(start_paused = true)]
    async fn episode_shuts_everyone_down_at_the_timeout_even_mid_step() {
        let (asker, _reported) = asker(["ponderer"], None);
        let actors = cast![("asker", asker), ("ponderer", ponderer())];
        let began = Instant::now();

        let ending = episode(actors, None, PATIENCE, None).await.unwrap();

        assert_eq!(ending, Ending::TimedOut);
        assert_eq!(began.elapsed(), PATIENCE);
    }

    #[tokio::test(start_paused = true)]
    async fn episode_only_wires_the_links_the_topology_allows() {
        let (asker, _reported) = asker(["echo"], None);
        let actors = cast![("asker", asker), ("echo", echo())];
        // Only echo may speak, and only to the asker; the asker may reach no one.
        let topology = HashMap::from([("echo".to_string(), HashSet::from(["asker".to_string()]))]);

        let error = episode(actors, Some(topology), PATIENCE, None)
            .await
            .unwrap_err();

        let report = error.to_string();
        assert!(
            report.contains("asker") && report.contains("echo"),
            "{report}"
        );
    }

    /// Panics as soon as it starts.
    struct Crasher;

    #[async_trait]
    impl Policy for Crasher {
        type Message = String;

        async fn start(&mut self, _context: &Context<String>) -> Result<()> {
            panic!("the crasher crashed")
        }

        async fn reply(
            &mut self,
            _from: ActorId,
            _message: String,
            _context: &Context<String>,
        ) -> Result<Option<String>> {
            Ok(None)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn episode_reports_an_actor_that_panics() {
        let actors = cast![("crasher", Crasher), ("echo", echo())];

        let error = episode(actors, None, PATIENCE, None).await.unwrap_err();

        assert!(error.to_string().contains("panicked"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn episode_rejects_a_topology_that_names_unknown_actors() {
        let actors = [("a".to_string(), mute())];
        let topology = HashMap::from([("a".to_string(), HashSet::from(["ghost".to_string()]))]);

        let error = episode(actors, Some(topology), PATIENCE, None)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("ghost"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn episode_rejects_duplicate_actor_ids() {
        let actors = [("a".to_string(), mute()), ("a".to_string(), mute())];

        let error = episode(actors, None, PATIENCE, None).await.unwrap_err();

        assert!(error.to_string().contains("a"), "{error}");
    }

    /// An event stripped of its timestamp.
    type Recorded<M> = (RequestId, ActorId, ActorId, Said<M>);

    /// Run an episode with a log attached and return everything recorded, in
    /// the order recorded.
    async fn recorded<P: Policy + Send + 'static>(
        actors: impl IntoIterator<Item = (ActorId, P)>,
    ) -> Vec<Recorded<P::Message>> {
        let (log, mut events) = Log::new();
        episode(actors, None, PATIENCE, Some(log)).await.unwrap();
        let mut record = Vec::new();
        while let Ok(event) = events.try_recv() {
            record.push((event.request, event.from, event.to, event.said));
        }
        record
    }

    fn asked(id: RequestId, from: &str, to: &str, what: &str) -> Recorded<String> {
        (
            id,
            from.to_string(),
            to.to_string(),
            Said::Asked(what.to_string()),
        )
    }

    fn replied(id: RequestId, from: &str, to: &str, reply: Reply<String>) -> Recorded<String> {
        (id, from.to_string(), to.to_string(), Said::Replied(reply))
    }

    #[tokio::test(start_paused = true)]
    async fn the_tap_records_every_request_and_every_reply() {
        let (asker, _reported) = asker(["echo", "mute"], None);
        let actors = cast![("asker", asker), ("echo", echo()), ("mute", mute())];

        let mut record = recorded(actors).await;

        record.sort_by(|a, b| (a.0, &a.1, &a.2).cmp(&(b.0, &b.1, &b.2)));
        let id = record[0].0;
        assert_eq!(
            record,
            vec![
                asked(id, "asker", "echo", "hello"),
                asked(id, "asker", "mute", "hello"),
                replied(
                    id,
                    "echo",
                    "asker",
                    Reply::Message("asker said hello".to_string())
                ),
                replied(id, "mute", "asker", Reply::Acknowledge),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_tap_records_a_question_before_its_answer_and_each_question_once() {
        let record = recorded(volley(3)).await;

        let ids: Vec<_> = record.iter().map(|event| event.0).collect();
        assert_eq!(
            ids,
            vec![ids[0], ids[0], ids[2], ids[2], ids[4], ids[4]],
            "{record:?}"
        );
        assert!(ids[0] < ids[2] && ids[2] < ids[4]);
        for pair in record.chunks(2) {
            assert!(matches!(pair[0].3, Said::Asked(_)), "{pair:?}");
            assert!(
                matches!(pair[1].3, Said::Replied(Reply::Message(_))),
                "{pair:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_tap_records_an_unanswered_request_from_the_askers_side() {
        let (asker, _reported) = asker(["slow"], Some(Duration::from_secs(5)));
        let actors = cast![("asker", asker), ("slow", slow(Duration::from_secs(10)))];

        let record = recorded(actors).await;

        let unanswered = replied(record[0].0, "slow", "asker", Reply::Unanswered);
        assert!(record.contains(&unanswered), "{record:?}");
    }

    #[test]
    fn an_event_round_trips_through_json() {
        let event = Event {
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            request: 7,
            from: "asker".to_string(),
            to: "echo".to_string(),
            said: Said::Asked("hello".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let back: Event<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(back, event);
        assert!(json.contains("\"request\":7"), "{json}");
    }
}
