use crate::rl::actor::{Actor, ActorId, ActorInit, Behavior, Observation, Outbox, Shutdown};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

struct Episode<B: Behavior> {
    actors: HashMap<ActorId, Actor<B>>,
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
    fn run() -> anyhow::Result<()> {
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
