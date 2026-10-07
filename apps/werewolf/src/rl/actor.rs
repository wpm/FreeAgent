use crate::rl::message::{Message, Request};
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

    fn send(&self, _message: B::Message) -> anyhow::Result<()> {
        todo!()
    }

    async fn request(
        &self,
        _message: B::Message,
        _to: HashSet<ActorId>,
    ) -> anyhow::Result<HashMap<ActorId, B::Message>> {
        todo!()
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
