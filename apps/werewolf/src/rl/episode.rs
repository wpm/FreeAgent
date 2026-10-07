use crate::rl::actor::{
    Actor, ActorId, ActorInit, Behavior, Context, Lifecycle, Observation, Outbox, Shutdown,
};
use anyhow::{Context as _, bail};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

struct Episode<L: Lifecycle, B: Behavior> {
    actors: HashMap<ActorId, Actor<L, B>>,
    /// One-time signals from each actor that it is running its message loop.
    readies: HashMap<ActorId, oneshot::Receiver<()>>,
    /// One-time signals telling each actor that every actor is running.
    starts: HashMap<ActorId, oneshot::Sender<()>>,
}

impl<L: Lifecycle, B: Behavior> Episode<L, B> {
    fn new(init: HashMap<ActorId, ActorInit<L, B>>) -> Self {
        // First pass: give every actor a channel and a shutdown token. The
        // init and the receiver are unique, so they stay together in one
        // map. The senders and tokens are clonable, so they go into lookup
        // tables that each actor copies the permitted entries from.
        let mut senders = HashMap::new();
        let mut shutdowns = HashMap::new();
        let staged: HashMap<
            ActorId,
            (ActorInit<L, B>, UnboundedReceiver<Observation<B::Message>>),
        > = init
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
            .map(|(id, (init, inbox))| {
                let (ready, is_ready) = oneshot::channel();
                readies.insert(id.clone(), is_ready);
                let (start, started) = oneshot::channel();
                starts.insert(id.clone(), start);
                let context = Context {
                    id: id.clone(),
                    outbox: Outbox {
                        loopback: senders[&id].clone(),
                        others: pick(&senders, &init.can_send_to),
                    },
                    shutdown: Shutdown {
                        mine: shutdowns[&id].clone(),
                        others: pick(&shutdowns, &init.can_shut_down),
                    },
                };
                (
                    id,
                    Actor {
                        lifecycle: init.lifecycle,
                        behavior: (init.behavior)(context),
                        ready,
                        start: started,
                        inbox,
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
    /// them to finish. If they have not finished within `patience`, shut
    /// them all down and fail.
    ///
    /// No actor starts until every actor is ready, so an opening move never
    /// lands on an actor that is still initializing.
    ///
    /// # Errors
    ///
    /// The first actor to fail shuts the others down, and its error is the
    /// episode's. An actor that fails to initialize fails the episode before
    /// any actor starts. Running out of patience is an error too.
    async fn run(self, patience: Duration) -> anyhow::Result<()>
    where
        L: 'static,
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

/// Wait for every actor to report that it is ready. False if one of them
/// dropped its signal instead, which means it failed to initialize.
async fn all_ready(readies: HashMap<ActorId, oneshot::Receiver<()>>) -> bool {
    for ready in readies.into_values() {
        if ready.await.is_err() {
            return false;
        }
    }
    true
}

/// Wait for every task to finish. The first failure shuts the remaining
/// actors down, since an episode with a broken actor in it cannot be trusted
/// to finish on its own, and is the error returned.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rl::message::Message;
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc::UnboundedSender;

    #[derive(Debug, Clone)]
    struct Note;
    impl Message for Note {}

    /// A lifecycle whose initialization can be held up or broken. With no
    /// gate and not broken, it has nothing to set up or tear down.
    #[derive(Default)]
    struct Gated {
        /// Initialization waits for a permit from here, if present.
        gate: Option<Arc<Semaphore>>,
        /// Initialization fails.
        broken: bool,
    }
    #[async_trait]
    impl Lifecycle for Gated {
        async fn initialize(&self) -> anyhow::Result<()> {
            if self.broken {
                bail!("cannot initialize");
            }
            if let Some(gate) = &self.gate {
                gate.acquire().await?.forget();
            }
            Ok(())
        }
    }

    /// A behavior that reports when it is started, fails to start if told
    /// to, and otherwise never says anything.
    struct Reporter {
        context: Context<Note>,
        started: UnboundedSender<ActorId>,
        fails: bool,
    }
    #[async_trait]
    impl Behavior for Reporter {
        type Message = Note;
        fn context(&self) -> &Context<Note> {
            &self.context
        }
        async fn policy(&self, _observation: &Note) -> anyhow::Result<Vec<Note>> {
            Ok(vec![])
        }
        async fn start(&self) -> anyhow::Result<()> {
            if self.fails {
                bail!("{} refuses to start", self.context.id);
            }
            self.started.send(self.context.id.clone())?;
            Ok(())
        }
    }

    /// An episode of [`Reporter`]s named `ids`, those in `failing` set to
    /// refuse to start, along with the channel on which every reporter
    /// announces that it was started. None of the actors may send to or
    /// shut down any other, and each has `lifecycle(id)`.
    fn episode_with(
        ids: &[&str],
        failing: &[&str],
        lifecycle: impl Fn(&str) -> Gated,
    ) -> (Episode<Gated, Reporter>, UnboundedReceiver<ActorId>) {
        let (started, starts) = unbounded_channel();
        let init = ids
            .iter()
            .map(|id| {
                let started = started.clone();
                let fails = failing.contains(id);
                let init = ActorInit {
                    lifecycle: lifecycle(id),
                    behavior: Box::new(move |context| Reporter {
                        context,
                        started,
                        fails,
                    }),
                    can_send_to: HashSet::new(),
                    can_shut_down: HashSet::new(),
                };
                (id.to_string(), init)
            })
            .collect();
        (Episode::new(init), starts)
    }

    /// [`episode_with`] where every actor initializes at once.
    fn episode_of(
        ids: &[&str],
        failing: &[&str],
    ) -> (Episode<Gated, Reporter>, UnboundedReceiver<ActorId>) {
        episode_with(ids, failing, |_| Gated::default())
    }

    fn stops<L: Lifecycle, B: Behavior>(episode: &Episode<L, B>) -> Vec<CancellationToken> {
        episode
            .actors
            .values()
            .map(|actor| actor.behavior.context().shutdown.mine.clone())
            .collect()
    }

    #[tokio::test]
    async fn run_starts_every_actor_then_waits_for_them_to_finish() {
        let (episode, mut starts) = episode_of(&["ann", "bob"], &[]);
        let stops = stops(&episode);
        let running = tokio::spawn(episode.run(Duration::from_secs(60)));

        let mut started = HashSet::new();
        started.insert(starts.recv().await.unwrap());
        started.insert(starts.recv().await.unwrap());
        assert_eq!(
            started,
            HashSet::from(["ann".to_string(), "bob".to_string()])
        );

        for stop in stops {
            stop.cancel();
        }
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn run_gives_up_after_patience() {
        let (episode, _starts) = episode_of(&["ann", "bob"], &[]);
        let error = episode.run(Duration::from_secs(5)).await.unwrap_err();
        assert!(error.to_string().contains("patience"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_actor_ends_the_episode_with_its_error() {
        let (episode, _starts) = episode_of(&["ann", "bob"], &["bob"]);
        let error = episode.run(Duration::from_secs(60)).await.unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("actor bob failed"), "{text}");
        assert!(text.contains("bob refuses to start"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn no_actor_starts_until_every_actor_has_initialized() {
        let gate = Arc::new(Semaphore::new(0));
        let slow = Arc::clone(&gate);
        let (episode, mut starts) = episode_with(&["ann", "bob"], &[], move |id| Gated {
            gate: (id == "bob").then(|| Arc::clone(&slow)),
            broken: false,
        });
        let stops = stops(&episode);
        let running = tokio::spawn(episode.run(Duration::from_secs(60)));

        // Ann is ready at once, but with Bob still initializing nothing
        // happens, not even after the runtime has gone idle.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(starts.try_recv().is_err(), "nobody should have started");

        gate.add_permits(1);
        let mut started = HashSet::new();
        started.insert(starts.recv().await.unwrap());
        started.insert(starts.recv().await.unwrap());
        assert_eq!(
            started,
            HashSet::from(["ann".to_string(), "bob".to_string()])
        );

        for stop in stops {
            stop.cancel();
        }
        running.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn an_actor_that_fails_to_initialize_fails_the_episode_before_anyone_starts() {
        let (episode, mut starts) = episode_with(&["ann", "bob"], &[], |id| Gated {
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
        let gate = Arc::new(Semaphore::new(0));
        let (episode, mut starts) = episode_with(&["ann", "bob"], &[], move |id| Gated {
            gate: (id == "bob").then(|| Arc::clone(&gate)),
            broken: false,
        });

        let error = episode.run(Duration::from_secs(5)).await.unwrap_err();

        assert!(error.to_string().contains("patience"), "{error}");
        assert!(starts.try_recv().is_err(), "nobody should have started");
    }
}
