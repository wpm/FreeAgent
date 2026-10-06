use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

type ActorId = String;
pub trait Message: Debug {}
struct Actor<B: Behavior> {
    id: ActorId,
    behavior: B,
    inbox: UnboundedReceiver<Observation<B::Message>>,
    outbox: Outbox<B::Message>,
    shutdown: CancellationToken,
}

impl<B: Behavior> Actor<B> {
    async fn run(&mut self) -> Result<()> {
        loop {
            let observation = tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => return Ok(()),
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

    fn send(&self, _message: B::Message) -> Result<()> {
        todo!()
    }

    async fn request(
        &self,
        _message: B::Message,
        _to: HashSet<ActorId>,
    ) -> Result<HashMap<ActorId, B::Message>> {
        todo!()
    }
}

trait Behavior {
    type Message: Message;
    async fn policy(&self, observation: &Request<Self::Message>) -> Result<Vec<Self::Message>>;
}

struct Outbox<M: Message> {
    loopback: UnboundedSender<Action<M>>,
    sender: HashMap<ActorId, UnboundedSender<Action<M>>>,
    shutdown: HashMap<ActorId, CancellationToken>,
}

/// Messages an [`Actor`](Actor) receives.
#[derive(Debug)]
enum Observation<M: Message> {
    Broadcast(M),
    Request(Request<M>),
}

/// Messages an [`Actor`](Actor) sends.
#[derive(Debug)]
enum Action<M: Message> {
    Broadcast(M),
    Reply(Reply<M>),
}

type RequestId = u64;

#[derive(Debug)]
struct Request<M: Message> {
    id: RequestId,
    message: M,
    reply_to: oneshot::Sender<Reply<M>>,
}

impl<M: Message> Request<M> {
    fn reply(self, messages: Vec<M>) -> Result<()> {
        self.reply_to
            .send(Reply {
                id: self.id,
                messages,
            })
            .ok()
            .context("the asker stopped waiting")
    }
}

#[derive(Debug)]
struct Reply<M: Message> {
    id: RequestId,
    messages: Vec<M>,
}
