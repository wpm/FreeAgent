//! A game played live for a viewer. [`play`] starts a [`Prepared`] game in
//! the background and folds each event through the game's [`Stage`] into
//! cues as it happens, ending with [`Beat::Over`]. A [`Session`] plays one
//! game at a time, from preparing it to its end.

use crate::game::{Actor, Entry};
use crate::run::{Prepared, Ready};
use crate::stage::{Stage, Staged, millis};
use anyhow::anyhow;
use free_agent::{Event, Stopper};
use social_deduction_stage::{Beat, Cue, Outcome};
use std::future::Future;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How long a game may take before it is abandoned.
pub const PATIENCE: Duration = Duration::from_secs(60 * 60);

/// A game playing in the background.
pub struct Playing {
    /// Stops the game's episode.
    stopper: Stopper,
    /// The task that reads the game's log, which finishes after the
    /// episode does.
    task: JoinHandle<()>,
}

impl Playing {
    /// Stop the game. It ends with [`Outcome::Stopped`] unless it has
    /// already ended.
    pub fn stop(&self) {
        self.stopper.stop();
    }

    /// Whether the game has ended and its last cue is sent.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Wait until the game has ended and its last cue is sent.
    pub async fn finished(self) {
        let _ = self.task.await;
    }
}

/// Why a session refuses a game: it has one already.
pub const BUSY: &str = "A game is already playing.";

/// Why a game stopped before it started never does.
pub const STOPPED: &str = "The game was stopped before it started.";

/// The one game an app plays at a time, from the moment it is asked for:
/// prepared, then played, and stopped at either stage.
#[derive(Default)]
pub struct Session(Mutex<Now>);

/// Where a [`Session`]'s game is.
#[derive(Default)]
enum Now {
    /// There is none, or it was stopped before it started.
    #[default]
    Idle,
    /// It is being prepared, until the token is cancelled.
    Preparing(CancellationToken),
    /// It is playing, or has ended.
    Playing(Playing),
}

impl Now {
    /// Whether a game is being prepared or playing.
    fn busy(&self) -> bool {
        match self {
            Now::Idle => false,
            Now::Preparing(_) => true,
            Now::Playing(playing) => !playing.is_finished(),
        }
    }
}

impl Session {
    /// Where the session's game is.
    fn now(&self) -> MutexGuard<'_, Now> {
        self.0.lock().unwrap()
    }

    /// Prepare a game with `prepare` and [`play`] it, its cues passed to
    /// `show`. It returns once the game has started.
    ///
    /// # Errors
    ///
    /// [`BUSY`] while another game is being prepared or playing, and
    /// [`STOPPED`] when [`Session::stop`] comes first. When the game
    /// cannot be prepared, why.
    pub async fn play(
        &self,
        prepare: impl Future<Output = anyhow::Result<Prepared>>,
        show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let stopped = self.claim()?;
        let prepared = tokio::select! {
            biased;
            () = stopped.cancelled() => Err(anyhow!(STOPPED)),
            prepared = prepare => prepared,
        };
        let mut now = self.now();
        *now = Now::Idle;
        let prepared = prepared?;
        anyhow::ensure!(!stopped.is_cancelled(), STOPPED);
        *now = Now::Playing(play(prepared, show));
        Ok(())
    }

    /// Mark the session as preparing a game, and give back what stops it.
    fn claim(&self) -> anyhow::Result<CancellationToken> {
        let mut now = self.now();
        anyhow::ensure!(!now.busy(), BUSY);
        let stopped = CancellationToken::new();
        *now = Now::Preparing(stopped.clone());
        Ok(stopped)
    }

    /// Whether a game is being prepared or playing.
    pub fn busy(&self) -> bool {
        self.now().busy()
    }

    /// Stop the game being prepared, so that it never starts, or the game
    /// playing.
    pub fn stop(&self) {
        match &*self.now() {
            Now::Idle => {}
            Now::Preparing(stopped) => stopped.cancel(),
            Now::Playing(playing) => playing.stop(),
        }
    }
}

/// Start the game that is `prepared`. Each event of its log is folded
/// into cues, each passed to `show` as it happens. When `show` fails, the
/// game stops.
///
/// The last cue is [`Beat::Over`], after a thought for every call still
/// out: the side that won, [`Outcome::Stopped`] after a stop, or
/// [`Outcome::Failed`] when the episode fails, after one [`Beat::Told`]
/// line with the error.
pub fn play(
    prepared: Prepared,
    show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
) -> Playing {
    match prepared {
        Prepared::Werewolf(ready) => start(*ready, show),
        Prepared::Avalon(ready) => start(*ready, show),
    }
}

/// Start the game of `G` that is `ready`, as [`play`] does.
fn start<G: Staged>(
    ready: Ready<G>,
    show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
) -> Playing
where
    Actor<G>: 'static,
{
    let (logger, events) = unbounded_channel();
    let episode = ready.episode(logger);
    let stopper = episode.stopper();
    let run = tokio::spawn(episode.run(PATIENCE));
    let reader = Reader {
        show,
        stopper: stopper.clone(),
    };
    let task = tokio::spawn(reader.read::<G>(events, async move { run.await? }));
    Playing { stopper, task }
}

/// What reads a game's log: where its cues go, and what stops the game
/// when they can no longer go there.
struct Reader<S> {
    show: S,
    stopper: Stopper,
}

impl<S> Reader<S>
where
    S: FnMut(Cue) -> anyhow::Result<()>,
{
    /// Show every event from `events` until the episode `run` has ended,
    /// then show how the game ended.
    async fn read<G: Staged>(
        mut self,
        mut events: UnboundedReceiver<Event<Entry<G>>>,
        run: impl Future<Output = anyhow::Result<()>>,
    ) {
        let mut stage = Stage::<G>::default();
        let mut won = None;
        while let Some(event) = events.recv().await {
            for cue in stage.fold(&event) {
                match cue.beat {
                    Beat::Over { outcome } => won = Some(outcome),
                    _ => self.show(cue),
                }
            }
        }
        let ended = run.await;
        let at = millis(SystemTime::now());
        // No player is left thinking once the game is over.
        for cue in stage.close(at) {
            self.show(cue);
        }
        let outcome = match ended {
            Err(error) => {
                let error = format!("{error:#}");
                self.show(Cue {
                    at,
                    beat: Beat::Told(vec![error.clone()]),
                });
                Outcome::Failed(error)
            }
            Ok(()) => won.unwrap_or(Outcome::Stopped),
        };
        self.show(Cue {
            at,
            beat: Beat::Over { outcome },
        });
    }

    /// Show `cue`, or stop the game when it cannot be shown.
    fn show(&mut self, cue: Cue) {
        if (self.show)(cue).is_err() {
            self.stopper.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::Avalon;
    use crate::game::tests::logged;
    use crate::model::spec::Environment;
    use crate::run::prepare;
    use crate::stage::cues;
    use free_agent::Episode;
    use std::collections::HashMap;
    use std::path::Path;
    use tokio::sync::mpsc::UnboundedSender;

    /// The text of the example file `name` in `configs/`.
    fn example(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("configs")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    /// Where `show` sends each cue: down `cues`.
    fn sending(cues: UnboundedSender<Cue>) -> impl FnMut(Cue) -> anyhow::Result<()> {
        move |cue| Ok(cues.send(cue)?)
    }

    /// Every cue `cues` holds, once its sender is gone.
    async fn received(mut cues: UnboundedReceiver<Cue>) -> Vec<Cue> {
        let mut received = Vec::new();
        while let Some(cue) = cues.recv().await {
            received.push(cue);
        }
        received
    }

    /// The random game `name`, prepared.
    async fn prepared(name: &str) -> Prepared {
        prepare(&example(name), &Environment).await.unwrap()
    }

    /// Play the game of `G` that is `ready` as [`start`] does, its events
    /// also written as JSON Lines: the cues shown, and the log written.
    async fn teed<G: Staged>(ready: Ready<G>) -> (Vec<Cue>, String)
    where
        Actor<G>: 'static,
    {
        let (logger, mut log) = unbounded_channel();
        let episode = ready.episode(logger);
        let stopper = episode.stopper();
        let run = tokio::spawn(episode.run(PATIENCE));
        let (forward, events) = unbounded_channel();
        let copying = async move {
            let mut written = Vec::new();
            while let Some(event) = log.recv().await {
                event.write(&mut written).unwrap();
                forward.send(event).unwrap();
            }
            String::from_utf8(written).unwrap()
        };
        let (sender, receiver) = unbounded_channel();
        let reader = Reader {
            show: sending(sender),
            stopper,
        };
        let ((), written) =
            tokio::join!(reader.read::<G>(events, async move { run.await? }), copying);
        (received(receiver).await, written)
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_is_shown_as_its_log_folds_and_ends_with_its_outcome() {
        for name in ["werewolf/random.toml", "avalon/random.toml"] {
            let (live, written) = match prepared(name).await {
                Prepared::Werewolf(ready) => teed(*ready).await,
                Prepared::Avalon(ready) => teed(*ready).await,
            };
            let expected = cues(&written).unwrap();
            let beats =
                |cues: &[Cue]| -> Vec<Beat> { cues.iter().map(|c| c.beat.clone()).collect() };
            assert_eq!(beats(&live), beats(&expected), "{name}");
            assert!(
                matches!(
                    live.last().map(|cue| &cue.beat),
                    Some(Beat::Over {
                        outcome: Outcome::Won { .. }
                    })
                ),
                "{name}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_playing_is_not_finished_until_its_last_cue() {
        let (sender, receiver) = unbounded_channel();
        let playing = play(prepared("avalon/random.toml").await, sending(sender));
        assert!(!playing.is_finished());
        playing.finished().await;
        let cues = received(receiver).await;
        assert!(matches!(
            cues.last().map(|cue| &cue.beat),
            Some(Beat::Over { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stopped_game_ends_stopped() {
        let (sender, receiver) = unbounded_channel();
        let playing = play(prepared("avalon/random.toml").await, sending(sender));
        playing.stop();
        playing.finished().await;
        let cues = received(receiver).await;
        assert_eq!(
            cues.last().map(|cue| &cue.beat),
            Some(&Beat::Over {
                outcome: Outcome::Stopped,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_channel_stops_the_game() {
        let (sender, receiver) = unbounded_channel();
        let mut shown = 0;
        play(prepared("avalon/random.toml").await, move |cue| {
            shown += 1;
            anyhow::ensure!(shown == 1, "the window closed");
            Ok(sender.send(cue)?)
        })
        .finished()
        .await;
        let cues = received(receiver).await;
        assert_eq!(cues.len(), 1, "{cues:?}");
        assert!(!matches!(cues[0].beat, Beat::Over { .. }), "{cues:?}");
    }

    #[tokio::test]
    async fn a_failed_episode_closes_the_calls_still_out_tells_its_error_and_ends_failed() {
        let (logger, events) = unbounded_channel::<Event<Entry<Avalon>>>();
        let thinking = Entry::Thinking {
            player: "Player 2".to_string(),
            seq: 1,
        };
        logger.send(logged(thinking)).unwrap();
        drop(logger);
        let (sender, receiver) = unbounded_channel();
        let (unused, _) = unbounded_channel();
        let episode = Episode::<Actor<Avalon>>::new(HashMap::new(), unused);
        let reader = Reader {
            show: sending(sender),
            stopper: episode.stopper(),
        };
        let run = async { Err(anyhow::anyhow!("no model").context("actor Player 1 failed")) };
        reader.read::<Avalon>(events, run).await;
        let beats: Vec<Beat> = received(receiver)
            .await
            .into_iter()
            .map(|cue| cue.beat)
            .collect();
        let error = "actor Player 1 failed: no model".to_string();
        let who = "Player 2".to_string();
        assert_eq!(
            beats,
            [
                Beat::Thinking { who: who.clone() },
                Beat::Thought { who },
                Beat::Told(vec![error.clone()]),
                Beat::Over {
                    outcome: Outcome::Failed(error),
                }
            ]
        );
    }

    /// The random game `name`, to prepare.
    async fn preparing(name: &str) -> anyhow::Result<Prepared> {
        Ok(prepared(name).await)
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_plays_one_game_at_a_time() {
        let session = Session::default();
        let (sender, receiver) = unbounded_channel();
        session
            .play(preparing("avalon/random.toml"), sending(sender))
            .await
            .unwrap();
        let (refused, _) = unbounded_channel();
        let error = session
            .play(preparing("avalon/random.toml"), sending(refused))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), BUSY);
        session.stop();
        let cues = received(receiver).await;
        assert!(matches!(
            cues.last().map(|cue| &cue.beat),
            Some(Beat::Over {
                outcome: Outcome::Stopped,
            })
        ));
        assert!(!session.busy());
        let (sender, receiver) = unbounded_channel();
        session
            .play(preparing("werewolf/random.toml"), sending(sender))
            .await
            .unwrap();
        received(receiver).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_stopped_while_it_is_prepared_never_starts() {
        let session = Session::default();
        let (sender, receiver) = unbounded_channel();
        let (refused, _) = unbounded_channel();
        let hanging = std::future::pending();
        let (played, ()) = tokio::join!(session.play(hanging, sending(sender)), async {
            tokio::task::yield_now().await;
            assert!(session.busy());
            let error = session
                .play(std::future::pending(), sending(refused))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), BUSY);
            session.stop();
        });
        assert_eq!(played.unwrap_err().to_string(), STOPPED);
        assert!(received(receiver).await.is_empty());
        assert!(!session.busy());
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_stopped_just_as_it_is_prepared_never_starts() {
        let session = Session::default();
        let ready = prepared("avalon/random.toml").await;
        let (sender, receiver) = unbounded_channel();
        let stopping = async {
            session.stop();
            Ok(ready)
        };
        let error = session.play(stopping, sending(sender)).await.unwrap_err();
        assert_eq!(error.to_string(), STOPPED);
        assert!(received(receiver).await.is_empty());
    }

    #[tokio::test]
    async fn a_game_that_cannot_be_prepared_is_an_error_and_frees_the_session() {
        let session = Session::default();
        session.stop();
        let (sender, _) = unbounded_channel();
        let failing = async { Err(anyhow::anyhow!("no model")) };
        let error = session.play(failing, sending(sender)).await.unwrap_err();
        assert_eq!(error.to_string(), "no model");
        assert!(!session.busy());
    }
}
