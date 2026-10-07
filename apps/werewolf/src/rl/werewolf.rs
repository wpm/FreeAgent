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
struct ActorInit<B: Behavior> {
    behavior: B,
    can_send_to: HashSet<ActorId>,
    can_shut_down: HashSet<ActorId>,
}
impl<B: Behavior> Episode<B> {
    fn new(init: HashMap<ActorId, ActorInit<B>>) -> Self {
        // First pass: give every actor a channel and a shutdown token. The
        // init and the receiver are unique, so they stay together in one
        // map. The senders and tokens are clonable, so they go into lookup
        // tables that each actor copies the permitted entries from.
        let mut senders = HashMap::new();
        let mut shutdowns = HashMap::new();
        let staged: HashMap<ActorId, (ActorInit<B>, UnboundedReceiver<Observation<B::Message>>)> =
            init.into_iter()
                .map(|(id, init)| {
                    let (tx, rx) = unbounded_channel();
                    senders.insert(id.clone(), tx);
                    shutdowns.insert(id.clone(), CancellationToken::new());
                    (id, (init, rx))
                })
                .collect();
        // Second pass: now that the directories are complete, build each
        // actor with only the senders and tokens its init allows.
        let actors = staged
            .into_iter()
            .map(|(id, (init, inbox))| {
                let outbox = Outbox {
                    loopback: senders[&id].clone(),
                    others: pick(&senders, &init.can_send_to),
                };
                let shutdown = Shutdown {
                    mine: shutdowns[&id].clone(),
                    others: pick(&shutdowns, &init.can_shut_down),
                };
                (
                    id,
                    Actor {
                        behavior: init.behavior,
                        inbox,
                        outbox,
                        shutdown,
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

/// The entries of `directory` named in `allowed`. Naming an actor that does
/// not exist is a mistake in the episode's init, so it panics.
fn pick<V: Clone>(
    directory: &HashMap<ActorId, V>,
    allowed: &HashSet<ActorId>,
) -> HashMap<ActorId, V> {
    allowed
        .iter()
        .map(|id| {
            let v = directory
                .get(id)
                .unwrap_or_else(|| panic!("init names unknown actor {id:?}"));
            (id.clone(), v.clone())
        })
        .collect()
}

struct Actor<B: Behavior> {
    /// How this Actor handles startup and incoming Messages.
    behavior: B,
    /// The channel on which this Actor receives incoming Messages.
    inbox: UnboundedReceiver<Observation<B::Message>>,
    /// The channels on which this Actor sends Messages to other Actors.
    outbox: Outbox<B::Message>,
    /// Tokens on which this Actor is shut down, and shuts down others.
    shutdown: Shutdown,
}

impl<B: Behavior> Actor<B> {
    async fn run(&mut self) -> Result<()> {
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
    others: HashMap<ActorId, UnboundedSender<Observation<M>>>,
}

struct Shutdown {
    /// Token other Actors cancel to shut this Actor down.
    mine: CancellationToken,
    /// Tokens this Actor cancels to shut down other Actors.
    others: HashMap<ActorId, CancellationToken>,
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
