use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

type ActorId = String;
trait Message: Debug {}

fn episode<B: Behavior>(behaviors: HashMap<ActorId, (B, HashSet<ActorId>)>) -> Result<()> {
    let channels: HashMap<
        ActorId,
        (
            UnboundedSender<Observation<B::Message>>,
            UnboundedReceiver<Observation<B::Message>>,
        ),
    > = behaviors
        .keys()
        .map(|actor_id| (actor_id.clone(), unbounded_channel()))
        .collect();
    // One directory of senders, shared by everyone. Each actor's outbox is a
    // clone of it, so an actor can message itself like any other peer.
    let directory: HashMap<ActorId, UnboundedSender<Observation<B::Message>>> = channels
        .iter()
        .map(|(actor_id, (sender, _))| (actor_id.clone(), sender.clone()))
        .collect();
    let senders: HashMap<ActorId, HashMap<ActorId, UnboundedSender<Observation<B::Message>>>> =
        channels
            .keys()
            .map(|actor_id| (actor_id.clone(), directory.clone()))
            .collect();
    Ok(())
}

// struct Outbox<M: Message> {
//     senders: HashMap<ActorId, UnboundedSender<Observation<M>>>,
//     shutdown: HashMap<ActorId, CancellationToken>,
// }

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
