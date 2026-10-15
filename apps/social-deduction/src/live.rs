//! A game played live for a viewer. [`play`] starts a [`Prepared`] game in
//! the background, saves its log as JSON Lines, and folds each event
//! through the game's [`Stage`] into cues as it happens, ending with
//! [`Beat::Over`]. A [`Session`] plays one game at a time, from
//! preparing it to its end.

use crate::config::Name;
use crate::game::{Actor, Entry};
use crate::run::{Prepared, Ready};
use crate::stage::{Stage, Staged, millis};
use anyhow::{Context as _, anyhow};
use free_agent::{Event, Stopper};
use social_deduction_stage::{Beat, Cue, Outcome};
use std::fs::{self, File};
use std::future::Future;
use std::io::{self, LineWriter, Write};
use std::path::{Path, PathBuf};
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
/// prepared, then played, and stopped at either stage. The log of the
/// last game started is kept.
#[derive(Default)]
pub struct Session(Mutex<Held>);

/// What a [`Session`] holds.
#[derive(Default)]
struct Held {
    /// The game.
    now: Now,
    /// The log of the last game started.
    log: Option<PathBuf>,
}

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

impl Held {
    /// Whether a game is being prepared or playing.
    fn busy(&self) -> bool {
        match &self.now {
            Now::Idle => false,
            Now::Preparing(_) => true,
            Now::Playing(playing) => !playing.is_finished(),
        }
    }
}

impl Session {
    /// What the session holds.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.0.lock().unwrap()
    }

    /// Prepare a game with `prepare`, create its log in `dir` as
    /// [`create_log`] does, and [`play`] it, its cues passed to `show`. It
    /// returns once the game has started.
    ///
    /// # Errors
    ///
    /// [`BUSY`] while another game is being prepared or playing, and
    /// [`STOPPED`] when [`Session::stop`] comes first. When the game
    /// cannot be prepared or its log created, why. A game that does not
    /// start leaves no log.
    pub async fn play(
        &self,
        prepare: impl Future<Output = anyhow::Result<Prepared>>,
        dir: &Path,
        show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let stopped = self.claim()?;
        let prepared = tokio::select! {
            biased;
            () = stopped.cancelled() => Err(anyhow!(STOPPED)),
            prepared = prepare => prepared,
        };
        let mut held = self.held();
        held.now = Now::Idle;
        let prepared = prepared?;
        anyhow::ensure!(!stopped.is_cancelled(), STOPPED);
        let (file, path) = create_log(dir, prepared.game(), SystemTime::now())
            .context("The log could not be created")?;
        let shown = Some(path.display().to_string());
        held.now = Now::Playing(play(prepared, file, shown, show));
        held.log = Some(path);
        Ok(())
    }

    /// Mark the session as preparing a game, and give back what stops it.
    fn claim(&self) -> anyhow::Result<CancellationToken> {
        let mut held = self.held();
        anyhow::ensure!(!held.busy(), BUSY);
        let stopped = CancellationToken::new();
        held.now = Now::Preparing(stopped.clone());
        Ok(stopped)
    }

    /// Whether a game is being prepared or playing.
    pub fn busy(&self) -> bool {
        self.held().busy()
    }

    /// Stop the game being prepared, so that it never starts, or the game
    /// playing.
    pub fn stop(&self) {
        match &self.held().now {
            Now::Idle => {}
            Now::Preparing(stopped) => stopped.cancel(),
            Now::Playing(playing) => playing.stop(),
        }
    }

    /// The log of the last game started, if one has.
    pub fn log(&self) -> Option<PathBuf> {
        self.held().log.clone()
    }
}

/// Start the game that is `prepared`. Each event of its log is written to
/// `log`, the file at `path`, and folded into cues, each passed to `show`
/// as it happens. When `show` fails, the game stops.
///
/// The last cue is [`Beat::Over`], with `path` as its log, after a
/// thought for every call still out: the side that won,
/// [`Outcome::Stopped`] after a stop, or [`Outcome::Failed`] when the
/// episode fails, after one [`Beat::Told`] line with the error. A write to
/// `log` that fails tells one line saying so, and the game goes on
/// unsaved.
pub fn play(
    prepared: Prepared,
    log: impl Write + Send + 'static,
    path: Option<String>,
    show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
) -> Playing {
    match prepared {
        Prepared::Werewolf(ready) => start(*ready, log, path, show),
        Prepared::Avalon(ready) => start(*ready, log, path, show),
    }
}

/// Start the game of `G` that is `ready`, as [`play`] does.
fn start<G: Staged>(
    ready: Ready<G>,
    log: impl Write + Send + 'static,
    path: Option<String>,
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
        log,
        path,
        show,
        stopper: stopper.clone(),
    };
    let task = tokio::spawn(reader.read::<G>(events, async move { run.await? }));
    Playing { stopper, task }
}

/// What reads a game's log: where it is saved, where its cues go, and
/// what stops the game when they can no longer go there.
struct Reader<W, S> {
    log: W,
    path: Option<String>,
    show: S,
    stopper: Stopper,
}

impl<W, S> Reader<W, S>
where
    W: Write,
    S: FnMut(Cue) -> anyhow::Result<()>,
{
    /// Save and show every event from `events` until the episode `run`
    /// has ended, then show how the game ended.
    async fn read<G: Staged>(
        mut self,
        mut events: UnboundedReceiver<Event<Entry<G>>>,
        run: impl Future<Output = anyhow::Result<()>>,
    ) {
        let mut stage = Stage::<G>::default();
        let mut saving = true;
        let mut won = None;
        while let Some(event) = events.recv().await {
            let mut cues = Vec::new();
            if saving && let Err(error) = event.write(&mut self.log) {
                saving = false;
                let line = format!("The log stopped being saved: {error}.");
                cues.push(Cue {
                    at: millis(event.time),
                    beat: Beat::Told(vec![line]),
                });
            }
            for cue in stage.fold(&event) {
                match cue.beat {
                    Beat::Over { outcome, .. } => won = Some(outcome),
                    _ => cues.push(cue),
                }
            }
            for cue in cues {
                self.show(cue);
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
        let log = self.path.take();
        self.show(Cue {
            at,
            beat: Beat::Over { outcome, log },
        });
    }

    /// Show `cue`, or stop the game when it cannot be shown.
    fn show(&mut self, cue: Cue) {
        if (self.show)(cue).is_err() {
            self.stopper.stop();
        }
    }
}

/// Create the log of a game of `game` started at `time`:
/// `<dir>/<YYYY-MM-DDTHH-MM-SS>-<game>.jsonl`, with the time in UTC.
/// A log of that name already there is kept, and the new one numbered
/// `-2`, `-3` and on. The file comes back, written a line at a time, with
/// its path.
///
/// # Errors
///
/// When the directory or the file cannot be created.
pub fn create_log(
    dir: &Path,
    game: Name,
    time: SystemTime,
) -> io::Result<(LineWriter<File>, PathBuf)> {
    fs::create_dir_all(dir)?;
    let stamp = humantime::format_rfc3339_seconds(time)
        .to_string()
        .trim_end_matches('Z')
        .replace(':', "-");
    let game = match game {
        Name::Werewolf => "werewolf",
        Name::Avalon => "avalon",
    };
    let mut number = 1;
    loop {
        let suffix = match number {
            1 => String::new(),
            n => format!("-{n}"),
        };
        let path = dir.join(format!("{stamp}-{game}{suffix}.jsonl"));
        match File::create_new(&path) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => number += 1,
            file => return Ok((LineWriter::new(file?), path)),
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
    use std::io::Cursor;
    use std::time::UNIX_EPOCH;
    use tokio::sync::mpsc::UnboundedSender;

    /// The text of the example file `name` in `configs/`.
    fn example(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("configs")
            .join(name);
        fs::read_to_string(path).unwrap()
    }

    /// A directory of its own for the test `name`, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("live-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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

    #[tokio::test(start_paused = true)]
    async fn a_game_is_saved_event_by_event_and_shown_as_its_log_folds() {
        for (name, game) in [
            ("werewolf/random.toml", Name::Werewolf),
            ("avalon/random.toml", Name::Avalon),
        ] {
            let dir = scratch(&format!("played-{name}").replace('/', "-"));
            let (file, path) = create_log(&dir.join("games"), game, SystemTime::now()).unwrap();
            let shown = path.display().to_string();
            let (sender, receiver) = unbounded_channel();
            let playing = play(
                prepared(name).await,
                file,
                Some(shown.clone()),
                sending(sender),
            );
            assert!(!playing.is_finished());
            playing.finished().await;
            let live = received(receiver).await;
            let mut expected = cues(&fs::read_to_string(&path).unwrap()).unwrap();
            let Some(Beat::Over { outcome, log }) = expected.last_mut().map(|cue| &mut cue.beat)
            else {
                panic!("{name} did not end");
            };
            assert!(matches!(outcome, Outcome::Won { .. }), "{name}");
            *log = Some(shown);
            let beats =
                |cues: &[Cue]| -> Vec<Beat> { cues.iter().map(|c| c.beat.clone()).collect() };
            assert_eq!(beats(&live), beats(&expected), "{name}");
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_stopped_game_ends_stopped_with_its_logs_path() {
        let (sender, receiver) = unbounded_channel();
        let playing = play(
            prepared("avalon/random.toml").await,
            io::sink(),
            Some("game.jsonl".to_string()),
            sending(sender),
        );
        playing.stop();
        playing.finished().await;
        let cues = received(receiver).await;
        assert_eq!(
            cues.last().map(|cue| &cue.beat),
            Some(&Beat::Over {
                outcome: Outcome::Stopped,
                log: Some("game.jsonl".to_string())
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_write_tells_one_line_and_the_game_goes_on() {
        let (sender, receiver) = unbounded_channel();
        play(
            prepared("avalon/random.toml").await,
            Cursor::new([0u8; 0]),
            None,
            sending(sender),
        )
        .finished()
        .await;
        let cues = received(receiver).await;
        let told: Vec<&Beat> = cues
            .iter()
            .map(|cue| &cue.beat)
            .filter(
                |beat| matches!(beat, Beat::Told(lines) if lines[0].starts_with("The log stopped")),
            )
            .collect();
        assert_eq!(
            told,
            [&Beat::Told(vec![
                "The log stopped being saved: failed to write whole buffer.".to_string()
            ])]
        );
        assert!(matches!(cues[0].beat, Beat::Told(_)));
        assert!(matches!(
            cues.last().map(|cue| &cue.beat),
            Some(Beat::Over {
                outcome: Outcome::Won { .. },
                log: None
            })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_channel_stops_the_game() {
        let dir = scratch("closed");
        let (file, path) = create_log(&dir.join("games"), Name::Avalon, SystemTime::now()).unwrap();
        let mut shown = 0;
        play(
            prepared("avalon/random.toml").await,
            file,
            None,
            move |_| {
                shown += 1;
                anyhow::ensure!(shown == 1, "the window closed");
                Ok(())
            },
        )
        .finished()
        .await;
        let log = fs::read_to_string(&path).unwrap();
        assert!(!log.is_empty());
        let ended = |cue: &Cue| matches!(cue.beat, Beat::Over { .. });
        assert!(!cues(&log).unwrap().iter().any(ended), "{log}");
        fs::remove_dir_all(dir).unwrap();
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
            log: io::sink(),
            path: Some("game.jsonl".to_string()),
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
                    log: Some("game.jsonl".to_string())
                }
            ]
        );
    }

    /// The random game `name`, to prepare.
    async fn preparing(name: &str) -> anyhow::Result<Prepared> {
        Ok(prepared(name).await)
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_plays_one_game_at_a_time_and_keeps_its_log() {
        let session = Session::default();
        let dir = scratch("session");
        let (sender, receiver) = unbounded_channel();
        let preparing_avalon = preparing("avalon/random.toml");
        session
            .play(preparing_avalon, &dir.join("games"), sending(sender))
            .await
            .unwrap();
        let path = session.log().unwrap();
        assert!(path.starts_with(dir.join("games")), "{}", path.display());
        let (refused, _) = unbounded_channel();
        let error = session
            .play(
                preparing("avalon/random.toml"),
                &dir.join("games"),
                sending(refused),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), BUSY);
        assert_eq!(fs::read_dir(dir.join("games")).unwrap().count(), 1);
        session.stop();
        let cues = received(receiver).await;
        assert!(matches!(
            cues.last().map(|cue| &cue.beat),
            Some(Beat::Over {
                outcome: Outcome::Stopped,
                ..
            })
        ));
        assert!(!session.busy());
        let (sender, receiver) = unbounded_channel();
        session
            .play(
                preparing("werewolf/random.toml"),
                &dir.join("games"),
                sending(sender),
            )
            .await
            .unwrap();
        assert_ne!(session.log().unwrap(), path);
        received(receiver).await;
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_stopped_while_it_is_prepared_never_starts() {
        let session = Session::default();
        let dir = scratch("stopped-preparing");
        let games = dir.join("games");
        let (sender, receiver) = unbounded_channel();
        let (refused, _) = unbounded_channel();
        let hanging = std::future::pending();
        let (played, ()) = tokio::join!(session.play(hanging, &games, sending(sender)), async {
            tokio::task::yield_now().await;
            assert!(session.busy());
            let error = session
                .play(std::future::pending(), &games, sending(refused))
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), BUSY);
            session.stop();
        });
        assert_eq!(played.unwrap_err().to_string(), STOPPED);
        assert!(received(receiver).await.is_empty());
        assert!(!dir.join("games").exists());
        assert!(session.log().is_none());
        assert!(!session.busy());
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_stopped_just_as_it_is_prepared_never_starts() {
        let session = Session::default();
        let dir = scratch("stopped-prepared");
        let ready = prepared("avalon/random.toml").await;
        let (sender, receiver) = unbounded_channel();
        let stopping = async {
            session.stop();
            Ok(ready)
        };
        let error = session
            .play(stopping, &dir.join("games"), sending(sender))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), STOPPED);
        assert!(received(receiver).await.is_empty());
        assert!(!dir.join("games").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_game_that_cannot_be_prepared_or_logged_is_an_error_and_frees_the_session() {
        let session = Session::default();
        session.stop();
        let dir = scratch("unprepared");
        let (sender, _) = unbounded_channel();
        let failing = async { Err(anyhow::anyhow!("no model")) };
        let error = session
            .play(failing, &dir.join("games"), sending(sender.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "no model");
        assert!(!session.busy());
        fs::write(dir.join("games"), "").unwrap();
        let error = session
            .play(
                preparing("avalon/random.toml"),
                &dir.join("games"),
                sending(sender),
            )
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").starts_with("The log could not be created: "),
            "{error:#}"
        );
        assert!(!session.busy());
        assert!(session.log().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_log_is_named_for_when_its_game_started_and_which_game_it_is() {
        let dir = scratch("named");
        let time = UNIX_EPOCH + Duration::from_secs(1_791_900_000);
        let (_, path) = create_log(&dir.join("games"), Name::Werewolf, time).unwrap();
        assert_eq!(path, dir.join("games/2026-10-13T14-00-00-werewolf.jsonl"));
        assert!(path.exists());
        let (_, path) = create_log(&dir.join("games"), Name::Avalon, time).unwrap();
        assert!(path.ends_with("2026-10-13T14-00-00-avalon.jsonl"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_second_log_started_the_same_second_is_numbered_and_the_first_is_kept() {
        let dir = scratch("same-second");
        let time = UNIX_EPOCH + Duration::from_secs(1_791_900_000);
        let (mut first, path) = create_log(&dir.join("games"), Name::Avalon, time).unwrap();
        writeln!(first, "first").unwrap();
        let (_, second) = create_log(&dir.join("games"), Name::Avalon, time).unwrap();
        let (_, third) = create_log(&dir.join("games"), Name::Avalon, time).unwrap();
        assert!(second.ends_with("2026-10-13T14-00-00-avalon-2.jsonl"));
        assert!(third.ends_with("2026-10-13T14-00-00-avalon-3.jsonl"));
        assert_eq!(fs::read_to_string(path).unwrap(), "first\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_log_that_cannot_be_created_is_an_error() {
        let dir = scratch("blocked");
        fs::write(dir.join("games"), "").unwrap();
        assert!(create_log(&dir.join("games"), Name::Avalon, SystemTime::now()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
