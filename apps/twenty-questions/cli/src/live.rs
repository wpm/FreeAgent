//! A game played live for the screen. [`play`] starts a [`Ready`] game in
//! the background and folds each event through a [`Stage`] into cues as it
//! happens, ending with [`Beat::Over`](twenty_questions_gui_types::Beat::Over), and keeps its log, as
//! `twenty-questions play` writes it, as a [`GameLog`]. A [`Session`] plays
//! one game at a time, from preparing it to its end, and keeps the last
//! one's log until the next one starts.

use crate::log::{Entry, Writer};
use crate::run::Ready;
use crate::stage::{Stage, millis};
use anyhow::anyhow;
use free_agent::{Event, Stopper};
use free_agent_game::log::{Ended, Secrets};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use twenty_questions_gui_types::Cue;
use uuid::Uuid;

/// The log of a game that is over, written as `twenty-questions play`
/// writes it, closing line included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLog {
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
    /// When it started.
    started: SystemTime,
    /// The game's log, once it is over.
    kept: Arc<OnceLock<Arc<[u8]>>>,
}

impl Playing {
    /// Stop the game. It ends with
    /// [`Outcome::Stopped`](twenty_questions_gui_types::Outcome::Stopped)
    /// unless it has already ended.
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
        prepare: impl Future<Output = anyhow::Result<Ready>>,
        show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let stopped = self.claim()?;
        let ready = tokio::select! {
            biased;
            () = stopped.cancelled() => Err(anyhow!(STOPPED)),
            ready = prepare => ready,
        };
        let mut now = self.now();
        *now = Now::Idle;
        let ready = ready?;
        anyhow::ensure!(!stopped.is_cancelled(), STOPPED);
        *now = Now::Playing(play(&ready, show));
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

/// Start a game of the one `ready`. Each event of its log is folded into
/// cues, each passed to `show` as it happens. When `show` fails, the game
/// stops.
///
/// The last cue is [`Beat::Over`](twenty_questions_gui_types::Beat::Over), after a thought for every call still
/// out: how the game was won,
/// [`Outcome::Stopped`](twenty_questions_gui_types::Outcome::Stopped) after
/// a stop, or
/// [`Outcome::Failed`](twenty_questions_gui_types::Outcome::Failed) when
/// the episode fails, after one [`Beat::Told`](twenty_questions_gui_types::Beat::Told) line with the error.
pub fn play(
    ready: &Ready,
    show: impl FnMut(Cue) -> anyhow::Result<()> + Send + 'static,
) -> Playing {
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
    let run = tokio::spawn(episode.run(ready.patience()));
    let task = tokio::spawn(reader.read(events, async move { run.await? }));
    Playing {
        stopper,
        task,
        started,
        kept,
    }
}

/// What reads a game's log: where its cues go, what stops the game when
/// they can no longer go there, and what writes the log down, until a line
/// of it cannot be written, and where it is kept once it is over.
struct Reader<S> {
    show: S,
    stopper: Stopper,
    writer: Option<Writer<Vec<u8>>>,
    kept: Arc<OnceLock<Arc<[u8]>>>,
}

impl<S> Reader<S>
where
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
    /// nothing is shown once a line cannot be written.
    async fn read(
        mut self,
        mut events: UnboundedReceiver<Event<Entry>>,
        run: impl Future<Output = anyhow::Result<()>>,
    ) {
        let mut stage = Stage::default();
        while let Some(event) = events.recv().await {
            let line = (self.writer.as_mut()).and_then(|writer| writer.write(&event).ok());
            self.writer = self.writer.take().filter(|_| line.is_some());
            let read = line.and_then(|line| serde_json::from_str(&line).ok());
            for cue in read.iter().flat_map(|event| stage.fold(event)) {
                self.show(cue);
            }
        }
        let ended = Ended::of(&run.await, stage.is_won());
        let at = SystemTime::now();
        let closed = self
            .writer
            .take()
            .map(|writer| writer.end_at(ended.clone(), at));
        if let Some(Ok(log)) = closed {
            let _ = self.kept.set(log.into());
        }
        for cue in stage.end(millis(at), Some(ended)) {
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
    use crate::log::tests::logged;
    use crate::model::canned::{CANARY, CANARY_VARIABLE, assert_redacted, leaking, playing};
    use crate::run::prepare;
    use crate::stage::tests::only;
    use free_agent::Episode;
    use free_agent_model::secret::Keychain;
    use free_agent_model::secret::testing::Fake;
    use free_agent_model::spec::Environment;
    use secrecy::SecretString;
    use std::collections::HashMap;
    use tokio::sync::mpsc::UnboundedSender;
    use twenty_questions_gui_types::{Beat, Outcome, Victory};

    /// A quick game of two players at the provider at `base_url`, the
    /// model's table ending with `model`.
    fn quick(base_url: &str, model: &str) -> String {
        format!(
            "time_limit = \"30s\"\n\
             [players]\ncount = 2\n\
             [host]\nobject = \"a teapot\"\n\
             system = \"You are the host. The object is {{{{ object }}}}.\"\n\
             [models.qwen]\nbase_url = \"{base_url}\"\nid = \"qwen2.5-7b-instruct\"\n{model}\n\
             [strategies.plain]\nsystem = \"You are {{{{ name }}}}.\"\n"
        )
    }

    /// A quick game against the canned provider, prepared.
    async fn prepared() -> Ready {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        prepare(&quick(&base_url, ""), &Environment).await.unwrap()
    }

    /// A quick game against the canned provider, to prepare.
    async fn preparing() -> anyhow::Result<Ready> {
        Ok(prepared().await)
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

    /// The last beat of `cues`.
    fn last(cues: &[Cue]) -> Option<&Beat> {
        cues.last().map(|cue| &cue.beat)
    }

    /// The text of the log `session` keeps, and the cues it reads back as.
    fn kept(session: &Session) -> (String, Vec<Cue>) {
        let text = String::from_utf8(session.log().unwrap().bytes.to_vec()).unwrap();
        let cues = only(&text);
        (text, cues)
    }

    #[tokio::test]
    async fn a_game_is_shown_as_its_saved_log_reads_back_and_ends_won() {
        let session = Session::default();
        let (sender, receiver) = unbounded_channel();
        session.play(preparing(), sending(sender)).await.unwrap();
        let cues = received(receiver).await;
        assert!(matches!(
            last(&cues),
            Some(Beat::Over {
                outcome: Outcome::Won {
                    by: Victory::Named,
                    ..
                }
            })
        ));
        let (text, saved) = kept(&session);
        assert_eq!(saved, cues);
        assert!(text.ends_with("{\"Ended\":\"won\"}}\n"), "{text}");
    }

    #[tokio::test]
    async fn a_session_plays_one_game_at_a_time_and_a_stopped_game_ends_stopped() {
        let session = Session::default();
        let (sender, receiver) = unbounded_channel();
        session.play(preparing(), sending(sender)).await.unwrap();
        let (refused, _) = unbounded_channel();
        let error = session
            .play(preparing(), sending(refused))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), BUSY);
        assert_eq!(session.log(), None);
        session.stop();
        let cues = received(receiver).await;
        let stopped = Beat::Over {
            outcome: Outcome::Stopped,
        };
        assert_eq!(last(&cues), Some(&stopped));
        assert!(!session.busy());
        let (text, saved) = kept(&session);
        assert_eq!(saved, cues);
        assert!(text.ends_with("{\"Ended\":\"stopped\"}}\n"), "{text}");
        assert!(session.log().unwrap().started <= SystemTime::now());
        let (sender, receiver) = unbounded_channel();
        session.play(preparing(), sending(sender)).await.unwrap();
        assert_eq!(session.log(), None);
        received(receiver).await;
        assert!(session.log().is_some());
    }

    #[tokio::test]
    async fn a_game_playing_is_not_finished_until_its_last_cue() {
        let (sender, receiver) = unbounded_channel();
        let playing = play(&prepared().await, sending(sender));
        assert!(!playing.is_finished());
        assert_eq!(playing.log(), None);
        playing.finished().await;
        let cues = received(receiver).await;
        assert!(matches!(last(&cues), Some(Beat::Over { .. })));
    }

    #[tokio::test]
    async fn a_closed_window_stops_the_game() {
        let (sender, receiver) = unbounded_channel();
        let mut shown = 0;
        play(&prepared().await, move |cue| {
            shown += 1;
            anyhow::ensure!(shown == 1, "the window closed");
            Ok(sender.send(cue)?)
        })
        .finished()
        .await;
        let cues = received(receiver).await;
        assert_eq!(cues.len(), 1, "{cues:?}");
        assert!(matches!(cues[0].beat, Beat::Cast { .. }), "{cues:?}");
    }

    #[tokio::test]
    async fn a_failed_episode_closes_the_calls_still_out_tells_its_error_and_ends_failed() {
        let (logger, events) = unbounded_channel::<Event<Entry>>();
        let thinking = Entry::Thinking {
            by: "Bob".to_string(),
        };
        logger.send(logged(thinking)).unwrap();
        drop(logger);
        let (unused, _) = unbounded_channel();
        let stopper = Episode::<crate::game::Actor>::new(HashMap::new(), unused).stopper();
        let (sender, receiver) = unbounded_channel();
        let kept = Arc::<OnceLock<_>>::default();
        let reader = Reader::new(
            sending(sender),
            stopper,
            Uuid::nil(),
            Secrets::default(),
            Arc::clone(&kept),
        );
        let run = async { Err(anyhow!("no model").context("actor Host failed")) };
        reader.read(events, run).await;
        let cues = received(receiver).await;
        let saved = String::from_utf8(kept.get().unwrap().to_vec()).unwrap();
        assert_eq!(cues, only(&saved));
        assert!(
            saved.ends_with("{\"Ended\":{\"failed\":\"actor Host failed: no model\"}}}\n"),
            "{saved}"
        );
        let beats: Vec<Beat> = cues.into_iter().map(|cue| cue.beat).collect();
        let error = "actor Host failed: no model".to_string();
        let who = "Bob".to_string();
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

    #[tokio::test]
    async fn a_game_stopped_just_as_it_is_prepared_never_starts() {
        let session = Session::default();
        let ready = prepared().await;
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
        let text = quick(&base_url, &format!("api_key_env = \"{CANARY_VARIABLE}\""));
        session
            .play(prepare(&text, &keychain), sending(sender))
            .await
            .unwrap();
        let cues = received(receiver).await;
        let (log, saved) = kept(&session);
        assert_redacted(&log, CANARY);
        let shown = serde_json::to_string(&cues).unwrap();
        assert!(!shown.contains(CANARY), "{shown}");
        assert_eq!(saved, cues);
        assert!(log.ends_with("{\"Ended\":\"won\"}}\n"), "{log}");
    }

    #[tokio::test]
    async fn a_game_that_cannot_be_prepared_is_an_error_and_frees_the_session() {
        let session = Session::default();
        session.stop();
        let (sender, _) = unbounded_channel();
        let failing = async { Err(anyhow!("no model")) };
        let error = session.play(failing, sending(sender)).await.unwrap_err();
        assert_eq!(error.to_string(), "no model");
        assert!(!session.busy());
        assert_eq!(session.log(), None);
    }
}
