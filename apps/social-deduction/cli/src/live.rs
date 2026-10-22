//! A game played live for a viewer. [`play`] starts a [`Prepared`] game in
//! the background and folds each event through the game's [`Stage`] into
//! cues as it happens, ending with [`Beat::Over`], and keeps its log, as
//! `social-deduction play` writes it, as a [`GameLog`]. A [`Session`]
//! plays one game at a time, from preparing it to its end, and keeps the
//! last one's log until the next one starts.

use crate::config::Name;
use crate::game::{Actor, Entry};
use crate::log::Writer;
use crate::run::{Prepared, Ready};
use crate::stage::{Stage, Staged, millis};
use anyhow::anyhow;
use free_agent::{Event, Stopper};
use free_agent_game::log::{Ended, Secrets};
use social_deduction_gui_types::{Beat, Cue};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How long a game may take before it is abandoned.
pub const PATIENCE: Duration = Duration::from_secs(60 * 60);

/// The log of a game that is over, written as `social-deduction play`
/// writes it, closing line included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLog {
    /// The game played.
    pub game: Name,
    /// When it started.
    pub started: SystemTime,
    /// The log, in JSON Lines.
    pub bytes: Arc<[u8]>,
}

/// A game playing in the background.
pub struct Playing {
    /// Stops the game's episode.
    stopper: Stopper,
    /// The task that reads the game's log, which finishes after the
    /// episode does.
    task: JoinHandle<()>,
    /// The game played.
    game: Name,
    /// When it started.
    started: SystemTime,
    /// The game's log, once it is over.
    kept: Arc<OnceLock<Arc<[u8]>>>,
}

impl Playing {
    /// Stop the game. It ends with
    /// [`Outcome::Stopped`](social_deduction_gui_types::Outcome::Stopped) unless it has
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

    /// The game's log, once it is over. It is kept before the last cue
    /// is sent.
    pub fn log(&self) -> Option<GameLog> {
        Some(GameLog {
            game: self.game,
            started: self.started,
            bytes: Arc::clone(self.kept.get()?),
        })
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

    /// The log of the last game played, once it is over, until the next
    /// one is asked for.
    pub fn log(&self) -> Option<GameLog> {
        match &*self.now() {
            Now::Playing(playing) => playing.log(),
            Now::Idle | Now::Preparing(_) => None,
        }
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
/// out: the side that won,
/// [`Outcome::Stopped`](social_deduction_gui_types::Outcome::Stopped) after a
/// stop, or [`Outcome::Failed`](social_deduction_gui_types::Outcome::Failed)
/// when the episode fails, after one [`Beat::Told`]
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
    let (episode, _) = ready.episode(logger);
    let started = SystemTime::now();
    let stopper = episode.stopper();
    let kept = Arc::<OnceLock<_>>::default();
    let reader = Reader::new(
        show,
        stopper.clone(),
        episode.id(),
        ready.secrets().clone(),
        Arc::clone(&kept),
    );
    let run = tokio::spawn(episode.run(PATIENCE));
    let task = tokio::spawn(reader.read(events, async move { run.await? }));
    Playing {
        stopper,
        task,
        game: G::KEY,
        started,
        kept,
    }
}

/// What reads a game's log: where its cues go, what stops the game when
/// they can no longer go there, and what writes the log down, until a
/// line of it cannot be written, and where it is kept once it is over.
struct Reader<G, S> {
    show: S,
    stopper: Stopper,
    writer: Option<Writer<G, Vec<u8>>>,
    kept: Arc<OnceLock<Arc<[u8]>>>,
}

impl<G, S> Reader<G, S>
where
    G: Staged,
    S: FnMut(Cue) -> anyhow::Result<()>,
{
    /// A reader of the episode `episode` that shows its cues with `show`,
    /// stops the game with `stopper`, and keeps its log, without
    /// `secrets`, in `kept`.
    fn new(
        show: S,
        stopper: Stopper,
        episode: Uuid,
        secrets: Secrets,
        kept: Arc<OnceLock<Arc<[u8]>>>,
    ) -> Self {
        Reader {
            show,
            stopper,
            writer: Writer::start(Vec::new(), episode, secrets).ok(),
            kept,
        }
    }

    /// Write down and show every event from `events` until the episode
    /// `run` has ended, then keep the log, closed with how the game
    /// ended, and show that, both stamped with one time. Each event is
    /// shown as its line of the log reads back, without the keys, and
    /// nothing is shown once a line cannot be written but that the game
    /// is over.
    async fn read(
        mut self,
        mut events: UnboundedReceiver<Event<Entry<G>>>,
        run: impl Future<Output = anyhow::Result<()>>,
    ) {
        let mut stage = Stage::<G>::default();
        let mut won = None;
        while let Some(event) = events.recv().await {
            let line = (self.writer.as_mut()).and_then(|writer| writer.write(&event).ok());
            self.writer = self.writer.take().filter(|_| line.is_some());
            let read = line.and_then(|line| serde_json::from_str(&line).ok());
            for cue in read.iter().flat_map(|event| stage.fold(event)) {
                match cue.beat {
                    Beat::Over { .. } => won = Some(cue),
                    _ => self.show(cue),
                }
            }
        }
        let ended = Ended::of(&run.await, won.is_some());
        let at = SystemTime::now();
        let closed = (self.writer.take()).and_then(|writer| writer.end_at(ended, at).ok());
        let mut ended = None;
        if let Some((log, line)) = closed {
            let _ = self.kept.set(log.into());
            if let Ok(Event::<Entry<G>> {
                payload: Entry::Ended(how),
                ..
            }) = serde_json::from_str(&line)
            {
                ended = Some(how);
            }
        }
        for cue in stage.end(millis(at), won, ended) {
            self.show(cue);
        }
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
    use crate::log::Writer;
    use crate::model::canned::{
        CANARY, CANARY_VARIABLE, assert_redacted, leaking, leaking_werewolf,
    };
    use crate::run::prepare;
    use crate::stage::recordings;
    use free_agent::Episode;
    use free_agent_model::secret::Keychain;
    use free_agent_model::secret::testing::Fake;
    use free_agent_model::spec::Environment;
    use secrecy::SecretString;
    use social_deduction_gui_types::Outcome;
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

    /// `log` line by line, its first and last lines, the header and how
    /// the game ended, without the times they were written.
    fn unstamped(log: &str) -> Vec<String> {
        let mut lines: Vec<String> = log.lines().map(str::to_string).collect();
        let last = lines.len() - 1;
        for line in [0, last] {
            let mut event: serde_json::Value = serde_json::from_str(&lines[line]).unwrap();
            event.as_object_mut().unwrap().remove("time");
            lines[line] = event.to_string();
        }
        lines
    }

    /// Read the episode `episode`'s events from `log` as [`start`] does
    /// until `run` comes back, each event also written by the log's
    /// writer as the command line writes it, ending `ended`. The cues
    /// shown, and the log the reader kept, which is the command line's
    /// but for the times of its first and last lines.
    async fn teed<G: Staged>(
        episode: Uuid,
        stopper: Stopper,
        mut log: UnboundedReceiver<Event<Entry<G>>>,
        run: impl Future<Output = anyhow::Result<()>>,
        ended: Ended,
    ) -> (Vec<Cue>, String) {
        let mut writer = Writer::<G, _>::start(Vec::new(), episode, Secrets::default()).unwrap();
        let (forward, events) = unbounded_channel();
        let copying = async move {
            while let Some(event) = log.recv().await {
                writer.write(&event).unwrap();
                forward.send(event).unwrap();
            }
            String::from_utf8(writer.end(ended).unwrap()).unwrap()
        };
        let (sender, receiver) = unbounded_channel();
        let kept = Arc::<OnceLock<_>>::default();
        let secrets = Secrets::default();
        let reader = Reader::new(
            sending(sender),
            stopper,
            episode,
            secrets,
            Arc::clone(&kept),
        );
        let ((), written) = tokio::join!(reader.read(events, run), copying);
        let saved = String::from_utf8(kept.get().unwrap().to_vec()).unwrap();
        assert_eq!(unstamped(&saved), unstamped(&written));
        (received(receiver).await, saved)
    }

    /// Play the game of `G` that is `ready` as [`start`] does, stopped
    /// at once when `stop`: the cues shown, and the log kept.
    async fn played<G: Staged>(ready: Ready<G>, stop: bool) -> (Vec<Cue>, String)
    where
        Actor<G>: 'static,
    {
        let (logger, log) = unbounded_channel();
        let (episode, _) = ready.episode(logger);
        let (id, stopper) = (episode.id(), episode.stopper());
        let ended = if stop {
            stopper.stop();
            Ended::Stopped
        } else {
            Ended::Won
        };
        let run = tokio::spawn(episode.run(PATIENCE));
        teed(id, stopper, log, async move { run.await? }, ended).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_is_shown_as_its_saved_log_reads_back_and_ends_with_its_outcome() {
        for name in ["werewolf/random.toml", "avalon/random.toml"] {
            for stop in [false, true] {
                let (live, saved) = match prepared(name).await {
                    Prepared::Werewolf(ready) => played(*ready, stop).await,
                    Prepared::Avalon(ready) => played(*ready, stop).await,
                };
                let expected = recordings(&saved).unwrap().games.remove(0).cues;
                assert_eq!(live, expected, "{name}");
                let last = live.last().map(|cue| &cue.beat);
                if stop {
                    let stopped = Beat::Over {
                        outcome: Outcome::Stopped,
                    };
                    assert_eq!(last, Some(&stopped), "{name}");
                    assert!(saved.ends_with("{\"Ended\":\"stopped\"}}\n"), "{saved}");
                } else {
                    let won = matches!(
                        last,
                        Some(Beat::Over {
                            outcome: Outcome::Won { .. }
                        })
                    );
                    assert!(won, "{name}");
                    assert!(saved.ends_with("{\"Ended\":\"won\"}}\n"), "{saved}");
                }
            }
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
        let (unused, _) = unbounded_channel();
        let stopper = Episode::<Actor<Avalon>>::new(HashMap::new(), unused).stopper();
        let run = async { Err(anyhow::anyhow!("no model").context("actor Player 1 failed")) };
        let failed = Ended::Failed("actor Player 1 failed: no model".to_string());
        let (cues, saved) = teed(Uuid::nil(), stopper, events, run, failed).await;
        assert_eq!(cues, recordings(&saved).unwrap().games.remove(0).cues);
        assert!(
            saved.ends_with("{\"Ended\":{\"failed\":\"actor Player 1 failed: no model\"}}}\n"),
            "{saved}"
        );
        let beats: Vec<Beat> = cues.into_iter().map(|cue| cue.beat).collect();
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

    #[tokio::test]
    async fn a_failed_episode_whose_error_carries_a_key_shows_none_of_it() {
        let (logger, events) = unbounded_channel::<Event<Entry<Avalon>>>();
        let thinking = Entry::Thinking {
            player: format!("{CANARY} player"),
            seq: 1,
        };
        logger.send(logged(thinking)).unwrap();
        drop(logger);
        let (unused, _) = unbounded_channel();
        let stopper = Episode::<Actor<Avalon>>::new(HashMap::new(), unused).stopper();
        let secrets: Secrets = [SecretString::from(CANARY)].into_iter().collect();
        let (sender, receiver) = unbounded_channel();
        let kept = Arc::<OnceLock<_>>::default();
        let reader = Reader::new(
            sending(sender),
            stopper,
            Uuid::nil(),
            secrets,
            Arc::clone(&kept),
        );
        let run = async { Err(anyhow::anyhow!("{CANARY} cannot be used")) };
        reader.read(events, run).await;
        let cues = received(receiver).await;
        let shown = serde_json::to_string(&cues).unwrap();
        assert!(!shown.contains(CANARY), "{shown}");
        let saved = String::from_utf8(kept.get().unwrap().to_vec()).unwrap();
        assert_eq!(cues, recordings(&saved).unwrap().games.remove(0).cues);
        assert_eq!(
            cues.last().map(|cue| &cue.beat),
            Some(&Beat::Over {
                outcome: Outcome::Failed("[redacted] cannot be used".to_string()),
            })
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
        assert_eq!(session.log(), None);
        session.stop();
        let cues = received(receiver).await;
        assert!(matches!(
            cues.last().map(|cue| &cue.beat),
            Some(Beat::Over {
                outcome: Outcome::Stopped,
            })
        ));
        assert!(!session.busy());
        let log = session.log().unwrap();
        assert_eq!(log.game, Name::Avalon);
        let text = String::from_utf8(log.bytes.to_vec()).unwrap();
        assert_eq!(recordings(&text).unwrap().games.remove(0).cues, cues);
        assert!(text.ends_with("{\"Ended\":\"stopped\"}}\n"), "{text}");
        let (sender, receiver) = unbounded_channel();
        session
            .play(preparing("werewolf/random.toml"), sending(sender))
            .await
            .unwrap();
        assert_eq!(session.log(), None);
        received(receiver).await;
        assert_eq!(session.log().unwrap().game, Name::Werewolf);
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
    async fn a_game_whose_keys_come_from_the_keychain_shows_and_keeps_none_of_them() {
        let base_url = leaking(&["qwen2.5-7b-instruct"], CANARY);
        let keychain = Keychain(Fake::default());
        keychain
            .store(CANARY_VARIABLE, &SecretString::from(CANARY))
            .unwrap();
        let session = Session::default();
        let (sender, receiver) = unbounded_channel();
        let text = leaking_werewolf(&base_url);
        let preparing = prepare(&text, &keychain);
        session.play(preparing, sending(sender)).await.unwrap();
        let cues = received(receiver).await;
        let log = String::from_utf8(session.log().unwrap().bytes.to_vec()).unwrap();
        assert_redacted(&log, CANARY);
        let shown = serde_json::to_string(&cues).unwrap();
        assert!(!shown.contains(CANARY), "{shown}");
        assert_eq!(recordings(&log).unwrap().games.remove(0).cues, cues);
        assert!(log.ends_with("{\"Ended\":\"won\"}}\n"), "{log}");
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
        assert_eq!(session.log(), None);
    }
}
