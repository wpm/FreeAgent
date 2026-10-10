//! An episode brings a set of actors into being together, wires them to
//! one another as their inits allow, and runs them until every one has
//! stopped.

use crate::actor::{Actor, ActorId, ActorInit, Behavior, Context, Envelope, Shutdown, ThinkLoop};
use crate::log::Logger;
use anyhow::{Context as _, bail};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A set of actors brought into being together, wired to one another as
/// their inits allow, and run until every one of them has stopped.
pub struct Episode<B: Behavior> {
    actors: HashMap<ActorId, Actor<B>>,
    /// One-time signals from each actor that it is running its message loop.
    readies: HashMap<ActorId, oneshot::Receiver<()>>,
    /// One-time signals telling each actor that every actor is running.
    starts: HashMap<ActorId, oneshot::Sender<()>>,
}

impl<B: Behavior> Episode<B> {
    /// The actors described by `init`, wired to one another as it allows.
    /// Those with `has_logger` get a copy of `logger`. An actor with a think
    /// loop gets a second context for it, with everything the first has.
    ///
    /// # Panics
    ///
    /// Panics when an init's `can_send_to` or `can_shut_down` names an actor
    /// missing from `init`, or its `can_send_to` names the actor itself. An
    /// episode's wiring is checked when it is built.
    pub fn new(init: HashMap<ActorId, ActorInit<B>>, logger: Logger<B::Log>) -> Self {
        // First pass: give every actor a channel and a shutdown token. The
        // init and the receiver are unique, so they stay together in one
        // map. The senders and tokens are clonable, so they go into lookup
        // tables that each actor copies the permitted entries from.
        let episode = Uuid::new_v4();
        let mut senders = HashMap::new();
        let mut shutdowns = HashMap::new();
        let staged: Staged<B> = init
            .into_iter()
            .map(|(id, init)| {
                let (tx, rx) = unbounded_channel();
                senders.insert(id.clone(), tx);
                shutdowns.insert(id.clone(), CancellationToken::new());
                (id, (init, rx))
            })
            .collect();
        // Second pass: now that the directories are complete, build each
        // actor with only the senders and tokens its init allows.
        let mut readies = HashMap::new();
        let mut starts = HashMap::new();
        let actors = staged
            .into_iter()
            .map(|(id, (init, mailbox))| {
                let (ready, is_ready) = oneshot::channel();
                readies.insert(id.clone(), is_ready);
                let (start, started) = oneshot::channel();
                starts.insert(id.clone(), start);
                assert!(
                    !init.can_send_to.contains(&id),
                    "init of {id:?} names itself"
                );
                let mut context = Context {
                    id: id.clone(),
                    episode,
                    mailboxes: pick(&senders, &init.can_send_to),
                    shutdown: Shutdown {
                        mine: shutdowns[&id].clone(),
                        others: pick(&shutdowns, &init.can_shut_down),
                    },
                    log: init.has_logger.then(|| logger.clone()),
                    thoughts: None,
                };
                let think = init.think.map(|build| ThinkLoop::open(build, &mut context));
                (
                    id,
                    Actor {
                        behavior: (init.behavior)(context),
                        think,
                        ready: Some(ready),
                        start: started,
                        mailbox,
                    },
                )
            })
            .collect();
        Self {
            actors,
            readies,
            starts,
        }
    }

    /// Spawn every actor, wait until all of them are initialized and running
    /// their message loops, then tell each one to start, and wait for all of
    /// them to finish. Once `patience` runs out, shut down every actor still
    /// running and fail.
    ///
    /// Every actor is ready before any actor starts, so an opening move
    /// always lands on an actor that is running.
    ///
    /// # Errors
    ///
    /// The first actor to fail shuts the others down, and its error is the
    /// episode's. An actor that fails to initialize fails the episode before
    /// any actor starts. Running out of patience is an error too.
    pub async fn run(self, patience: Duration) -> anyhow::Result<()>
    where
        B: 'static,
    {
        let stops: Vec<_> = self
            .actors
            .values()
            .map(|actor| actor.behavior.context().shutdown.mine.clone())
            .collect();
        let mut tasks = JoinSet::new();
        for (id, actor) in self.actors {
            tasks.spawn(async move {
                actor
                    .run()
                    .await
                    .with_context(|| format!("actor {id} failed"))
            });
        }
        let episode = async {
            if all_ready(self.readies).await {
                for start in self.starts.into_values() {
                    // An actor that has stopped since reporting ready has
                    // dropped its receiver. Whatever stopped it surfaces
                    // when its task is joined.
                    let _ = start.send(());
                }
            } else {
                // An actor failed to initialize. Its error surfaces when its
                // task is joined. Nobody starts.
                for stop in &stops {
                    stop.cancel();
                }
            }
            wait_for_all(&mut tasks, &stops).await
        };
        match timeout(patience, episode).await {
            Ok(outcome) => outcome,
            Err(_) => {
                for stop in &stops {
                    stop.cancel();
                }
                wait_for_all(&mut tasks, &stops).await?;
                bail!("ran out of patience after {patience:?}");
            }
        }
    }
}

/// Each actor's init, together with the receiving end of its mailbox, between
/// the two passes of [`Episode::new`].
type Staged<B> = HashMap<
    ActorId,
    (
        ActorInit<B>,
        UnboundedReceiver<Envelope<<B as Behavior>::Message>>,
    ),
>;

/// Wait for every actor to report that it is ready. False once one of them
/// drops its signal, which it does when it fails to initialize.
async fn all_ready(readies: HashMap<ActorId, oneshot::Receiver<()>>) -> bool {
    for ready in readies.into_values() {
        if ready.await.is_err() {
            return false;
        }
    }
    true
}

/// Wait for every task to finish. The first failure shuts the remaining
/// actors down, so the episode ends as a whole, and is the error returned.
async fn wait_for_all(
    tasks: &mut JoinSet<anyhow::Result<()>>,
    stops: &[CancellationToken],
) -> anyhow::Result<()> {
    let mut first_failure = None;
    while let Some(outcome) = tasks.join_next().await {
        let outcome = outcome.context("an actor panicked").and_then(|ran| ran);
        if let Err(failure) = outcome {
            for stop in stops {
                stop.cancel();
            }
            first_failure.get_or_insert(failure);
        }
    }
    first_failure.map_or(Ok(()), Err)
}

/// The entries of `directory` named in `allowed`. Panics when `allowed`
/// names an actor missing from `directory`, since the episode's wiring is
/// checked when it is built.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{Builder, Think, ThinkBuilder};
    use crate::log::Event;
    use crate::message::Message;
    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc::UnboundedSender;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Note;
    impl Message for Note {}

    /// How a [`Reporter`] initializes. Left as default, it initializes at
    /// once.
    #[derive(Default)]
    struct Initialization {
        /// Initialization waits for a permit from here, if present.
        gate: Option<Arc<Semaphore>>,
        /// Initialization fails.
        broken: bool,
    }

    /// A behavior that reports when it is started, both to the test and to
    /// the log, hands a note to its think loop if it has one, fails to
    /// start if told to, and is otherwise silent.
    struct Reporter {
        context: Context<Note, ActorId>,
        initialization: Initialization,
        started: UnboundedSender<ActorId>,
        fails: bool,
    }
    #[async_trait]
    impl Behavior for Reporter {
        type Message = Note;
        /// A reporter logs its own name.
        type Log = ActorId;
        fn context(&self) -> &Context<Note, ActorId> {
            &self.context
        }
        async fn initialize(&mut self) -> anyhow::Result<()> {
            if self.initialization.broken {
                bail!("cannot initialize");
            }
            if let Some(gate) = &self.initialization.gate {
                gate.acquire().await?.forget();
            }
            Ok(())
        }
        async fn start(&mut self) -> anyhow::Result<()> {
            if self.fails {
                bail!("{} refuses to start", self.context.id);
            }
            self.context.log(self.context.id.clone());
            self.started.send(self.context.id.clone())?;
            // Only an actor with a think loop can.
            let _ = self.context.think(Note);
            Ok(())
        }
    }

    /// Builds a reporter that announces on `started`, initializes as
    /// `initialization` says, and refuses to start if `fails`.
    fn reporter(
        started: UnboundedSender<ActorId>,
        initialization: Initialization,
        fails: bool,
    ) -> Builder<Reporter> {
        Box::new(move |context| Reporter {
            context,
            initialization,
            started,
            fails,
        })
    }

    /// An episode under test, with the channels the test watches it through.
    struct Stage {
        episode: Episode<Reporter>,
        /// Every reporter announces here that it was started.
        starts: UnboundedReceiver<ActorId>,
        /// The episode's log.
        log: UnboundedReceiver<Event<ActorId>>,
    }

    /// An episode of [`Reporter`]s named `ids`, those in `failing` set to
    /// refuse to start and those in `logging` holding the episode's logger.
    /// Each actor is wired to no one and initializes as
    /// `initialization(id)` says.
    fn episode_with(
        ids: &[&str],
        failing: &[&str],
        logging: &[&str],
        initialization: impl Fn(&str) -> Initialization,
    ) -> Stage {
        let (started, starts) = unbounded_channel();
        let (logger, log) = unbounded_channel();
        let init = ids
            .iter()
            .map(|id| {
                let init = ActorInit {
                    behavior: reporter(started.clone(), initialization(id), failing.contains(id)),
                    think: None,
                    can_send_to: HashSet::new(),
                    can_shut_down: HashSet::new(),
                    has_logger: logging.contains(id),
                };
                (id.to_string(), init)
            })
            .collect();
        Stage {
            episode: Episode::new(init, logger),
            starts,
            log,
        }
    }

    /// [`episode_with`] where every actor initializes at once and the logger
    /// stays with the test.
    fn episode_of(ids: &[&str], failing: &[&str]) -> Stage {
        episode_with(ids, failing, &[], |_| Initialization::default())
    }

    /// An episode of Ann and Bob in which Bob's initialization waits for a
    /// permit from the returned gate.
    fn episode_with_bob_held_up() -> (Stage, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        let slow = Arc::clone(&gate);
        let stage = episode_with(&["ann", "bob"], &[], &[], move |id| Initialization {
            gate: (id == "bob").then(|| Arc::clone(&slow)),
            broken: false,
        });
        (stage, gate)
    }

    /// The next two actors to announce themselves on `starts`.
    async fn two_starts(starts: &mut UnboundedReceiver<ActorId>) -> HashSet<ActorId> {
        let mut started = HashSet::new();
        started.insert(starts.recv().await.unwrap());
        started.insert(starts.recv().await.unwrap());
        started
    }

    fn ann_and_bob() -> HashSet<ActorId> {
        HashSet::from(["ann".to_string(), "bob".to_string()])
    }

    /// An episode of reporters in which each actor may send to and shut
    /// down the actors listed beside its name.
    fn wired(links: &[(&str, &[&str])]) -> Episode<Reporter> {
        let (started, _) = unbounded_channel();
        let (logger, _) = unbounded_channel();
        let init = links
            .iter()
            .map(|(id, others)| {
                let others: HashSet<ActorId> = others.iter().map(|o| o.to_string()).collect();
                let init = ActorInit {
                    behavior: reporter(started.clone(), Initialization::default(), false),
                    think: None,
                    can_send_to: others.clone(),
                    can_shut_down: others,
                    has_logger: false,
                };
                (id.to_string(), init)
            })
            .collect();
        Episode::new(init, logger)
    }

    /// A think loop that ends the episode with its first thought, stopping
    /// the one actor it may and then itself, and reports each thought on
    /// `alive`, which closes when the loop is gone.
    struct Closer {
        context: Context<Note, ActorId>,
        alive: UnboundedSender<()>,
    }
    #[async_trait]
    impl Think for Closer {
        type Message = Note;
        async fn think(&mut self, _message: &Note) -> anyhow::Result<()> {
            for other in self.context.shutdown.others.keys() {
                self.context.stop(other)?;
            }
            self.context.shutdown();
            self.alive.send(())?;
            Ok(())
        }
    }

    /// An episode in which Ann thinks, with what the test watches it through.
    struct Thinking {
        episode: Episode<Reporter>,
        /// A twin of the context Ann's think loop was built from.
        context: UnboundedReceiver<Context<Note, ActorId>>,
        /// Ann's think loop reports each thought here, and the channel
        /// closes when the loop is gone.
        thoughts: UnboundedReceiver<()>,
        /// Every reporter announces here that it was started.
        _starts: UnboundedReceiver<ActorId>,
    }

    /// Ann and Bob wired to each other, Ann with a [`Closer`] think loop
    /// and the logger.
    fn with_ann_thinking() -> Thinking {
        let (started, starts) = unbounded_channel();
        let (logger, _) = unbounded_channel();
        let (alive, ended) = unbounded_channel();
        let (seen, contexts) = unbounded_channel();
        let init = [("ann", "bob"), ("bob", "ann")]
            .into_iter()
            .map(|(id, other)| {
                let think = (id == "ann").then(|| {
                    let alive = alive.clone();
                    let seen = seen.clone();
                    let build: ThinkBuilder<Note, ActorId> = Box::new(move |context| {
                        seen.send(context.twin()).unwrap();
                        Box::new(Closer { context, alive })
                    });
                    build
                });
                let init = ActorInit {
                    behavior: reporter(started.clone(), Initialization::default(), false),
                    think,
                    can_send_to: HashSet::from([other.to_string()]),
                    can_shut_down: HashSet::from([other.to_string()]),
                    has_logger: id == "ann",
                };
                (id.to_string(), init)
            })
            .collect();
        Thinking {
            episode: Episode::new(init, logger),
            context: contexts,
            thoughts: ended,
            _starts: starts,
        }
    }

    fn stops<B: Behavior>(episode: &Episode<B>) -> Vec<CancellationToken> {
        episode
            .actors
            .values()
            .map(|actor| actor.behavior.context().shutdown.mine.clone())
            .collect()
    }

    #[test]
    fn new_wires_each_actor_to_the_actors_its_init_names() {
        let episode = wired(&[("ann", &["bob"]), ("bob", &[])]);

        let ann = episode.actors["ann"].behavior.context();
        let reaches: Vec<_> = ann.mailboxes.keys().cloned().collect();
        let stops: Vec<_> = ann.shutdown.others.keys().cloned().collect();
        assert_eq!(reaches, ["bob"]);
        assert_eq!(stops, ["bob"]);
        let bob = episode.actors["bob"].behavior.context();
        assert!(bob.mailboxes.is_empty());
        assert!(bob.shutdown.others.is_empty());
    }

    #[test]
    fn new_gives_a_think_loop_a_context_like_its_perceive_loops() {
        let Thinking {
            episode,
            mut context,
            ..
        } = with_ann_thinking();

        let perceive = episode.actors["ann"].behavior.context();
        let think = context.try_recv().unwrap();
        assert_eq!(think.id, perceive.id);
        assert_eq!(think.episode, perceive.episode);
        let reaches: Vec<_> = think.mailboxes.keys().cloned().collect();
        assert_eq!(reaches, ["bob"]);
        let stops: Vec<_> = think.shutdown.others.keys().cloned().collect();
        assert_eq!(stops, ["bob"]);
        assert!(think.log.is_some(), "ann's think loop should log");
        assert!(think.thoughts.is_some(), "ann's think loop should think");
        assert!(perceive.thoughts.is_some(), "ann should think");
        assert!(
            episode.actors["bob"].behavior.context().thoughts.is_none(),
            "bob has no think loop"
        );
        // One shutdown stops both loops.
        perceive.shutdown.mine.cancel();
        assert!(think.shutdown.mine.is_cancelled());
    }

    #[tokio::test]
    async fn run_returns_only_after_every_think_loop_has_ended() {
        // The reporters announce on `starts`, so it has to stay open.
        let Thinking {
            episode,
            mut thoughts,
            _starts,
            ..
        } = with_ann_thinking();

        // Ann's think loop ends the episode with its first thought.
        episode.run(Duration::from_secs(60)).await.unwrap();

        assert_eq!(thoughts.recv().await, Some(()));
        // The think loop, which held the other end, is gone.
        assert_eq!(thoughts.recv().await, None);
    }

    #[test]
    #[should_panic(expected = "unknown actor \"zed\"")]
    fn new_panics_when_an_init_names_an_unknown_actor() {
        wired(&[("ann", &["zed"])]);
    }

    #[test]
    #[should_panic(expected = "\"ann\" names itself")]
    fn new_panics_when_an_init_names_the_actor_itself() {
        wired(&[("ann", &["ann"])]);
    }

    #[tokio::test]
    async fn run_starts_every_actor_then_waits_for_them_to_finish() {
        let Stage {
            episode,
            mut starts,
            ..
        } = episode_of(&["ann", "bob"], &[]);
        let stops = stops(&episode);
        let running = tokio::spawn(episode.run(Duration::from_secs(60)));

        assert_eq!(two_starts(&mut starts).await, ann_and_bob());

        for stop in stops {
            stop.cancel();
        }
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn run_gives_up_after_patience() {
        // The reporters announce on `starts`, so it has to stay open.
        let Stage {
            episode,
            starts: _starts,
            ..
        } = episode_of(&["ann", "bob"], &[]);
        let error = episode.run(Duration::from_secs(5)).await.unwrap_err();
        assert!(error.to_string().contains("patience"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_actor_ends_the_episode_with_its_error() {
        // The reporters announce on `starts`, so it has to stay open.
        let Stage {
            episode,
            starts: _starts,
            ..
        } = episode_of(&["ann", "bob"], &["bob"]);
        let error = episode.run(Duration::from_secs(60)).await.unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("actor bob failed"), "{text}");
        assert!(text.contains("bob refuses to start"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn no_actor_starts_until_every_actor_has_initialized() {
        let (
            Stage {
                episode,
                mut starts,
                ..
            },
            gate,
        ) = episode_with_bob_held_up();
        let stops = stops(&episode);
        let running = tokio::spawn(episode.run(Duration::from_secs(60)));

        // Ann is ready at once, but with Bob still initializing nothing
        // happens, not even after the runtime has gone idle.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(starts.try_recv().is_err(), "nobody should have started");

        gate.add_permits(1);
        assert_eq!(two_starts(&mut starts).await, ann_and_bob());

        for stop in stops {
            stop.cancel();
        }
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_actor_that_fails_to_initialize_fails_the_episode_before_anyone_starts() {
        let Stage {
            episode,
            mut starts,
            ..
        } = episode_with(&["ann", "bob"], &[], &[], |id| Initialization {
            gate: None,
            broken: id == "bob",
        });

        let error = episode.run(Duration::from_secs(60)).await.unwrap_err();

        let text = format!("{error:#}");
        assert!(text.contains("actor bob failed"), "{text}");
        assert!(text.contains("cannot initialize"), "{text}");
        assert!(starts.try_recv().is_err(), "nobody should have started");
    }

    #[tokio::test(start_paused = true)]
    async fn patience_runs_out_while_an_actor_is_still_initializing() {
        let (
            Stage {
                episode,
                mut starts,
                ..
            },
            _gate,
        ) = episode_with_bob_held_up();

        let error = episode.run(Duration::from_secs(5)).await.unwrap_err();

        assert!(error.to_string().contains("patience"), "{error}");
        assert!(starts.try_recv().is_err(), "nobody should have started");
    }

    #[tokio::test]
    async fn only_an_actor_with_the_logger_logs() {
        let Stage {
            episode,
            mut starts,
            mut log,
        } = episode_with(&["ann", "bob"], &[], &["ann"], |_| {
            Initialization::default()
        });
        let stops = stops(&episode);
        let running = tokio::spawn(episode.run(Duration::from_secs(60)));
        starts.recv().await.unwrap();
        starts.recv().await.unwrap();
        for stop in stops {
            stop.cancel();
        }
        running.await.unwrap().unwrap();

        let event = log.recv().await.unwrap();
        assert_eq!(event.payload, "ann");
        assert!(log.try_recv().is_err(), "bob has no logger");
    }
}
