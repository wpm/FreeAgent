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
        // First pass: give every actor a channel and a shutdown token. The
        // behavior and the receiver are unique, so they stay together in one
        // map. The senders and tokens are clonable, so they go into lookup
        // tables that every outbox can copy.
        let mut senders = HashMap::new();
        let mut shutdowns = HashMap::new();
        let staged: HashMap<
            ActorId,
            (
                B,
                HashSet<ActorId>,
                UnboundedReceiver<Observation<B::Message>>,
            ),
        > = init
            .into_iter()
            .map(|(id, (behavior, peers))| {
                let (tx, rx) = unbounded_channel();
                senders.insert(id.clone(), tx);
                shutdowns.insert(id.clone(), CancellationToken::new());
                (id, (behavior, peers, rx))
            })
            .collect();
        // Second pass: now that the directory of senders is complete, build
        // each actor around its behavior and inbox.
        let actors = staged
            .into_iter()
            .map(|(id, (behavior, _peers, inbox))| {
                let loopback = senders[&id].clone();
                let outbox = Outbox {
                    loopback,
                    senders: senders.clone(),
                };
                let shutdown = shutdowns[&id].clone();
                (
                    id,
                    Actor {
                        behavior,
                        inbox,
                        outbox,
                        shutdown,
                        shutdowns: shutdowns.clone(),
                    },
                )
            })
            .collect();
        Self { actors }
    }
    fn run() -> Result<()> {
        todo!()
    }
}

struct Actor<B: Behavior> {
    /// How this Actor handles startup and incoming Messages.
    behavior: B,
    /// The channel on which this Actor receives incoming Messages.
    inbox: UnboundedReceiver<Observation<B::Message>>,
    /// The channels on which this Actor sends Messages to other Actors.
    outbox: Outbox<B::Message>,
    /// Token other Actors use to shut down this Actor..
    shutdown: CancellationToken,
    /// Tokens the Actor uses to shutdown other Actors.
    shutdowns: HashMap<ActorId, CancellationToken>,
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
    /// Channel on which an Actor sends a Message to itself.
    loopback: UnboundedSender<Observation<M>>,
    /// Channels on which an Actor sends Messages to other Actors.
    senders: HashMap<ActorId, UnboundedSender<Observation<M>>>,
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
