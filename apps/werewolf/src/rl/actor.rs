use crate::rl::message::{Message, Request};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

pub(super) type ActorId = String;

pub(super) struct ActorInit<B: Behavior> {
    pub(super) behavior: B,
    pub(super) can_send_to: HashSet<ActorId>,
    pub(super) can_shut_down: HashSet<ActorId>,
}

pub(super) struct Actor<B: Behavior> {
    /// How this Actor handles startup and incoming Messages.
    pub(super) behavior: B,
    /// The channel on which this Actor receives incoming Messages.
    pub(super) inbox: UnboundedReceiver<Observation<B::Message>>,
    /// The channels on which this Actor sends Messages to other Actors.
    pub(super) outbox: Outbox<B::Message>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    pub(super) shutdown: Shutdown,
}

impl<B: Behavior> Actor<B> {
    async fn run(&mut self) -> anyhow::Result<()> {
        loop {
            let observation = tokio::select! {
                biased;
                _ = self.shutdown.mine.cancelled() => return Ok(()),
                observation = self.inbox.recv() => observation,
            };
            match observation {
                None => return Ok(()), // Receiver is closed.
                Some(observation) => match observation {
                    Observation::Broadcast(_broadcast) => {
                        todo!()
                    }
                    Observation::Request(request) => {
                        let action = self.behavior.policy(&request).await?;
                        request.reply(action).expect("failed to send reply");
                    }
                },
            }
        }
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

pub(super) trait Behavior {
    type Message: Message;
    async fn policy(
        &self,
        observation: &Request<Self::Message>,
    ) -> anyhow::Result<Vec<Self::Message>>;
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
