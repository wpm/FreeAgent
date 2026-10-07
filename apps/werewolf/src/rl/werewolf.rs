use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

type ActorId = String;
trait Message: Debug {}

struct Episode<B: Behavior> {
    actors: HashMap<ActorId, Actor<B>>,
}
impl<B: Behavior> Episode<B> {
    fn new(init: HashMap<ActorId, (B, HashSet<ActorId>)>) -> Self {
        let (behavior, topology): (HashMap<ActorId, B>, HashMap<ActorId, HashSet<ActorId>>) = init
            .into_iter()
            .map(|(id, (b, peers))| ((id.clone(), b), (id, peers)))
            .unzip();
        let (senders, receivers): (
            HashMap<ActorId, UnboundedSender<Observation<B::Message>>>,
            HashMap<ActorId, UnboundedReceiver<Observation<B::Message>>>,
        ) = behavior
            .keys()
            .map(|id| {
                let (sender, receiver) = unbounded_channel();
                ((id.clone(), sender), (id.clone(), receiver))
            })
            .unzip();
        todo!()
    }
    fn run() -> Result<()> {
        todo!()
    }
}

struct Actor<B: Behavior> {
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
    loopback: UnboundedSender<Observation<M>>,
    senders: HashMap<ActorId, UnboundedSender<Observation<M>>>,
    shutdown: HashMap<ActorId, CancellationToken>,
}

/// Messages an [`Actor`](Actor) receives.
#[derive(Debug)]
enum Observation<M: Message> {
    Broadcast(M),
    Request(Request<M>),
}

/// Messages an [`Actor`](Actor) sends. A reply is not one of these: it goes
/// straight back down the request's own channel.
#[derive(Debug)]
enum Action<M: Message> {
    Broadcast(M),
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
