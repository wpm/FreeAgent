//! Actors in the sense of the [actor model]: each one keeps its own state,
//! takes messages from a mailbox one at a time, and reaches other actors
//! only by sending them messages.
//!
//! An actor perceives, thinks, and acts. It perceives in its message loop:
//! a [`Behavior`] driven by a mailbox handles each message as it comes. It
//! thinks in an optional second loop, a [`Think`] in a task of its own,
//! which takes the messages handed to it by [`Context::think`] and the
//! timers set by [`Context::think_after`] one at a time, so that slow work
//! such as a model call never holds up perceiving. It acts through every
//! message either loop sends. Each loop reaches the rest of the episode
//! through a [`Context`] of its own, which an [`Episode`](crate::Episode)
//! builds from the actor's [`ActorInit`].
//!
//! [actor model]: https://en.wikipedia.org/wiki/Actor_model

use crate::log::{Event, Logger};
use crate::message::Message;
use anyhow::{Context as _, bail};
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet};
use std::future::poll_fn;
use std::ops::ControlFlow::{self, Break, Continue};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::time::DelayQueue;
use uuid::Uuid;

/// An actor's name, unique within an episode.
pub type ActorId = String;

/// Builds a [`Behavior`] from the [`Context`] it will own.
pub type Builder<B> =
    Box<dyn FnOnce(Context<<B as Behavior>::Message, <B as Behavior>::Log>) -> B + Send>;

/// Builds a think loop's [`Think`] from the [`Context`] it will own.
pub type ThinkBuilder<M, L> = Box<dyn FnOnce(Context<M, L>) -> Box<dyn Think<Message = M>> + Send>;

/// Everything an [`Episode`](crate::Episode) needs to build one actor: how
/// it behaves, how it thinks, and whom it may reach.
pub struct ActorInit<B: Behavior> {
    /// Builds the behavior once its [`Context`] exists, which is when the
    /// episode has opened every actor's channels.
    pub behavior: Builder<B>,
    /// Builds the actor's think loop, if it has one, from a [`Context`] of
    /// its own.
    pub think: Option<ThinkBuilder<B::Message, B::Log>>,
    /// The actors this one may send to.
    pub can_send_to: HashSet<ActorId>,
    /// The actors this one may stop.
    pub can_shut_down: HashSet<ActorId>,
    /// Whether this actor gets a copy of the episode's logger.
    pub has_logger: bool,
}

/// A running actor: its behavior, its think loop if it has one, its
/// mailbox, and the two one-time signals it exchanges with the episode on
/// the way up.
pub(crate) struct Actor<B: Behavior> {
    /// What this Actor does at each point in its life. It owns the
    /// [`Context`] through which it reaches other actors.
    pub(crate) behavior: B,
    /// What this Actor thinks about, with the queue and the timers it takes
    /// it from. Taken when the loop is spawned.
    pub(crate) think: Option<ThinkLoop<B::Message>>,
    /// This Actor's one-time signal to the episode that it is initialized
    /// and running its message loop. Taken when it is sent.
    pub(crate) ready: Option<oneshot::Sender<()>>,
    /// The episode's one-time signal that every actor is running.
    pub(crate) start: oneshot::Receiver<()>,
    /// The channel on which this Actor receives incoming Messages.
    pub(crate) mailbox: UnboundedReceiver<B::Message>,
}

impl<B: Behavior> Actor<B> {
    /// An actor's whole life, in five phases.
    ///
    /// 1. Initialize, and tell the episode this actor is ready.
    /// 2. Spawn the think loop, if there is one, in a task of its own.
    /// 3. Wait for the episode's start signal, which comes once every actor
    ///    is ready, and make the opening move.
    /// 4. Deliver each message in the mailbox to the behavior, one at a
    ///    time, until shut down.
    /// 5. Wait for the think loop to end, then clean up.
    ///
    /// Phases 3 and 4 overlap: mail that arrives before the start signal is
    /// delivered as it comes. The mailbox closes once every actor that may
    /// send to this one has stopped, and from then on the actor only waits
    /// to be shut down. An error from the behavior at any phase ends the
    /// actor with that error. A panic in the think loop ends only that
    /// loop, and is the actor's error once the actor stops.
    ///
    /// Shutdown cuts any phase short. A step or a thought in progress is
    /// dropped at its next await, mail still in the mailbox and messages
    /// still on the think queue stay there, pending timers are dropped, and
    /// cleanup runs only after a finished initialization.
    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        let shutdown = self.behavior.context().shutdown.mine.clone();
        let Some(initialized) = run_unless_stopped(&shutdown, self.behavior.initialize()).await
        else {
            return Ok(());
        };
        initialized?;
        // The episode may already be gone, in which case no one is waiting.
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(());
        }
        let thinking = self
            .think
            .take()
            .map(|think| tokio::spawn(think.run(shutdown.clone())));
        let perceived = self.perceive(&shutdown).await;
        if let Some(thinking) = thinking {
            // Whatever ended perceiving ends thinking, and the think loop
            // is gone before anything is cleaned up from under it.
            shutdown.cancel();
            thinking.await.context("the think loop panicked")?;
        }
        perceived?;
        self.behavior.clean_up().await
    }

    /// Wait for the start signal and the mail, and hand each to the
    /// behavior, until shut down or the episode is gone before the start.
    async fn perceive(&mut self, shutdown: &CancellationToken) -> anyhow::Result<()> {
        let mut start_consumed = false;
        loop {
            // Wait for whichever happens first: shutdown, the start signal,
            // or the next message. Each arm is `pattern = future => body`;
            // the body runs with the future's output bound to the pattern.
            // `biased` tries the arms in order, so shutdown wins a tie. The
            // start arm drops out once it has fired, because a oneshot
            // receiver panics if polled again. The mailbox arm drops out
            // when the mailbox yields `None`, which it does once it has
            // closed because every actor that may send to this one has
            // stopped.
            let flow = tokio::select! {
                biased;
                _ = shutdown.cancelled() => Break(()),
                signal = &mut self.start, if !start_consumed => {
                    start_consumed = true;
                    self.open(signal).await?
                }
                Some(message) = self.mailbox.recv() => self.deliver(message).await?,
            };
            if flow.is_break() {
                break;
            }
        }
        Ok(())
    }

    /// Make the opening move, now that the episode has signaled that every
    /// actor is running. An error in place of the signal means the episode
    /// is gone, and there is nothing to open.
    async fn open(
        &mut self,
        signal: Result<(), oneshot::error::RecvError>,
    ) -> anyhow::Result<ControlFlow<()>> {
        if signal.is_err() {
            return Ok(Break(()));
        }
        let shutdown = self.behavior.context().shutdown.mine.clone();
        let Some(opened) = run_unless_stopped(&shutdown, self.behavior.start()).await else {
            return Ok(Break(()));
        };
        opened?;
        Ok(Continue(()))
    }

    /// Hand `message` to the behavior.
    ///
    /// The message loop watches for shutdown only between messages, so each
    /// step races shutdown on its own here.
    async fn deliver(&mut self, message: B::Message) -> anyhow::Result<ControlFlow<()>> {
        let shutdown = self.behavior.context().shutdown.mine.clone();
        let Some(received) = run_unless_stopped(&shutdown, self.behavior.receive(&message)).await
        else {
            return Ok(Break(()));
        };
        received?;
        Ok(Continue(()))
    }
}

/// An actor's think loop: what thinks, the queue it thinks from, and the
/// channel its timers arrive on.
pub(crate) struct ThinkLoop<M: Message> {
    /// What this loop thinks. It owns the loop's [`Context`].
    pub(crate) think: Box<dyn Think<Message = M>>,
    /// The channel on which this loop receives what to think about.
    pub(crate) queue: UnboundedReceiver<M>,
    /// The channel on which this loop receives timers to set: a deadline
    /// and the message to think about once it has passed.
    pub(crate) timers: UnboundedReceiver<(Instant, M)>,
}

impl<M: Message> ThinkLoop<M> {
    /// Open a think queue and a timer channel into `context`, and build the
    /// loop from `build` with a twin of it.
    pub(crate) fn open<L>(build: ThinkBuilder<M, L>, context: &mut Context<M, L>) -> Self {
        let (now, queue) = unbounded_channel();
        let (later, timers) = unbounded_channel();
        context.thoughts = Some(Thoughts { now, later });
        Self {
            think: build(context.twin()),
            queue,
            timers,
        }
    }

    /// Think about each message in turn, one at a time, until `shutdown`
    /// fires: a timer as soon as it is due, ahead of whatever is waiting on
    /// the queue, and otherwise the queue in the order it arrived. Nothing
    /// interrupts a thought in progress. An error from a thought is dropped
    /// and the loop goes on to the next message. Shutdown drops a thought in
    /// progress at its next await, and leaves the rest of the queue and
    /// every pending timer unhandled.
    async fn run(mut self, shutdown: CancellationToken) {
        let mut pending = DelayQueue::new();
        loop {
            // Wait for whichever happens first, trying the arms in order.
            // A new timer joins the pending ones, and the wait goes on. The
            // pending timers are polled only while there are some, because
            // an empty `DelayQueue` reports that it is finished rather than
            // pending.
            let message = tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                Some((deadline, message)) = self.timers.recv() => {
                    pending.insert_at(message, deadline);
                    continue;
                }
                Some(due) = poll_fn(|cx| pending.poll_expired(cx)), if !pending.is_empty() => {
                    due.into_inner()
                }
                Some(message) = self.queue.recv() => message,
            };
            let _ = run_unless_stopped(&shutdown, self.think.think(&message)).await;
        }
    }
}

/// Run `step` to completion, unless `shutdown` fires first, in which case
/// the step is dropped where it stands.
async fn run_unless_stopped<T>(
    shutdown: &CancellationToken,
    step: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => None,
        outcome = step => Some(outcome),
    }
}

/// The ways out of an actor: what a [`Behavior`] may do besides receive.
///
/// A behavior owns one of these, and the actor keeps the mailbox, so messages
/// reach the behavior one step at a time.
///
/// `L` is what this actor logs. It defaults to the message type, which is
/// what most actors log.
pub struct Context<M: Message, L = M> {
    /// This actor's name, as the other actors know it.
    pub id: ActorId,
    /// The episode this actor is in, stamped on everything it logs.
    pub episode: Uuid,
    /// The sending ends of the mailboxes of the actors this one may send to.
    pub(crate) mailboxes: HashMap<ActorId, UnboundedSender<M>>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    pub(crate) shutdown: Shutdown,
    /// Where this Actor's events go, when it has a logger.
    pub(crate) log: Option<Logger<L>>,
    /// The way into this Actor's think loop, when it has one.
    pub(crate) thoughts: Option<Thoughts<M>>,
}

/// The sending ends of a think loop's channels: the think queue, and the
/// timers.
#[derive(Clone)]
pub(crate) struct Thoughts<M> {
    /// Messages to think about next.
    now: UnboundedSender<M>,
    /// Messages to think about once their deadline has passed.
    later: UnboundedSender<(Instant, M)>,
}

impl<M: Message, L> Context<M, L> {
    /// A second context for the same actor, with everything this one has:
    /// the think loop's, beside the perceive loop's. Only the episode makes
    /// contexts, so this is not `Clone`.
    pub(crate) fn twin(&self) -> Self {
        Self {
            id: self.id.clone(),
            episode: self.episode,
            mailboxes: self.mailboxes.clone(),
            shutdown: self.shutdown.clone(),
            log: self.log.clone(),
            thoughts: self.thoughts.clone(),
        }
    }

    /// Log `payload` as an event stamped now. The event reaches the log when
    /// this actor holds a logger and the log is listening; otherwise it is
    /// dropped, and the actor carries on either way.
    pub fn log(&self, payload: L) {
        if let Some(log) = &self.log {
            let _ = log.send(Event::now(self.episode, payload));
        }
    }

    /// Send `message` to every actor in `to`. An actor that has already
    /// stopped is skipped. An actor never sends to itself: nothing it says
    /// comes back to its own mailbox.
    ///
    /// # Errors
    ///
    /// Fails before anything is sent when `to` names this actor itself, or
    /// an actor outside those this one may send to.
    pub fn send(&self, message: M, to: HashSet<ActorId>) -> anyhow::Result<()> {
        let senders = to
            .iter()
            .map(|id| self.mailbox_of(id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        for sender in senders {
            // A failed send means the recipient's mailbox is gone.
            let _ = sender.send(message.clone());
        }
        Ok(())
    }

    /// The sending end of the mailbox of `id`.
    ///
    /// # Errors
    ///
    /// Fails when `id` is this actor itself, or not an actor this one may
    /// send to.
    fn mailbox_of(&self, id: &ActorId) -> anyhow::Result<&UnboundedSender<M>> {
        if id == &self.id {
            bail!("{id} cannot send to itself");
        }
        self.mailboxes.get(id).with_context(|| {
            format!(
                "{} cannot send to {id}: not an actor it may send to",
                self.id
            )
        })
    }

    /// Hand `message` to this actor's think loop, without waiting. The loop
    /// thinks about it once it has finished with everything handed to it
    /// before. From the perceive loop this hands slow work off; from the
    /// think loop it loops back.
    ///
    /// # Errors
    ///
    /// Fails when this actor has no think loop.
    pub fn think(&self, message: M) -> anyhow::Result<()> {
        // A failed send means the think loop has already ended.
        let _ = self.thoughts()?.now.send(message);
        Ok(())
    }

    /// Hand `message` to this actor's think loop once `delay` has passed.
    /// The deadline is set now, and the loop thinks about the message as
    /// soon after it as the thought in progress is done, ahead of anything
    /// waiting on the think queue. Timers fire in deadline order. A timer
    /// cannot be cancelled: an actor that no longer wants one recognizes it
    /// when it arrives, usually by a sequence number it put in the message,
    /// and ignores it.
    ///
    /// # Errors
    ///
    /// Fails when this actor has no think loop.
    pub fn think_after(&self, delay: Duration, message: M) -> anyhow::Result<()> {
        // A failed send means the think loop has already ended.
        let _ = self
            .thoughts()?
            .later
            .send((Instant::now() + delay, message));
        Ok(())
    }

    /// The way into this actor's think loop.
    ///
    /// # Errors
    ///
    /// Fails when this actor has no think loop.
    fn thoughts(&self) -> anyhow::Result<&Thoughts<M>> {
        self.thoughts
            .as_ref()
            .with_context(|| format!("{} cannot think: it has no think loop", self.id))
    }

    /// Stop another actor at once, wherever it is in its step.
    ///
    /// # Errors
    ///
    /// Fails when `who` is outside the actors this one may shut down.
    pub fn stop(&self, who: &ActorId) -> anyhow::Result<()> {
        match self.shutdown.others.get(who) {
            Some(token) => {
                token.cancel();
                Ok(())
            }
            None => bail!(
                "{} cannot stop {who}: not an actor it may shut down",
                self.id
            ),
        }
    }

    /// Shut this actor down. The step that calls this is abandoned at its
    /// next await, so a behavior with last words says them first. The actor
    /// then stops, leaving whatever is in its mailbox there.
    pub fn shutdown(&self) {
        self.shutdown.mine.cancel();
    }
}

/// What an actor does at each point in its life. It initializes, makes an
/// opening move once every actor is running, then handles each message it
/// receives in [`receive`](Behavior::receive). On the way out it cleans
/// up. Every method but [`context`](Behavior::context)
/// has an empty default, so the simplest behavior is a context and nothing
/// else. Anything else the behavior wants to say, such as who it is, goes
/// inside its messages.
///
/// A behavior's state is its own: the mailbox hands it one message at a
/// time, so each step may change that state freely. Actors run on a
/// multi-threaded runtime, so a behavior has to be sendable between threads.
///
/// A behavior keeps the [`Context`] it is built with and returns it from
/// [`context`](Behavior::context). The episode makes the context, and the
/// [`Builder`] in the actor's [`ActorInit`] wraps the behavior around it:
///
/// ```
/// # use async_trait::async_trait;
/// # use free_agent::{ActorInit, Behavior, Context, Message};
/// # use std::collections::HashSet;
/// # use serde::{Deserialize, Serialize};
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct Note(String);
/// impl Message for Note {}
///
/// /// Writes down everything it hears.
/// struct Scribe {
///     context: Context<Note>,
/// }
/// #[async_trait]
/// impl Behavior for Scribe {
///     type Message = Note;
///     type Log = Note;
///     fn context(&self) -> &Context<Note> {
///         &self.context
///     }
///     async fn receive(&mut self, message: &Note) -> anyhow::Result<()> {
///         self.context.log(message.clone());
///         Ok(())
///     }
/// }
///
/// let init = ActorInit {
///     behavior: Box::new(|context| Scribe { context }),
///     think: None,
///     can_send_to: HashSet::new(),
///     can_shut_down: HashSet::new(),
///     has_logger: true,
/// };
/// # let _: ActorInit<Scribe> = init;
/// ```
#[async_trait]
pub trait Behavior: Send {
    /// What this behavior sends and receives.
    type Message: Message;
    /// What this behavior logs. Most often the message type.
    type Log: Serialize + DeserializeOwned + Send + 'static;
    /// The ways out of this actor, handed to the behavior when it was built.
    fn context(&self) -> &Context<Self::Message, Self::Log>;
    /// Called before anything else: open a connection, say. No actor starts
    /// until every actor has initialized.
    async fn initialize(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    /// Handle `message`. By default it is ignored.
    async fn receive(&mut self, _message: &Self::Message) -> anyhow::Result<()> {
        Ok(())
    }
    /// Called once every actor in the episode is running. This is where an
    /// actor with an opening move makes it.
    async fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    /// Called last, after the actor has stopped taking mail: close what
    /// `initialize` opened.
    async fn clean_up(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// What an actor thinks about, one message at a time, in a task beside its
/// perceive loop. A message gets here by [`Context::think`], from either
/// loop, or as a timer by [`Context::think_after`], which hands it over once
/// a delay has passed. A due timer is thought about before anything waiting
/// on the think queue, so a deadline is never stuck behind a backlog. A slow
/// await inside [`think`](Think::think), such as a model call, holds up only
/// the next thought: the actor keeps receiving in the meantime. An error
/// from a thought is dropped, and the loop goes on to the next message.
///
/// The think loop keeps the [`Context`] it is built with, and the
/// [`ThinkBuilder`] in the actor's [`ActorInit`] wraps the `Think` around
/// it. Through it the loop does everything the behavior does: it sends
/// messages, stops actors, shuts its actor down, logs, thinks some more,
/// and sets timers.
///
/// Both loops run at once, so state they share lives in an
/// `Arc<std::sync::Mutex<_>>` held briefly and never across an await: a
/// `std::sync::MutexGuard` is not `Send`, so the compiler rejects a lock
/// held across a model call.
///
/// ```
/// # use async_trait::async_trait;
/// # use free_agent::{ActorInit, Behavior, Context, Message, Think};
/// # use std::collections::HashSet;
/// # use std::sync::{Arc, Mutex};
/// # use serde::{Deserialize, Serialize};
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum Note {
///     Question(String),
///     Findings,
///     Answer(String),
/// }
/// impl Message for Note {}
///
/// /// Hands each question off to be studied, and logs the findings so far
/// /// at once when asked for them.
/// struct Scholar {
///     context: Context<Note>,
///     findings: Arc<Mutex<Vec<String>>>,
/// }
/// #[async_trait]
/// impl Behavior for Scholar {
///     type Message = Note;
///     type Log = Note;
///     fn context(&self) -> &Context<Note> {
///         &self.context
///     }
///     async fn receive(&mut self, message: &Note) -> anyhow::Result<()> {
///         match message {
///             Note::Question(_) => self.context.think(message.clone()),
///             Note::Findings => {
///                 let findings = self.findings.lock().unwrap();
///                 for finding in findings.iter() {
///                     self.context.log(Note::Answer(finding.clone()));
///                 }
///                 Ok(())
///             }
///             Note::Answer(_) => Ok(()),
///         }
///     }
/// }
///
/// /// Takes its time over each question, then records what it found.
/// struct Study {
///     context: Context<Note>,
///     findings: Arc<Mutex<Vec<String>>>,
/// }
/// #[async_trait]
/// impl Think for Study {
///     type Message = Note;
///     async fn think(&mut self, message: &Note) -> anyhow::Result<()> {
///         let Note::Question(question) = message else {
///             return Ok(());
///         };
///         // The slow work runs with no lock held.
///         let answer = look_up(question).await;
///         self.findings.lock().unwrap().push(answer);
///         Ok(())
///     }
/// }
/// # async fn look_up(question: &str) -> String { format!("{question}: 42") }
///
/// let findings = Arc::new(Mutex::new(Vec::new()));
/// let shared = Arc::clone(&findings);
/// let init = ActorInit {
///     behavior: Box::new(move |context| Scholar { context, findings }),
///     think: Some(Box::new(move |context| {
///         Box::new(Study {
///             context,
///             findings: shared,
///         })
///     })),
///     can_send_to: HashSet::new(),
///     can_shut_down: HashSet::new(),
///     has_logger: true,
/// };
/// # let _: ActorInit<Scholar> = init;
/// ```
#[async_trait]
pub trait Think: Send {
    /// What this think loop thinks about: its actor's message type.
    type Message: Message;
    /// Think about `message`, acting through this loop's [`Context`].
    async fn think(&mut self, message: &Self::Message) -> anyhow::Result<()>;
}

/// The cancellation tokens an actor is stopped through and stops others
/// through.
#[derive(Clone)]
pub(crate) struct Shutdown {
    /// Token other Actors cancel to shut this Actor down.
    pub(crate) mine: CancellationToken,
    /// Tokens this Actor cancels to shut down other Actors.
    pub(crate) others: HashMap<ActorId, CancellationToken>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::{sleep, timeout};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Note(String);
    impl Message for Note {}

    fn note(text: &str) -> Note {
        Note(text.to_string())
    }

    /// A behavior that reports every message it receives on `heard`, after
    /// waiting for a permit from its gate if it has one. Slow to start, it
    /// waits for a permit in `start` as well. Broken, it fails every step.
    struct Reporter {
        context: Context<Note>,
        heard: UnboundedSender<Note>,
        gate: Option<Arc<Semaphore>>,
        slow_start: bool,
        broken: bool,
    }
    #[async_trait]
    impl Behavior for Reporter {
        type Message = Note;
        type Log = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn receive(&mut self, message: &Note) -> anyhow::Result<()> {
            self.step().await?;
            self.heard.send(message.clone())?;
            Ok(())
        }
        async fn start(&mut self) -> anyhow::Result<()> {
            if self.slow_start
                && let Some(gate) = &self.gate
            {
                gate.acquire().await?.forget();
            }
            Ok(())
        }
    }

    impl Reporter {
        /// Fail if broken, otherwise wait at the gate if there is one.
        async fn step(&self) -> anyhow::Result<()> {
            if self.broken {
                bail!("reporter is broken");
            }
            let Some(gate) = &self.gate else {
                return Ok(());
            };
            gate.acquire().await?.forget();
            Ok(())
        }
    }

    /// A behavior with every default: no opening move, and messages
    /// ignored.
    struct Mute(Context<Note>);
    #[async_trait]
    impl Behavior for Mute {
        type Message = Note;
        type Log = Note;
        fn context(&self) -> &Context<Note> {
            &self.0
        }
    }

    /// A behavior that reports every message it receives on `heard` and
    /// hands it to its think loop.
    struct Delegator {
        context: Context<Note>,
        heard: UnboundedSender<Note>,
    }
    #[async_trait]
    impl Behavior for Delegator {
        type Message = Note;
        type Log = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn receive(&mut self, message: &Note) -> anyhow::Result<()> {
            self.heard.send(message.clone())?;
            self.context.think(message.clone())
        }
    }

    /// A think loop that takes `delay` over each note, then reports the
    /// note it has thought about on `thought`. It fails on "fail", panics
    /// on "panic", and on "again: X" hands X back to itself first.
    struct Thinker {
        context: Context<Note>,
        delay: Duration,
        thought: UnboundedSender<Note>,
    }
    #[async_trait]
    impl Think for Thinker {
        type Message = Note;
        async fn think(&mut self, message: &Note) -> anyhow::Result<()> {
            sleep(self.delay).await;
            match message.0.as_str() {
                "fail" => bail!("cannot think about that"),
                "panic" => panic!("lost its mind"),
                _ => {}
            }
            if let Some(again) = message.0.strip_prefix("again: ") {
                self.context.think(note(again))?;
            }
            self.thought.send(message.clone())?;
            Ok(())
        }
    }

    /// A behavior that counts the messages it has received and reports the
    /// count on `heard` after each one.
    struct Tally {
        context: Context<Note>,
        heard: UnboundedSender<Note>,
        seen: usize,
    }
    #[async_trait]
    impl Behavior for Tally {
        type Message = Note;
        type Log = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn receive(&mut self, _message: &Note) -> anyhow::Result<()> {
            self.seen += 1;
            self.heard.send(note(&self.seen.to_string()))?;
            Ok(())
        }
    }

    /// An actor, along with what a test needs to feed it, start it, stop it
    /// from the outside, and hear what its behavior reports.
    struct Rig<B: Behavior = Reporter> {
        actor: Actor<B>,
        sender: UnboundedSender<Note>,
        start: oneshot::Sender<()>,
        stop: CancellationToken,
        heard: UnboundedReceiver<Note>,
    }

    impl<B: Behavior<Message = Note, Log = Note>> Rig<B> {
        fn context(&self) -> &Context<Note> {
            self.actor.behavior.context()
        }
    }

    /// A rig around a [`Reporter`] with no gate.
    fn rig(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Note>>,
        can_stop: HashMap<ActorId, CancellationToken>,
    ) -> Rig {
        rig_with(name, others, can_stop, |context, heard| Reporter {
            context,
            heard,
            gate: None,
            slow_start: false,
            broken: false,
        })
    }

    /// A rig around the behavior `build` makes from its context and the
    /// channel it reports on, which may send to `others`.
    fn rig_with<B: Behavior<Message = Note, Log = Note>>(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Note>>,
        can_stop: HashMap<ActorId, CancellationToken>,
        build: impl FnOnce(Context<Note>, UnboundedSender<Note>) -> B,
    ) -> Rig<B> {
        let (sender, mailbox) = unbounded_channel();
        let (ready, _) = oneshot::channel();
        let (start, started) = oneshot::channel();
        let (report, heard) = unbounded_channel();
        let stop = CancellationToken::new();
        let context = Context {
            id: id(name),
            episode: Uuid::new_v4(),
            mailboxes: others,
            shutdown: Shutdown {
                mine: stop.clone(),
                others: can_stop,
            },
            log: None,
            thoughts: None,
        };
        let actor = Actor {
            behavior: build(context, report),
            think: None,
            ready: Some(ready),
            start: started,
            mailbox,
        };
        Rig {
            actor,
            sender,
            start,
            stop,
            heard,
        }
    }

    /// A rig around a [`Delegator`] named `name` with a [`Thinker`] that
    /// takes `delay` over each note, along with where the thinker reports
    /// what it has thought about.
    fn thinking_rig(name: &str, delay: Duration) -> (Rig<Delegator>, UnboundedReceiver<Note>) {
        let (thought, thoughts) = unbounded_channel();
        let mut rig = rig_with(name, HashMap::new(), HashMap::new(), |context, heard| {
            Delegator { context, heard }
        });
        let build = move |context| {
            let thinker = Thinker {
                context,
                delay,
                thought,
            };
            Box::new(thinker) as Box<dyn Think<Message = Note>>
        };
        rig.actor.think = Some(ThinkLoop::open(
            Box::new(build),
            &mut rig.actor.behavior.context,
        ));
        (rig, thoughts)
    }

    /// Ann, who may send to `bob`.
    fn ann_beside<B: Behavior<Message = Note, Log = Note>>(bob: &Rig<B>) -> Rig {
        rig(
            "ann",
            HashMap::from([(id("bob"), bob.sender.clone())]),
            HashMap::new(),
        )
    }

    /// Assert that "first", "second", and "third" come on `thoughts` in
    /// that order, one each second from `began`.
    async fn first_second_third(thoughts: &mut UnboundedReceiver<Note>, began: Instant) {
        for (text, seconds) in [("first", 1), ("second", 2), ("third", 3)] {
            assert_eq!(thoughts.recv().await.unwrap(), note(text));
            assert_eq!(began.elapsed(), Duration::from_secs(seconds), "{text}");
        }
    }

    fn id(name: &str) -> ActorId {
        name.to_string()
    }

    #[tokio::test]
    async fn send_reaches_each_actor_it_is_addressed_to() {
        let (bob, mut bob_mailbox) = unbounded_channel();
        let (cat, mut cat_mailbox) = unbounded_channel();
        let (dan, mut dan_mailbox) = unbounded_channel();
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob), (id("cat"), cat), (id("dan"), dan)]),
            HashMap::new(),
        );

        ann.context()
            .send(note("hello"), HashSet::from([id("bob"), id("cat")]))
            .unwrap();

        for mailbox in [&mut bob_mailbox, &mut cat_mailbox] {
            assert_eq!(mailbox.recv().await.unwrap(), note("hello"));
        }
        assert!(dan_mailbox.try_recv().is_err(), "nothing should reach dan");
    }

    #[tokio::test]
    async fn send_fails_without_sending_if_a_recipient_may_not_be_addressed() {
        let (bob, mut bob_mailbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());

        let error = ann
            .context()
            .send(note("psst"), HashSet::from([id("bob"), id("zed")]))
            .unwrap_err();

        assert!(error.to_string().contains("zed"), "{error}");
        assert!(bob_mailbox.try_recv().is_err(), "nothing should reach bob");
    }

    #[tokio::test]
    async fn send_to_itself_fails_and_sends_nothing() {
        let (bob, mut bob_mailbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());

        let error = ann
            .context()
            .send(note("remember this"), HashSet::from([id("ann"), id("bob")]))
            .unwrap_err();

        assert!(error.to_string().contains("itself"), "{error}");
        assert!(bob_mailbox.try_recv().is_err(), "nothing should reach bob");
    }

    #[tokio::test]
    async fn send_skips_a_recipient_that_has_stopped() {
        let (bob, bob_mailbox) = unbounded_channel();
        let (cat, mut cat_mailbox) = unbounded_channel();
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob), (id("cat"), cat)]),
            HashMap::new(),
        );
        drop(bob_mailbox);

        ann.context()
            .send(note("anyone?"), HashSet::from([id("bob"), id("cat")]))
            .unwrap();

        assert_eq!(cat_mailbox.recv().await.unwrap(), note("anyone?"));
    }

    #[tokio::test]
    async fn a_message_sent_to_a_running_actor_reaches_its_behavior() {
        let mut bob = rig("bob", HashMap::new(), HashMap::new());
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        ann.context()
            .send(note("hello"), HashSet::from([id("bob")]))
            .unwrap();

        assert_eq!(bob.heard.recv().await.unwrap(), note("hello"));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_behavior_keeps_its_state_between_steps() {
        let mut bob = rig_with("bob", HashMap::new(), HashMap::new(), |context, heard| {
            Tally {
                context,
                heard,
                seen: 0,
            }
        });
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        for _ in 0..2 {
            ann.context()
                .send(note("one more"), HashSet::from([id("bob")]))
                .unwrap();
        }

        assert_eq!(bob.heard.recv().await.unwrap(), note("1"));
        assert_eq!(bob.heard.recv().await.unwrap(), note("2"));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn the_default_receive_ignores_the_message() {
        let mut bob = rig_with("bob", HashMap::new(), HashMap::new(), |context, _| {
            Mute(context)
        });
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        ann.context()
            .send(note("whatever"), HashSet::from([id("bob")]))
            .unwrap();

        // Bob is still running afterward, having said nothing of it.
        sleep(Duration::from_secs(1)).await;
        assert!(!running.is_finished(), "bob should still be running");
        assert!(bob.heard.try_recv().is_err(), "bob should report nothing");
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn stop_cancels_an_actor_it_may_shut_down_and_refuses_others() {
        let bob = rig("bob", HashMap::new(), HashMap::new());
        let ann = rig(
            "ann",
            HashMap::new(),
            HashMap::from([(id("bob"), bob.stop.clone())]),
        );

        ann.context().stop(&id("bob")).unwrap();
        assert!(bob.stop.is_cancelled());

        let error = ann.context().stop(&id("zed")).unwrap_err();
        assert!(error.to_string().contains("zed"), "{error}");
    }

    #[tokio::test]
    async fn log_sends_a_stamped_event_down_the_logger_if_there_is_one() {
        let (logger, mut events) = unbounded_channel();
        let mut ann = rig("ann", HashMap::new(), HashMap::new());
        ann.actor.behavior.context.log = Some(logger);

        ann.context().log(note("for the record"));

        let event = events.recv().await.unwrap();
        assert_eq!(event.payload, note("for the record"));
    }

    #[tokio::test]
    async fn log_does_nothing_without_a_logger() {
        let ann = rig("ann", HashMap::new(), HashMap::new());

        ann.context().log(note("into the void"));
    }

    #[tokio::test]
    async fn shutdown_cancels_the_actors_own_token() {
        let ann = rig("ann", HashMap::new(), HashMap::new());

        ann.context().shutdown();

        assert!(ann.stop.is_cancelled());
    }

    /// A rig whose actor blocks in every step until the gate gives a permit.
    fn gated_rig(name: &str) -> (Rig, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        let mut rig = rig(name, HashMap::new(), HashMap::new());
        rig.actor.behavior.gate = Some(Arc::clone(&gate));
        (rig, gate)
    }

    #[tokio::test(start_paused = true)]
    async fn a_step_in_progress_finishes_once_it_is_let_through() {
        let (mut bob, gate) = gated_rig("bob");
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        ann.context()
            .send(note("hello"), HashSet::from([id("bob")]))
            .unwrap();
        // Bob takes the message and blocks in `receive` until let through.
        sleep(Duration::from_secs(1)).await;
        assert!(bob.heard.try_recv().is_err(), "bob should still be busy");

        gate.add_permits(1);

        assert_eq!(bob.heard.recv().await.unwrap(), note("hello"));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_kill_stops_an_actor_in_the_middle_of_a_step() {
        let (bob, _gate) = gated_rig("bob");
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        bob.sender.send(note("take your time")).unwrap();
        // Let Bob take the message and block in `receive`.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let stopped = timeout(Duration::from_secs(5), running).await;
        stopped.expect("bob should stop").unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_kill_stops_an_actor_in_the_middle_of_starting() {
        let (mut bob, _gate) = gated_rig("bob");
        bob.actor.behavior.slow_start = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        // Let Bob take the start signal and block in its opening move.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let stopped = timeout(Duration::from_secs(5), running).await;
        stopped.expect("bob should stop").unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_behavior_with_no_opening_move_starts_and_waits() {
        let bob = rig_with("bob", HashMap::new(), HashMap::new(), |context, _| {
            Mute(context)
        });
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        // Let Bob take the start signal and settle into waiting.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_actor_nobody_can_send_to_waits_for_shutdown() {
        let bob = rig("bob", HashMap::new(), HashMap::new());
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        // The test held the one sender to Bob's mailbox. Without it the
        // mailbox closes, and Bob waits on.
        drop(bob.sender);
        sleep(Duration::from_secs(1)).await;
        assert!(!running.is_finished(), "bob should still be running");

        bob.stop.cancel();

        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn an_actor_stops_when_the_episode_is_gone_before_it_starts() {
        let bob = rig("bob", HashMap::new(), HashMap::new());
        let running = tokio::spawn(bob.actor.run());

        drop(bob.start);

        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_behavior_that_fails_to_receive_a_message_fails_the_actor() {
        let mut bob = rig("bob", HashMap::new(), HashMap::new());
        bob.actor.behavior.broken = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        bob.sender.send(note("hello")).unwrap();

        let error = running.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("broken"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_actor_keeps_perceiving_while_it_thinks() {
        let (mut bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(3600));
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        ann.context()
            .send(note("ponder this"), HashSet::from([id("bob")]))
            .unwrap();

        // With an hour of thinking ahead of him, Bob still hears.
        ann.context()
            .send(note("and this"), HashSet::from([id("bob")]))
            .unwrap();

        assert_eq!(bob.heard.recv().await.unwrap(), note("ponder this"));
        assert_eq!(bob.heard.recv().await.unwrap(), note("and this"));
        assert!(thoughts.try_recv().is_err(), "bob should still be thinking");
        assert_eq!(thoughts.recv().await.unwrap(), note("ponder this"));
        assert_eq!(thoughts.recv().await.unwrap(), note("and this"));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn thoughts_are_handled_one_at_a_time_in_order() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let began = Instant::now();

        for text in ["first", "second", "third"] {
            ann.context()
                .send(note(text), HashSet::from([id("bob")]))
                .unwrap();
        }

        // Each takes its second in turn: none of them overlap.
        first_second_third(&mut thoughts, began).await;
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn think_from_inside_think_waits_behind_what_is_already_queued() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        for text in ["again: later", "now"] {
            ann.context()
                .send(note(text), HashSet::from([id("bob")]))
                .unwrap();
        }

        for text in ["again: later", "now", "later"] {
            assert_eq!(thoughts.recv().await.unwrap(), note(text));
        }
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_error_from_think_is_dropped_and_the_next_thought_handled() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        for text in ["fail", "fine"] {
            ann.context()
                .send(note(text), HashSet::from([id("bob")]))
                .unwrap();
        }

        assert_eq!(thoughts.recv().await.unwrap(), note("fine"));
        assert!(!running.is_finished(), "bob should still be running");
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn think_fails_on_an_actor_with_no_think_loop() {
        let ann = rig("ann", HashMap::new(), HashMap::new());

        let error = ann.context().think(note("hmm")).unwrap_err();

        assert!(error.to_string().contains("no think loop"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_kill_stops_an_actor_in_the_middle_of_a_think() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(3600));
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        ann.context()
            .send(note("ponder this"), HashSet::from([id("bob")]))
            .unwrap();
        // Let Bob hand the note off and settle into thinking about it.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let stopped = timeout(Duration::from_secs(5), running).await;
        stopped.expect("bob should stop").unwrap().unwrap();
        // The thinker is gone with its thought unfinished.
        assert!(thoughts.recv().await.is_none(), "nothing should be thought");
    }

    #[tokio::test(start_paused = true)]
    async fn a_panic_in_the_think_loop_is_the_actors_error_once_it_stops() {
        let (mut bob, _thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let ann = ann_beside(&bob);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        ann.context()
            .send(note("panic"), HashSet::from([id("bob")]))
            .unwrap();
        // Bob's think loop has panicked, and Bob perceives on regardless.
        sleep(Duration::from_secs(2)).await;
        ann.context()
            .send(note("still there?"), HashSet::from([id("bob")]))
            .unwrap();
        assert_eq!(bob.heard.recv().await.unwrap(), note("panic"));
        assert_eq!(bob.heard.recv().await.unwrap(), note("still there?"));

        bob.stop.cancel();

        let error = running.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("panicked"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_due_timer_is_handled_before_messages_waiting_on_the_think_queue() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let context = bob.context().twin();
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        // The alarm falls due while Bob is on "first", with "second" waiting.
        context.think(note("first")).unwrap();
        context.think(note("second")).unwrap();
        context
            .think_after(Duration::from_millis(500), note("alarm"))
            .unwrap();

        for text in ["first", "alarm", "second"] {
            assert_eq!(thoughts.recv().await.unwrap(), note(text));
        }
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_timer_set_during_a_think_is_handled_as_soon_as_the_think_returns() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let context = bob.context().twin();
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let began = Instant::now();
        context.think(note("first")).unwrap();
        // Let Bob settle into thinking about "first".
        sleep(Duration::from_millis(100)).await;

        // The alarm comes due in the middle of "first", behind "second".
        context.think(note("second")).unwrap();
        context
            .think_after(Duration::from_millis(500), note("alarm"))
            .unwrap();

        for (text, seconds) in [("first", 1), ("alarm", 2), ("second", 3)] {
            assert_eq!(thoughts.recv().await.unwrap(), note(text));
            assert_eq!(began.elapsed(), Duration::from_secs(seconds), "{text}");
        }
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_deadline_is_measured_from_the_call_to_think_after() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::from_secs(1));
        let context = bob.context().twin();
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let began = Instant::now();
        context.think(note("first")).unwrap();
        // Let Bob settle into thinking about "first".
        sleep(Duration::from_millis(100)).await;

        // Set at 0.1s for 2.1s, though the loop only sees it at 1s.
        context
            .think_after(Duration::from_secs(2), note("alarm"))
            .unwrap();

        assert_eq!(thoughts.recv().await.unwrap(), note("first"));
        assert_eq!(thoughts.recv().await.unwrap(), note("alarm"));
        assert_eq!(began.elapsed(), Duration::from_millis(3100));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn timers_fire_in_deadline_order() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::ZERO);
        let context = bob.context().twin();
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let began = Instant::now();

        for (text, seconds) in [("third", 3), ("first", 1), ("second", 2)] {
            context
                .think_after(Duration::from_secs(seconds), note(text))
                .unwrap();
        }

        first_second_third(&mut thoughts, began).await;
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_timer_is_not_handled_before_its_deadline() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::ZERO);
        let context = bob.context().twin();
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let began = Instant::now();

        context
            .think_after(Duration::from_secs(10), note("alarm"))
            .unwrap();

        sleep(Duration::from_secs(9)).await;
        assert!(thoughts.try_recv().is_err(), "the alarm should not be due");
        assert_eq!(thoughts.recv().await.unwrap(), note("alarm"));
        assert_eq!(began.elapsed(), Duration::from_secs(10));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutting_down_drops_pending_timers() {
        let (bob, mut thoughts) = thinking_rig("bob", Duration::ZERO);
        let context = bob.context().twin();
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        for seconds in [1, 2] {
            context
                .think_after(Duration::from_secs(seconds), note("alarm"))
                .unwrap();
        }
        // Let Bob set both alarms and settle into waiting for them.
        sleep(Duration::from_millis(500)).await;

        bob.stop.cancel();

        running.await.unwrap().unwrap();
        // The thinker is gone, and neither alarm reached it.
        assert!(thoughts.recv().await.is_none(), "nothing should be thought");
    }

    #[tokio::test]
    async fn think_after_fails_on_an_actor_with_no_think_loop() {
        let ann = rig("ann", HashMap::new(), HashMap::new());

        let error = ann
            .context()
            .think_after(Duration::from_secs(1), note("hmm"))
            .unwrap_err();

        assert!(error.to_string().contains("no think loop"), "{error}");
    }
}
