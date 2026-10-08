//! Actors in the sense of the [actor model]: each one keeps its own state,
//! takes messages from a mailbox one at a time, and reaches other actors
//! only by sending them messages.
//!
//! Here an actor is a [`Behavior`] driven by a mailbox. The behavior
//! reaches the rest of the episode through its [`Context`], which an
//! [`Episode`](crate::Episode) builds from the actor's [`ActorInit`].
//!
//! [actor model]: https://en.wikipedia.org/wiki/Actor_model

use crate::log::{Event, Logger};
use crate::message::{Message, Request};
use anyhow::{Context as _, bail};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow::{self, Break, Continue};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// An actor's name, unique within an episode.
pub type ActorId = String;

/// Builds a [`Behavior`] from the [`Context`] it will own.
pub type Builder<B> =
    Box<dyn FnOnce(Context<<B as Behavior>::Message, <B as Behavior>::Log>) -> B + Send>;

/// Everything an [`Episode`](crate::Episode) needs to build one actor: how
/// it behaves and whom it may reach.
pub struct ActorInit<B: Behavior> {
    /// Builds the behavior once its [`Context`] exists, which is when the
    /// episode has opened every actor's channels.
    pub behavior: Builder<B>,
    /// The actors this one may send to and request from.
    pub can_send_to: HashSet<ActorId>,
    /// The actors this one may stop.
    pub can_shut_down: HashSet<ActorId>,
    /// Whether this actor gets a copy of the episode's logger.
    pub has_logger: bool,
}

/// A running actor: its behavior, its mailbox, and the two one-time signals
/// it exchanges with the episode on the way up.
pub(crate) struct Actor<B: Behavior> {
    /// What this Actor does at each point in its life. It owns the
    /// [`Context`] through which it reaches other actors.
    pub(crate) behavior: B,
    /// This Actor's one-time signal to the episode that it is initialized
    /// and running its message loop. Taken when it is sent.
    pub(crate) ready: Option<oneshot::Sender<()>>,
    /// The episode's one-time signal that every actor is running.
    pub(crate) start: oneshot::Receiver<()>,
    /// The channel on which this Actor receives incoming Messages.
    pub(crate) mailbox: UnboundedReceiver<Envelope<B::Message>>,
}

impl<B: Behavior> Actor<B> {
    /// An actor's whole life, in four phases.
    ///
    /// 1. Initialize, and tell the episode this actor is ready.
    /// 2. Wait for the episode's start signal, which comes once every actor
    ///    is ready, and make the opening move.
    /// 3. Deliver each envelope in the mailbox to the behavior, one at a
    ///    time, until shut down.
    /// 4. Clean up.
    ///
    /// Phases 2 and 3 overlap: mail that arrives before the start signal is
    /// delivered as it comes. An error from the behavior at any phase ends
    /// the actor with that error.
    ///
    /// Shutdown cuts any phase short. A step in progress is dropped at its
    /// next await, mail still in the mailbox stays there, and cleanup runs
    /// only after a finished initialization.
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
        let mut start_consumed = false;
        loop {
            // Wait for whichever happens first: shutdown, the start signal,
            // or the next envelope. Each arm is `pattern = future => body`;
            // the body runs with the future's output bound to the pattern.
            // `biased` tries the arms in order, so shutdown wins a tie. The
            // start arm drops out once it has fired, because a oneshot
            // receiver panics if polled again.
            let flow = tokio::select! {
                biased;
                _ = shutdown.cancelled() => Break(()),
                signal = &mut self.start, if !start_consumed => {
                    start_consumed = true;
                    self.open(signal).await?
                }
                envelope = self.mailbox.recv() => {
                    // This actor holds a sender to its own mailbox, so the
                    // mailbox outlives the loop.
                    self.deliver(envelope.expect("mailbox closed")).await?
                }
            };
            if flow.is_break() {
                break;
            }
        }
        self.behavior.clean_up().await?;
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

    /// Hand `envelope` to the behavior: either a statement to receive or a
    /// request to answer and reply to.
    ///
    /// The message loop watches for shutdown only between envelopes, so each
    /// step races shutdown on its own here.
    async fn deliver(&mut self, envelope: Envelope<B::Message>) -> anyhow::Result<ControlFlow<()>> {
        let shutdown = self.behavior.context().shutdown.mine.clone();
        match envelope {
            Envelope::Statement(message) => {
                let Some(received) =
                    run_unless_stopped(&shutdown, self.behavior.receive(&message)).await
                else {
                    return Ok(Break(()));
                };
                received?;
            }
            Envelope::Request(request) => {
                let Some(answered) =
                    run_unless_stopped(&shutdown, self.behavior.answer(request.message())).await
                else {
                    return Ok(Break(()));
                };
                // The asker may have stopped waiting, which is no fault of
                // this actor.
                let _ = request.reply(answered?);
            }
        }
        Ok(Continue(()))
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

/// The ways out of an actor: what a [`Behavior`] may do besides answer.
///
/// A behavior owns one of these, and the actor keeps the mailbox, so messages
/// reach the behavior one step at a time.
///
/// `L` is what this actor logs. It defaults to the message type, which is
/// what most actors log.
pub struct Context<M: Message, L = M> {
    /// This actor's name, as the other actors know it.
    pub id: ActorId,
    /// The sending ends of the mailboxes this actor may put something in,
    /// its own among them.
    pub(crate) mailboxes: HashMap<ActorId, UnboundedSender<Envelope<M>>>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    pub(crate) shutdown: Shutdown,
    /// Where this Actor's events go, when it has a logger.
    pub(crate) log: Option<Logger<L>>,
}

impl<M: Message, L> Context<M, L> {
    /// Log `payload` as an event stamped now. The event reaches the log when
    /// this actor holds a logger and the log is listening; otherwise it is
    /// dropped, and the actor carries on either way.
    pub fn log(&self, payload: L) {
        if let Some(log) = &self.log {
            let _ = log.send(Event::now(payload));
        }
    }

    /// Send the statement `message` to every actor in `to`. An actor that
    /// has already stopped is skipped. An actor may send to itself: the
    /// message waits in its own mailbox for the current step to finish.
    ///
    /// # Errors
    ///
    /// Fails before anything is sent when `to` names an actor outside those
    /// this one may send to.
    pub fn send(&self, message: M, to: HashSet<ActorId>) -> anyhow::Result<()> {
        let senders = to
            .iter()
            .map(|id| self.mailbox_of(id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        for sender in senders {
            // A failed send means the recipient's mailbox is gone.
            let _ = sender.send(Envelope::Statement(message.clone()));
        }
        Ok(())
    }

    /// Ask every actor in `to` the same thing and collect their replies. A
    /// recipient that has stopped, before or after receiving the request,
    /// is left out of the result.
    ///
    /// # Errors
    ///
    /// Fails before anything is sent when `to` names an actor outside those
    /// this one may send to, or names this actor itself, which is busy with this
    /// very step.
    pub async fn request(
        &self,
        message: M,
        to: HashSet<ActorId>,
    ) -> anyhow::Result<HashMap<ActorId, Vec<M>>> {
        if to.contains(&self.id) {
            bail!(
                "{} cannot request from itself: it would wait forever",
                self.id
            );
        }
        let senders = to
            .iter()
            .map(|id| self.mailbox_of(id).map(|sender| (id, sender)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut pending = Vec::new();
        for (id, sender) in senders {
            let (request, reply) = Request::new(message.clone());
            if sender.send(Envelope::Request(request)).is_ok() {
                pending.push((id.clone(), reply));
            }
        }
        let mut replies = HashMap::new();
        for (id, reply) in pending {
            // A dropped reply channel means the recipient stopped first.
            if let Ok(reply) = reply.await {
                replies.insert(id, reply.messages);
            }
        }
        Ok(replies)
    }

    /// The sending end of the mailbox of `id`, which is this actor's own
    /// when `id` is its own name.
    ///
    /// # Errors
    ///
    /// Fails when `id` is not an actor this one may send to.
    fn mailbox_of(&self, id: &ActorId) -> anyhow::Result<&UnboundedSender<Envelope<M>>> {
        self.mailboxes.get(id).with_context(|| {
            format!(
                "{} cannot send to {id}: not an actor it may send to",
                self.id
            )
        })
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
/// opening move once every actor is running, then maps what it receives to
/// what it does: a statement goes to [`receive`](Behavior::receive) and a
/// request to [`answer`](Behavior::answer), whose result is the reply. On
/// the way out it cleans up. Every method but [`context`](Behavior::context)
/// has an empty default, so the simplest behavior is a context and nothing
/// else. Anything else the behavior wants to say, such as who it is, goes
/// inside its messages.
///
/// A behavior's state is its own: the mailbox hands it one message at a
/// time, so each step may change that state freely. Actors run on a
/// multi-threaded runtime, so a behavior has to be sendable between threads.
#[async_trait]
pub trait Behavior: Send {
    /// What this behavior sends and receives.
    type Message: Message;
    /// What this behavior logs. Most often the message type.
    type Log: Send + 'static;
    /// The ways out of this actor, handed to the behavior when it was built.
    fn context(&self) -> &Context<Self::Message, Self::Log>;
    /// Called before anything else: open a connection, say. No actor starts
    /// until every actor has initialized.
    async fn initialize(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    /// Handle the statement `message`. By default it is ignored.
    async fn receive(&mut self, _message: &Self::Message) -> anyhow::Result<()> {
        Ok(())
    }
    /// Answer the request `message`. The result is the reply, which by
    /// default is nothing.
    async fn answer(&mut self, _message: &Self::Message) -> anyhow::Result<Vec<Self::Message>> {
        Ok(vec![])
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

/// The cancellation tokens an actor is stopped through and stops others
/// through.
pub(crate) struct Shutdown {
    /// Token other Actors cancel to shut this Actor down.
    pub(crate) mine: CancellationToken,
    /// Tokens this Actor cancels to shut down other Actors.
    pub(crate) others: HashMap<ActorId, CancellationToken>,
}

/// A message and what its recipient owes for it.
#[derive(Debug)]
pub(crate) enum Envelope<M: Message> {
    /// A message that does not require a reply.
    Statement(M),
    /// A message whose sender is waiting for reply.
    Request(Request<M>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::{sleep, timeout};

    #[derive(Debug, Clone, PartialEq)]
    struct Note(&'static str);
    impl Message for Note {}

    /// A behavior that answers every request with the message it was sent,
    /// after waiting for a permit from its gate if it has one, and waits at
    /// the gate on statements too. Slow to start, it waits for a permit in
    /// `start` as well. Broken, it fails every step.
    struct Echo {
        context: Context<Note>,
        gate: Option<Arc<Semaphore>>,
        slow_start: bool,
        broken: bool,
    }
    #[async_trait]
    impl Behavior for Echo {
        type Message = Note;
        type Log = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn receive(&mut self, _message: &Note) -> anyhow::Result<()> {
            self.step().await
        }
        async fn answer(&mut self, message: &Note) -> anyhow::Result<Vec<Note>> {
            self.step().await?;
            Ok(vec![message.clone()])
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

    impl Echo {
        /// Fail if broken, otherwise wait at the gate if there is one.
        async fn step(&self) -> anyhow::Result<()> {
            if self.broken {
                bail!("echo is broken");
            }
            if let Some(gate) = &self.gate {
                gate.acquire().await?.forget();
            }
            Ok(())
        }
    }

    /// A behavior with every default: no opening move, statements ignored,
    /// and requests answered with nothing.
    struct Mute(Context<Note>);
    #[async_trait]
    impl Behavior for Mute {
        type Message = Note;
        type Log = Note;
        fn context(&self) -> &Context<Note> {
            &self.0
        }
    }

    /// A behavior that counts the statements it has received and answers
    /// every request with that many notes.
    struct Tally {
        context: Context<Note>,
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
            Ok(())
        }
        async fn answer(&mut self, _message: &Note) -> anyhow::Result<Vec<Note>> {
            Ok(vec![Note("seen"); self.seen])
        }
    }

    /// An actor, along with what a test needs to feed it, start it, and stop
    /// it from the outside.
    struct Rig<B: Behavior = Echo> {
        actor: Actor<B>,
        sender: UnboundedSender<Envelope<Note>>,
        start: oneshot::Sender<()>,
        stop: CancellationToken,
    }

    impl<B: Behavior<Message = Note, Log = Note>> Rig<B> {
        fn context(&self) -> &Context<Note> {
            self.actor.behavior.context()
        }
    }

    /// A rig around an [`Echo`] with no gate.
    fn rig(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Envelope<Note>>>,
        can_stop: HashMap<ActorId, CancellationToken>,
    ) -> Rig {
        rig_with(name, others, can_stop, |context| Echo {
            context,
            gate: None,
            slow_start: false,
            broken: false,
        })
    }

    /// A rig around the behavior `build` makes from its context, which may
    /// send to itself and to `others`.
    fn rig_with<B: Behavior<Message = Note, Log = Note>>(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Envelope<Note>>>,
        can_stop: HashMap<ActorId, CancellationToken>,
        build: impl FnOnce(Context<Note>) -> B,
    ) -> Rig<B> {
        let (sender, mailbox) = unbounded_channel();
        let (ready, _) = oneshot::channel();
        let (start, started) = oneshot::channel();
        let stop = CancellationToken::new();
        let mut mailboxes = others;
        mailboxes.insert(id(name), sender.clone());
        let context = Context {
            id: id(name),
            mailboxes,
            shutdown: Shutdown {
                mine: stop.clone(),
                others: can_stop,
            },
            log: None,
        };
        let actor = Actor {
            behavior: build(context),
            ready: Some(ready),
            start: started,
            mailbox,
        };
        Rig {
            actor,
            sender,
            start,
            stop,
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
            .send(Note("hello"), HashSet::from([id("bob"), id("cat")]))
            .unwrap();

        for mailbox in [&mut bob_mailbox, &mut cat_mailbox] {
            let heard = mailbox.recv().await.unwrap();
            assert!(
                matches!(heard, Envelope::Statement(Note("hello"))),
                "{heard:?}"
            );
        }
        assert!(dan_mailbox.try_recv().is_err(), "nothing should reach dan");
    }

    #[tokio::test]
    async fn send_fails_without_sending_if_a_recipient_may_not_be_addressed() {
        let (bob, mut bob_mailbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());

        let error = ann
            .context()
            .send(Note("psst"), HashSet::from([id("bob"), id("zed")]))
            .unwrap_err();

        assert!(error.to_string().contains("zed"), "{error}");
        assert!(bob_mailbox.try_recv().is_err(), "nothing should reach bob");
    }

    #[tokio::test]
    async fn send_to_itself_lands_in_the_actors_own_mailbox() {
        let mut ann = rig("ann", HashMap::new(), HashMap::new());

        ann.context()
            .send(Note("remember this"), HashSet::from([id("ann")]))
            .unwrap();

        let heard = ann.actor.mailbox.recv().await.unwrap();
        assert!(
            matches!(heard, Envelope::Statement(Note("remember this"))),
            "{heard:?}"
        );
    }

    #[tokio::test]
    async fn request_collects_a_reply_from_every_recipient() {
        let bob = rig("bob", HashMap::new(), HashMap::new());
        let cat = rig("cat", HashMap::new(), HashMap::new());
        let ann = rig(
            "ann",
            HashMap::from([
                (id("bob"), bob.sender.clone()),
                (id("cat"), cat.sender.clone()),
            ]),
            HashMap::new(),
        );
        let bob_running = tokio::spawn(bob.actor.run());
        let cat_running = tokio::spawn(cat.actor.run());
        bob.start.send(()).unwrap();
        cat.start.send(()).unwrap();

        let replies = ann
            .context()
            .request(Note("who's there?"), HashSet::from([id("bob"), id("cat")]))
            .await
            .unwrap();

        let expected = HashMap::from([
            (id("bob"), vec![Note("who's there?")]),
            (id("cat"), vec![Note("who's there?")]),
        ]);
        assert_eq!(replies, expected);
        bob.stop.cancel();
        cat.stop.cancel();
        bob_running.await.unwrap().unwrap();
        cat_running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_behavior_keeps_its_state_between_steps() {
        let bob = rig_with("bob", HashMap::new(), HashMap::new(), |context| Tally {
            context,
            seen: 0,
        });
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob.sender.clone())]),
            HashMap::new(),
        );
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        for _ in 0..2 {
            ann.context()
                .send(Note("one more"), HashSet::from([id("bob")]))
                .unwrap();
        }

        let replies = ann
            .context()
            .request(Note("how many?"), HashSet::from([id("bob")]))
            .await
            .unwrap();

        let expected = HashMap::from([(id("bob"), vec![Note("seen"), Note("seen")])]);
        assert_eq!(replies, expected);
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn the_default_receive_ignores_the_statement() {
        let bob = rig_with("bob", HashMap::new(), HashMap::new(), Mute);
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob.sender.clone())]),
            HashMap::new(),
        );
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        ann.context()
            .send(Note("whatever"), HashSet::from([id("bob")]))
            .unwrap();

        // Bob is still running and answering afterward.
        let replies = ann
            .context()
            .request(Note("still there?"), HashSet::from([id("bob")]))
            .await
            .unwrap();
        assert_eq!(replies, HashMap::from([(id("bob"), vec![])]));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn the_default_answer_is_nothing() {
        let bob = rig_with("bob", HashMap::new(), HashMap::new(), Mute);
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob.sender.clone())]),
            HashMap::new(),
        );
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        let replies = ann
            .context()
            .request(Note("anything?"), HashSet::from([id("bob")]))
            .await
            .unwrap();

        assert_eq!(replies, HashMap::from([(id("bob"), vec![])]));
        bob.stop.cancel();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_fails_without_sending_if_a_recipient_may_not_be_addressed() {
        let (bob, mut bob_mailbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());

        let error = ann
            .context()
            .request(Note("psst"), HashSet::from([id("bob"), id("zed")]))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("zed"), "{error}");
        assert!(bob_mailbox.try_recv().is_err(), "nothing should reach bob");
    }

    #[tokio::test]
    async fn request_refuses_to_ask_the_actor_itself() {
        let ann = rig("ann", HashMap::new(), HashMap::new());

        let error = ann
            .context()
            .request(Note("hello me"), HashSet::from([id("ann")]))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("itself"), "{error}");
    }

    #[tokio::test]
    async fn request_leaves_out_a_recipient_that_has_stopped() {
        let (bob, bob_mailbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());
        drop(bob_mailbox);

        let replies = ann
            .context()
            .request(Note("anyone?"), HashSet::from([id("bob")]))
            .await
            .unwrap();

        assert!(replies.is_empty(), "{replies:?}");
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

        ann.context().log(Note("for the record"));

        let event = events.recv().await.unwrap();
        assert_eq!(event.payload, Note("for the record"));
    }

    #[tokio::test]
    async fn log_does_nothing_without_a_logger() {
        let ann = rig("ann", HashMap::new(), HashMap::new());

        ann.context().log(Note("into the void"));
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
    async fn a_kill_stops_an_actor_in_the_middle_of_a_step() {
        let (bob, _gate) = gated_rig("bob");
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        bob.sender
            .send(Envelope::Statement(Note("take your time")))
            .unwrap();
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
        let bob = rig_with("bob", HashMap::new(), HashMap::new(), Mute);
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        // Let Bob take the start signal and settle into waiting.
        sleep(Duration::from_secs(1)).await;

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
    async fn a_behavior_that_fails_to_receive_a_statement_fails_the_actor() {
        let mut bob = rig("bob", HashMap::new(), HashMap::new());
        bob.actor.behavior.broken = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        bob.sender.send(Envelope::Statement(Note("hello"))).unwrap();

        let error = running.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("broken"), "{error}");
    }

    #[tokio::test]
    async fn a_behavior_that_fails_to_answer_a_request_fails_the_actor() {
        let mut bob = rig("bob", HashMap::new(), HashMap::new());
        bob.actor.behavior.broken = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let (request, _reply) = Request::new(Note("well?"));

        bob.sender.send(Envelope::Request(request)).unwrap();

        let error = running.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("broken"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_recipient_killed_mid_step_is_left_out_of_the_replies() {
        let (bob, _gate) = gated_rig("bob");
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob.sender.clone())]),
            HashMap::new(),
        );
        let bob_running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let asking = tokio::spawn(async move {
            ann.context()
                .request(Note("well?"), HashSet::from([id("bob")]))
                .await
        });
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let replies = asking.await.unwrap().unwrap();
        assert!(replies.is_empty(), "{replies:?}");
        bob_running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_responder_whose_asker_gave_up_carries_on() {
        let (bob, gate) = gated_rig("bob");
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob.sender.clone())]),
            HashMap::new(),
        );
        let bob_running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let asking = tokio::spawn(async move {
            ann.context()
                .request(Note("well?"), HashSet::from([id("bob")]))
                .await
        });
        sleep(Duration::from_secs(1)).await;

        // Ann gives up, dropping her reply channel, and then Bob answers.
        asking.abort();
        gate.add_permits(1);
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();
        bob_running.await.unwrap().unwrap();
    }
}
