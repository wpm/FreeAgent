//! An actor is a [`Strategy`] driven by an inbox, with a [`Lifecycle`]
//! around it. The strategy reaches the rest of the episode through its
//! [`Context`], which an [`Episode`](crate::Episode) builds from the
//! actor's [`ActorInit`].

use crate::log::{Event, Logger};
use crate::message::{Message, Request};
use anyhow::{Context as _, bail};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// An actor's name, unique within an episode.
pub type ActorId = String;

/// Builds a [`Strategy`] from the [`Context`] it will own.
pub type Builder<S> =
    Box<dyn FnOnce(Context<<S as Strategy>::Message, <S as Strategy>::Payload>) -> S + Send>;

/// Everything an [`Episode`](crate::Episode) needs to build one actor: how
/// it lives, how it behaves, and whom it may reach.
pub struct ActorInit<L: Lifecycle, S: Strategy> {
    /// How this actor sets up and tears down.
    pub lifecycle: L,
    /// Builds the strategy once its [`Context`] exists, which is when the
    /// episode has opened every actor's channels.
    pub strategy: Builder<S>,
    /// The actors this one may send to and request from.
    pub can_send_to: HashSet<ActorId>,
    /// The actors this one may stop.
    pub can_shut_down: HashSet<ActorId>,
    /// Whether this actor gets a copy of the episode's logger.
    pub has_logger: bool,
}

/// A running actor: its lifecycle, its strategy, its inbox, and the two
/// one-time signals it exchanges with the episode on the way up.
pub(crate) struct Actor<L: Lifecycle, S: Strategy> {
    /// How this Actor handles startup and shutdown.
    pub(crate) lifecycle: L,
    /// How this Actor handles incoming Messages. It owns the [`Context`]
    /// through which it reaches other actors.
    pub(crate) strategy: S,
    /// This Actor's one-time signal to the episode that it is initialized
    /// and running its message loop.
    pub(crate) ready: oneshot::Sender<()>,
    /// The episode's one-time signal that every actor is running.
    pub(crate) start: oneshot::Receiver<()>,
    /// The channel on which this Actor receives incoming Messages.
    pub(crate) inbox: UnboundedReceiver<Observation<S::Message>>,
}

impl<L: Lifecycle, S: Strategy> Actor<L, S> {
    /// Initialize, then handle the start signal and observations until shut
    /// down or until every sender to this actor's inbox is gone, then clean
    /// up.
    ///
    /// Shutdown takes effect at once: a step in progress is abandoned at its
    /// next await, and whatever is waiting in the inbox stays there. An actor
    /// shut down while still initializing stops right there; cleanup follows
    /// a finished initialization.
    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        let shutdown = self.strategy.context().shutdown.mine.clone();
        let Some(initialized) = unless_stopped(&shutdown, self.lifecycle.initialize()).await else {
            return Ok(());
        };
        initialized?;
        // The episode may already be gone, in which case no one is waiting.
        let _ = self.ready.send(());
        // A oneshot receiver panics if polled after it completes, so it is
        // taken out of the select once it has fired.
        let mut start = Some(self.start);
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                started = async { start.as_mut().expect("guarded by the branch condition").await },
                    if start.is_some() =>
                {
                    start = None;
                    match started {
                        Ok(()) => {
                            let Some(opened) = unless_stopped(&shutdown, self.strategy.start()).await else {
                                break;
                            };
                            opened?;
                        }
                        Err(_) => break, // The episode is gone.
                    }
                }
                observation = self.inbox.recv() => match observation {
                    None => break, // Every sender is gone.
                    Some(Observation::Broadcast(message)) => {
                        let step = self.strategy.policy(&message);
                        let Some(acted) = unless_stopped(&shutdown, step).await else {
                            break;
                        };
                        acted?;
                    }
                    Some(Observation::Request(request)) => {
                        let step = self.strategy.policy(request.message());
                        let Some(acted) = unless_stopped(&shutdown, step).await else {
                            break;
                        };
                        // The asker may have stopped waiting, which is no
                        // fault of this actor.
                        let _ = request.reply(acted?);
                    }
                },
            }
        }
        self.lifecycle.clean_up().await?;
        Ok(())
    }
}

/// Run `step`, unless `shutdown` fires first.
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

/// The ways out of an actor: what a [`Strategy`] may do besides answer.
///
/// A strategy owns one of these, and the actor keeps the inbox, so messages
/// reach the strategy one step at a time.
///
/// `P` is what this actor logs. It defaults to the message type, which is
/// what most actors log.
pub struct Context<M: Message, P = M> {
    /// This actor's name, as the other actors know it.
    pub id: ActorId,
    /// The channels on which this Actor sends Messages.
    pub(crate) outbox: Outbox<M>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    pub(crate) shutdown: Shutdown,
    /// Where this Actor's events go, when it has a logger.
    pub(crate) log: Option<Logger<P>>,
}

impl<M: Message, P> Context<M, P> {
    /// Log `payload` as an event stamped now. The event reaches the log when
    /// this actor holds a logger and the log is listening; otherwise it is
    /// dropped, and the actor carries on either way.
    pub fn log(&self, payload: P) {
        if let Some(log) = &self.log {
            let _ = log.send(Event::now(payload));
        }
    }

    /// Broadcast `message` to every actor this one may send to. An actor
    /// that has already stopped is skipped.
    pub fn send(&self, message: M) -> anyhow::Result<()> {
        for sender in self.outbox.others.values() {
            // A failed send means the recipient's inbox is gone.
            let _ = sender.send(Observation::Broadcast(message.clone()));
        }
        Ok(())
    }

    /// Leave `message` in this actor's own inbox, to be handled in a later
    /// step. This is how an actor messages itself: it takes one step at a
    /// time, so the message waits for the current step to finish.
    pub fn note(&self, message: M) -> anyhow::Result<()> {
        self.outbox
            .loopback
            .send(Observation::Broadcast(message))
            .ok()
            .context("this actor's own inbox is gone")
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
            .map(|id| {
                self.outbox.others.get_key_value(id).with_context(|| {
                    format!(
                        "{} cannot request from {id}: not an actor it may send to",
                        self.id
                    )
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut pending = Vec::new();
        for (id, sender) in senders {
            let (request, reply) = Request::new(message.clone());
            if sender.send(Observation::Request(request)).is_ok() {
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
    /// next await, so a strategy with last words says them first. The actor
    /// then stops, leaving whatever is in its inbox there.
    pub fn shutdown(&self) {
        self.shutdown.mine.cancel();
    }
}

/// How an actor maps what it observes to what it does. The signature of
/// [`policy`](Strategy::policy) is the reinforcement-learning one: an
/// observation in, actions out. Anything else the strategy wants to say,
/// such as who it is, goes inside its messages.
///
/// Actors run on a multi-threaded runtime, so a strategy has to be shareable
/// across threads.
#[async_trait]
pub trait Strategy: Send + Sync {
    /// What this strategy sends and receives.
    type Message: Message;
    /// What this strategy logs. Most often the message type.
    type Payload: Send + 'static;
    /// The ways out of this actor, handed to the strategy when it was built.
    fn context(&self) -> &Context<Self::Message, Self::Payload>;
    /// Handle `observation`. The result is the reply to a request, and is
    /// dropped after a broadcast.
    async fn policy(&self, observation: &Self::Message) -> anyhow::Result<Vec<Self::Message>>;
    /// Called once every actor in the episode is running. This is where an
    /// actor with an opening move makes it.
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// What an actor does before its first step and after its last: open a
/// connection, say, and close it again. Both methods have empty defaults,
/// so the simplest lifecycle is an empty impl.
///
/// Actors run on a multi-threaded runtime, so a lifecycle has to be
/// shareable across threads.
#[async_trait]
pub trait Lifecycle: Send + Sync {
    /// Called just before the message handling loop begins.
    async fn initialize(&self) -> anyhow::Result<()> {
        Ok(())
    }
    /// Called just before the message handling loop exits.
    async fn clean_up(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// The sending ends of the inboxes an actor may put something in.
pub(crate) struct Outbox<M: Message> {
    /// Channel on which an Actor sends a Message to itself.
    pub(crate) loopback: UnboundedSender<Observation<M>>,
    /// Channels on which an Actor sends Messages to other Actors.
    pub(crate) others: HashMap<ActorId, UnboundedSender<Observation<M>>>,
}

/// The cancellation tokens an actor is stopped through and stops others
/// through.
pub(crate) struct Shutdown {
    /// Token other Actors cancel to shut this Actor down.
    pub(crate) mine: CancellationToken,
    /// Tokens this Actor cancels to shut down other Actors.
    pub(crate) others: HashMap<ActorId, CancellationToken>,
}

/// What arrives in an actor's inbox.
#[derive(Debug)]
pub(crate) enum Observation<M: Message> {
    /// A message sent and forgotten.
    Broadcast(M),
    /// A message whose sender is waiting for an answer.
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

    struct Idle;
    #[async_trait]
    impl Lifecycle for Idle {}

    /// A strategy that answers every request with the message it was sent,
    /// after waiting for a permit from its gate if it has one. Slow to
    /// start, it waits for a permit in `start` too. Broken, it fails every
    /// step.
    struct Echo {
        context: Context<Note>,
        gate: Option<Arc<Semaphore>>,
        slow_start: bool,
        broken: bool,
    }
    #[async_trait]
    impl Strategy for Echo {
        type Message = Note;
        type Payload = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn policy(&self, observation: &Note) -> anyhow::Result<Vec<Note>> {
            if self.broken {
                bail!("echo is broken");
            }
            if let Some(gate) = &self.gate {
                gate.acquire().await?.forget();
            }
            Ok(vec![observation.clone()])
        }
        async fn start(&self) -> anyhow::Result<()> {
            if self.slow_start
                && let Some(gate) = &self.gate
            {
                gate.acquire().await?.forget();
            }
            Ok(())
        }
    }

    /// A strategy with the default opening move, which answers nothing.
    struct Mute(Context<Note>);
    #[async_trait]
    impl Strategy for Mute {
        type Message = Note;
        type Payload = Note;
        fn context(&self) -> &Context<Note> {
            &self.0
        }
        async fn policy(&self, _observation: &Note) -> anyhow::Result<Vec<Note>> {
            Ok(vec![])
        }
    }

    /// An actor, along with what a test needs to feed it, start it, and stop
    /// it from the outside.
    struct Rig<S: Strategy = Echo> {
        actor: Actor<Idle, S>,
        sender: UnboundedSender<Observation<Note>>,
        start: oneshot::Sender<()>,
        stop: CancellationToken,
    }

    impl<S: Strategy<Message = Note, Payload = Note>> Rig<S> {
        fn context(&self) -> &Context<Note> {
            self.actor.strategy.context()
        }
    }

    /// A rig around an [`Echo`] with no gate.
    fn rig(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Observation<Note>>>,
        can_stop: HashMap<ActorId, CancellationToken>,
    ) -> Rig {
        rig_with(name, others, can_stop, |context| Echo {
            context,
            gate: None,
            slow_start: false,
            broken: false,
        })
    }

    /// A rig around the strategy `build` makes from its context.
    fn rig_with<S: Strategy<Message = Note, Payload = Note>>(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Observation<Note>>>,
        can_stop: HashMap<ActorId, CancellationToken>,
        build: impl FnOnce(Context<Note>) -> S,
    ) -> Rig<S> {
        let (sender, inbox) = unbounded_channel();
        let (ready, _) = oneshot::channel();
        let (start, started) = oneshot::channel();
        let stop = CancellationToken::new();
        let context = Context {
            id: id(name),
            outbox: Outbox {
                loopback: sender.clone(),
                others,
            },
            shutdown: Shutdown {
                mine: stop.clone(),
                others: can_stop,
            },
            log: None,
        };
        let actor = Actor {
            lifecycle: Idle,
            strategy: build(context),
            ready,
            start: started,
            inbox,
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
    async fn send_broadcasts_to_every_actor_it_may_send_to() {
        let (bob, mut bob_inbox) = unbounded_channel();
        let (cat, mut cat_inbox) = unbounded_channel();
        let ann = rig(
            "ann",
            HashMap::from([(id("bob"), bob), (id("cat"), cat)]),
            HashMap::new(),
        );

        ann.context().send(Note("hello")).unwrap();

        for inbox in [&mut bob_inbox, &mut cat_inbox] {
            let heard = inbox.recv().await.unwrap();
            assert!(
                matches!(heard, Observation::Broadcast(Note("hello"))),
                "{heard:?}"
            );
        }
    }

    #[tokio::test]
    async fn note_lands_in_the_actors_own_inbox() {
        let mut ann = rig("ann", HashMap::new(), HashMap::new());

        ann.context().note(Note("remember this")).unwrap();

        let heard = ann.actor.inbox.recv().await.unwrap();
        assert!(
            matches!(heard, Observation::Broadcast(Note("remember this"))),
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
    async fn request_fails_without_sending_if_a_recipient_may_not_be_addressed() {
        let (bob, mut bob_inbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());

        let error = ann
            .context()
            .request(Note("psst"), HashSet::from([id("bob"), id("zed")]))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("zed"), "{error}");
        assert!(bob_inbox.try_recv().is_err(), "nothing should reach bob");
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
        let (bob, bob_inbox) = unbounded_channel();
        let ann = rig("ann", HashMap::from([(id("bob"), bob)]), HashMap::new());
        drop(bob_inbox);

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
        ann.actor.strategy.context.log = Some(logger);

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
        rig.actor.strategy.gate = Some(Arc::clone(&gate));
        (rig, gate)
    }

    #[tokio::test(start_paused = true)]
    async fn a_kill_stops_an_actor_in_the_middle_of_a_step() {
        let (bob, _gate) = gated_rig("bob");
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        bob.sender
            .send(Observation::Broadcast(Note("take your time")))
            .unwrap();
        // Let Bob take the message and block in its policy.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let stopped = timeout(Duration::from_secs(5), running).await;
        stopped.expect("bob should stop").unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_kill_stops_an_actor_in_the_middle_of_starting() {
        let (mut bob, _gate) = gated_rig("bob");
        bob.actor.strategy.slow_start = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        // Let Bob take the start signal and block in its opening move.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let stopped = timeout(Duration::from_secs(5), running).await;
        stopped.expect("bob should stop").unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_strategy_with_no_opening_move_starts_and_waits() {
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
    async fn a_policy_that_fails_on_a_broadcast_fails_the_actor() {
        let mut bob = rig("bob", HashMap::new(), HashMap::new());
        bob.actor.strategy.broken = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();

        bob.sender
            .send(Observation::Broadcast(Note("hello")))
            .unwrap();

        let error = running.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("broken"), "{error}");
    }

    #[tokio::test]
    async fn a_policy_that_fails_on_a_request_fails_the_actor() {
        let mut bob = rig("bob", HashMap::new(), HashMap::new());
        bob.actor.strategy.broken = true;
        let running = tokio::spawn(bob.actor.run());
        bob.start.send(()).unwrap();
        let (request, _reply) = Request::new(Note("well?"));

        bob.sender.send(Observation::Request(request)).unwrap();

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
