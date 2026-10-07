use crate::rl::log::{Event, Logger};
use crate::rl::message::{Message, Request};
use anyhow::{Context as _, bail};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) type ActorId = String;

pub(super) struct ActorInit<L: Lifecycle, B: Behavior> {
    pub(super) lifecycle: L,
    /// Builds the behavior once its [`Context`] exists, which is not until
    /// the episode has opened every actor's channels.
    pub(super) behavior: Box<dyn FnOnce(Context<B::Message, B::Payload>) -> B + Send>,
    pub(super) can_send_to: HashSet<ActorId>,
    pub(super) can_shut_down: HashSet<ActorId>,
    /// Whether this actor gets a copy of the episode's logger.
    pub(super) has_logger: bool,
}

pub(super) struct Actor<L: Lifecycle, B: Behavior> {
    /// How this Actor handles startup and shutdown.
    pub(super) lifecycle: L,
    /// How this Actor handles incoming Messages. It owns the [`Context`]
    /// through which it reaches other actors.
    pub(super) behavior: B,
    /// This Actor's one-time signal to the episode that it is initialized
    /// and running its message loop.
    pub(super) ready: oneshot::Sender<()>,
    /// The episode's one-time signal that every actor is running.
    pub(super) start: oneshot::Receiver<()>,
    /// The channel on which this Actor receives incoming Messages.
    pub(super) inbox: UnboundedReceiver<Observation<B::Message>>,
}

impl<L: Lifecycle, B: Behavior> Actor<L, B> {
    /// Initialize, then handle the start signal and observations until shut
    /// down or until every sender to this actor's inbox is gone, then clean
    /// up.
    ///
    /// Shutdown takes effect at once. A step in progress is abandoned at its
    /// next await, and nothing else waiting in the inbox is taken in. An
    /// actor shut down while still initializing stops without cleaning up,
    /// since there is no telling how far initialization got.
    pub(super) async fn run(mut self) -> anyhow::Result<()> {
        let shutdown = self.behavior.context().shutdown.mine.clone();
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
                            let Some(opened) = unless_stopped(&shutdown, self.behavior.start()).await else {
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
                        let step = self.behavior.policy(&message);
                        let Some(acted) = unless_stopped(&shutdown, step).await else {
                            break;
                        };
                        acted?;
                    }
                    Some(Observation::Request(request)) => {
                        let step = self.behavior.policy(request.message());
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

/// The ways out of an [`Actor`]: what a [`Behavior`] may do besides answer.
///
/// A behavior owns one of these and nothing else of the actor. In particular
/// it cannot reach the inbox, so it cannot take the next message out of
/// turn.
///
/// `P` is what this actor logs. It defaults to the message type, which is
/// what most actors log.
pub(super) struct Context<M: Message, P = M> {
    /// This actor's name, as the other actors know it.
    pub(super) id: ActorId,
    /// The channels on which this Actor sends Messages.
    pub(super) outbox: Outbox<M>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    pub(super) shutdown: Shutdown,
    /// Where this Actor's events go, if it has a logger at all.
    pub(super) log: Option<Logger<P>>,
}

impl<M: Message, P> Context<M, P> {
    /// Log `payload` as an event stamped now. Nothing happens if this actor
    /// has no logger, or if the log has stopped listening. Logging never
    /// fails an actor.
    pub(super) fn log(&self, payload: P) {
        if let Some(log) = &self.log {
            let _ = log.send(Event::now(payload));
        }
    }

    /// Broadcast `message` to every actor this one may send to. An actor
    /// that has already stopped is skipped.
    pub(super) fn send(&self, message: M) -> anyhow::Result<()> {
        for sender in self.outbox.others.values() {
            // A failed send means the recipient's inbox is gone.
            let _ = sender.send(Observation::Broadcast(message.clone()));
        }
        Ok(())
    }

    /// Leave `message` in this actor's own inbox, to be handled in a later
    /// step. This is fire and forget: the only way an actor may message
    /// itself, since it cannot take a step while it is waiting for one to
    /// finish.
    pub(super) fn note(&self, message: M) -> anyhow::Result<()> {
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
    /// Fails, sending nothing, if any recipient is not an actor this one
    /// may send to, or is this actor itself, which could never answer.
    pub(super) async fn request(
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

    /// Stop another actor, without its cooperation.
    ///
    /// # Errors
    ///
    /// Fails if `who` is not an actor this one may shut down.
    pub(super) fn stop(&self, who: &ActorId) -> anyhow::Result<()> {
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
    /// then stops without taking anything else from its inbox.
    pub(super) fn shutdown(&self) {
        self.shutdown.mine.cancel();
    }
}

/// How an actor maps what it observes to what it does. The signature of
/// [`policy`](Behavior::policy) is the reinforcement-learning one: an
/// observation in, actions out. Anything else the behavior wants to say,
/// such as who it is, goes inside its messages.
///
/// Actors run on a multi-threaded runtime, so a behavior has to be shareable
/// across threads.
#[async_trait]
pub(super) trait Behavior: Send + Sync {
    type Message: Message;
    /// What this behavior logs. Most often the message type.
    type Payload: Send + 'static;
    /// The ways out of this actor, handed to the behavior when it was built.
    fn context(&self) -> &Context<Self::Message, Self::Payload>;
    /// Handle `observation`. The result is the reply if the observation was
    /// a request, and is ignored if it was a broadcast.
    async fn policy(&self, observation: &Self::Message) -> anyhow::Result<Vec<Self::Message>>;
    /// Called once every actor in the episode is running. This is where an
    /// actor with an opening move makes it.
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Actors run on a multi-threaded runtime, so a lifecycle has to be
/// shareable across threads.
#[async_trait]
pub(super) trait Lifecycle: Send + Sync {
    /// Called just before the message handling loop begins.
    async fn initialize(&self) -> anyhow::Result<()> {
        Ok(())
    }
    /// Called just before the message handling loop exits.
    async fn clean_up(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) struct Outbox<M: Message> {
    /// Channel on which an Actor sends a Message to itself.
    pub(super) loopback: UnboundedSender<Observation<M>>,
    /// Channels on which an Actor sends Messages to other Actors.
    pub(super) others: HashMap<ActorId, UnboundedSender<Observation<M>>>,
}

pub(super) struct Shutdown {
    /// Token other Actors cancel to shut this Actor down.
    pub(super) mine: CancellationToken,
    /// Tokens this Actor cancels to shut down other Actors.
    pub(super) others: HashMap<ActorId, CancellationToken>,
}

/// Messages an [`Actor`](Actor) receives.
#[derive(Debug)]
pub(super) enum Observation<M: Message> {
    Broadcast(M),
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

    /// A behavior that answers every request with the message it was sent,
    /// after waiting for a permit from its gate if it has one.
    struct Echo {
        context: Context<Note>,
        gate: Option<Arc<Semaphore>>,
    }
    #[async_trait]
    impl Behavior for Echo {
        type Message = Note;
        type Payload = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn policy(&self, observation: &Note) -> anyhow::Result<Vec<Note>> {
            if let Some(gate) = &self.gate {
                gate.acquire().await?.forget();
            }
            Ok(vec![observation.clone()])
        }
    }

    /// An actor, along with what a test needs to feed it, start it, and stop
    /// it from the outside.
    struct Rig {
        actor: Actor<Idle, Echo>,
        sender: UnboundedSender<Observation<Note>>,
        start: oneshot::Sender<()>,
        stop: CancellationToken,
    }

    impl Rig {
        fn context(&self) -> &Context<Note> {
            self.actor.behavior.context()
        }
    }

    fn rig(
        name: &str,
        others: HashMap<ActorId, UnboundedSender<Observation<Note>>>,
        can_stop: HashMap<ActorId, CancellationToken>,
    ) -> Rig {
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
            behavior: Echo {
                context,
                gate: None,
            },
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
            .send(Observation::Broadcast(Note("take your time")))
            .unwrap();
        // Let Bob take the message and block in its policy.
        sleep(Duration::from_secs(1)).await;

        bob.stop.cancel();

        let stopped = timeout(Duration::from_secs(5), running).await;
        stopped.expect("bob should stop").unwrap().unwrap();
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
