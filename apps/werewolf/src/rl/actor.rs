use crate::rl::message::{Message, Request};
use anyhow::Context;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) type ActorId = String;

pub(super) struct ActorInit<L: Lifecycle, B: Behavior> {
    pub(super) lifecycle: L,
    pub(super) behavior: B,
    pub(super) can_send_to: HashSet<ActorId>,
    pub(super) can_shut_down: HashSet<ActorId>,
}

pub(super) struct Actor<L: Lifecycle, B: Behavior> {
    /// How this Actor handles startup and shutdown.
    pub(super) lifecycle: L,
    /// How this Actor handles incoming Messages.
    pub(super) behavior: B,
    /// The episode's one-time signal that every actor is running.
    pub(super) start: oneshot::Receiver<()>,
    /// The channel on which this Actor receives incoming Messages.
    pub(super) inbox: UnboundedReceiver<Observation<B::Message>>,
    /// The channels on which this Actor sends Messages to other Actors.
    pub(super) outbox: Outbox<B::Message>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    pub(super) shutdown: Shutdown,
}

impl<L: Lifecycle, B: Behavior> Actor<L, B> {
    /// Initialize, then handle the start signal and observations until shut
    /// down or until every sender to this actor's inbox is gone, then clean
    /// up.
    pub(super) async fn run(mut self) -> anyhow::Result<()> {
        self.lifecycle.initialize().await?;
        // A oneshot receiver panics if polled after it completes, so it is
        // taken out of the select once it has fired.
        let mut start = Some(self.start);
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.mine.cancelled() => break,
                started = async { start.as_mut().expect("guarded by the branch condition").await },
                    if start.is_some() =>
                {
                    start = None;
                    match started {
                        Ok(()) => self.behavior.start().await?,
                        Err(_) => break, // The episode is gone.
                    }
                }
                observation = self.inbox.recv() => match observation {
                    None => break, // Every sender is gone.
                    Some(Observation::Broadcast(message)) => {
                        self.behavior.policy(&message).await?;
                    }
                    Some(Observation::Request(request)) => {
                        let action = self.behavior.policy(request.message()).await?;
                        request.reply(action).expect("failed to send reply");
                    }
                },
            }
        }
        self.lifecycle.clean_up().await?;
        Ok(())
    }

    /// Broadcast `message` to every actor this one may send to. An actor
    /// that has already stopped is skipped.
    pub(super) fn send(&self, message: B::Message) -> anyhow::Result<()> {
        for sender in self.outbox.others.values() {
            // A failed send means the recipient's inbox is gone.
            let _ = sender.send(Observation::Broadcast(message.clone()));
        }
        Ok(())
    }

    /// Ask every actor in `to` the same thing and collect their replies. A
    /// recipient that has stopped, before or after receiving the request,
    /// is left out of the result.
    ///
    /// # Errors
    ///
    /// Fails, sending nothing, if any recipient is not an actor this one
    /// may send to.
    pub(super) async fn request(
        &self,
        message: B::Message,
        to: HashSet<ActorId>,
    ) -> anyhow::Result<HashMap<ActorId, Vec<B::Message>>> {
        let senders = to
            .iter()
            .map(|id| {
                self.outbox.others.get_key_value(id).with_context(|| {
                    format!("cannot request from {id}: not an actor it may send to")
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
}

/// Actors run on a multi-threaded runtime, so a behavior has to be shareable
/// across threads.
#[async_trait]
pub(super) trait Behavior: Send + Sync {
    type Message: Message;
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

/// Messages an [`Actor`](Actor) sends. A reply is not one of these: it goes
/// straight back down the request's own channel.
#[derive(Debug)]
enum Action<M: Message> {
    Broadcast(M),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[derive(Debug, Clone, PartialEq)]
    struct Note(&'static str);
    impl Message for Note {}

    struct Idle;
    #[async_trait]
    impl Lifecycle for Idle {}

    /// A behavior that answers every request with the message it was sent.
    struct Echo;
    #[async_trait]
    impl Behavior for Echo {
        type Message = Note;
        async fn policy(&self, observation: &Note) -> anyhow::Result<Vec<Note>> {
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

    fn rig(others: HashMap<ActorId, UnboundedSender<Observation<Note>>>) -> Rig {
        let (sender, inbox) = unbounded_channel();
        let (start, started) = oneshot::channel();
        let stop = CancellationToken::new();
        let actor = Actor {
            lifecycle: Idle,
            behavior: Echo,
            start: started,
            inbox,
            outbox: Outbox {
                loopback: sender.clone(),
                others,
            },
            shutdown: Shutdown {
                mine: stop.clone(),
                others: HashMap::new(),
            },
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
        let ann = rig(HashMap::from([(id("bob"), bob), (id("cat"), cat)]));

        ann.actor.send(Note("hello")).unwrap();

        for inbox in [&mut bob_inbox, &mut cat_inbox] {
            let heard = inbox.recv().await.unwrap();
            assert!(
                matches!(heard, Observation::Broadcast(Note("hello"))),
                "{heard:?}"
            );
        }
    }

    #[tokio::test]
    async fn request_collects_a_reply_from_every_recipient() {
        let bob = rig(HashMap::new());
        let cat = rig(HashMap::new());
        let ann = rig(HashMap::from([
            (id("bob"), bob.sender.clone()),
            (id("cat"), cat.sender.clone()),
        ]));
        let bob_running = tokio::spawn(bob.actor.run());
        let cat_running = tokio::spawn(cat.actor.run());
        bob.start.send(()).unwrap();
        cat.start.send(()).unwrap();

        let replies = ann
            .actor
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
        let ann = rig(HashMap::from([(id("bob"), bob)]));

        let error = ann
            .actor
            .request(Note("psst"), HashSet::from([id("bob"), id("zed")]))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("zed"), "{error}");
        assert!(bob_inbox.try_recv().is_err(), "nothing should reach bob");
    }

    #[tokio::test]
    async fn request_leaves_out_a_recipient_that_has_stopped() {
        let (bob, bob_inbox) = unbounded_channel();
        let ann = rig(HashMap::from([(id("bob"), bob)]));
        drop(bob_inbox);

        let replies = ann
            .actor
            .request(Note("anyone?"), HashSet::from([id("bob")]))
            .await
            .unwrap();

        assert!(replies.is_empty(), "{replies:?}");
    }
}
