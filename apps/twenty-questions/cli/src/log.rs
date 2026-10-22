//! What the log holds, an [`Entry`] at a time, and how it is written down:
//! one episode after another in JSON Lines, each opening with a [`Header`]
//! that names the log's format, the version that wrote it and the game,
//! and closing with how its run [`Ended`]. One [`Writer`] writes every log.
//!
//! The log is written by every actor, in the language of reinforcement
//! learning: each message an actor sends to others is an action of its
//! sender, which the sender logs whole, once, and an observation of each
//! receiver, which the receiver logs as a receipt naming the message by its
//! sender and number.
//!
//! A log carries no secrets. The writer replaces every key the run loaded,
//! its [`Secrets`], with [`REDACTED`](free_agent_game::log::REDACTED), and
//! writes every URL without its userinfo and query.

use crate::config::Configuration;
use crate::message::Message;
use anyhow::Context as _;
use free_agent::{ActorId, Event};
use free_agent_game::log::{Ended, Secrets};
use free_agent_game::seat::PlayerId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// The shape of the log this version writes. It goes up whenever the
/// log's shape changes.
pub const LOG_FORMAT: u32 = 1;

/// The game a log's [`Header`] names.
pub const GAME: &str = "twenty-questions";

/// What the log holds: each episode's header, the table, the configuration,
/// the secret object when the host picked it, every message sent as the
/// action of its sender and the observation of its receiver, each model
/// call going out and coming back with the usage its provider reported,
/// each failure to act, each guess that came too late to count, and how
/// the game ended. The log's writer writes the header and how the episode
/// ended; the host logs the table, the configuration, the object, the late
/// guesses and the end; and every actor logs its own actions,
/// observations, calls and failures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    /// The first line of every episode, written by the log's [`Writer`].
    Header(Header),
    /// The game begins: who plays, with which strategy, how many guesses
    /// they have, and how long the game lasts.
    Start {
        /// Every player's strategy, in seat order.
        strategies: BTreeMap<PlayerId, String>,
        /// How many guesses the players have among them.
        guesses: u64,
        /// How long the game lasts.
        #[serde(with = "humantime_serde")]
        time_limit: Duration,
    },
    /// The configuration the game ran under, logged right after the start.
    Configuration(Box<Configuration>),
    /// The host picked the secret object, drawing `category` for it when
    /// the configuration has categories.
    Chosen {
        /// The secret object.
        object: String,
        /// The category the pick was asked for.
        category: Option<String>,
    },
    /// `from` sent `message`, which it numbered `n`, to everyone in `to`.
    /// Logged by the sender.
    Action {
        /// Who sent it.
        from: ActorId,
        /// The sender's number for it.
        n: u64,
        /// Everyone it was sent to.
        to: HashSet<ActorId>,
        /// What was sent.
        message: Message,
        /// How many memories the model was shown when it made the call
        /// that produced the message, and `None` for a message no model
        /// made.
        seen: Option<usize>,
        /// How many lines of talk the call that produced the message left
        /// out to fit the context window, `None` when it left out none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forgot: Option<usize>,
    },
    /// `by` received the message `from` numbered `n`. Logged by the
    /// receiver, naming the message without repeating it: the sender's
    /// `Action` holds it.
    Observation {
        /// Who received it.
        by: ActorId,
        /// Who sent it.
        from: ActorId,
        /// The sender's number for it.
        n: u64,
    },
    /// `by` failed to act: its model was not reached, or answered with
    /// something that is not a well-formed response, or the host's answer
    /// said the secret object's name.
    Failed {
        /// Who failed.
        by: ActorId,
        /// What went wrong, as the error says it, causes included.
        error: String,
    },
    /// `by` called its model. Its `Thought` follows unless it is stopped
    /// first, by the end of the game.
    Thinking {
        /// Who made the call.
        by: ActorId,
    },
    /// The call `by` made came back, as `ending` says. Logged as soon as
    /// the call returns, before its usage and before the answer is judged.
    Thought {
        /// Who made the call.
        by: ActorId,
        /// How the call came back.
        ending: Ending,
    },
    /// The model answered a call `by` made, and its provider reported
    /// `usage` for it, as sent.
    Usage {
        /// Who made the call.
        by: ActorId,
        /// What the provider reported, as sent.
        usage: serde_json::Value,
    },
    /// The guess `from` numbered `n` came after the last guess the players
    /// had, and is neither counted nor answered. Logged by the host.
    Uncounted {
        /// Who asked it.
        from: PlayerId,
        /// The player's number for it.
        n: u64,
    },
    /// The guess `from` numbered `n` repeats one put already, as when two
    /// players put the same guess at once, and is neither counted nor
    /// answered. Logged by the host.
    Repeated {
        /// Who asked it.
        from: PlayerId,
        /// The player's number for it.
        n: u64,
    },
    /// The game is over: who won and how, how many guesses the players
    /// used, the secret object, and how long the game took. Logged by the
    /// host.
    End {
        /// The side that won.
        winner: Side,
        /// How it won.
        by: Victory,
        /// How many guesses were answered.
        used: u64,
        /// The secret object.
        object: String,
        /// How long the game took, from the players' start.
        #[serde(with = "humantime_serde")]
        took: Duration,
    },
    /// How the episode's run came back, the last line of every episode,
    /// written by the log's [`Writer`].
    Ended(Ended),
}

/// How a model call came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ending {
    /// The provider answered. Whatever the answer leads to is logged after
    /// it.
    Answered,
    /// The call failed, with the error written as it says itself, causes
    /// included.
    Failed(String),
}

/// The sides of the game and how it is won, as the screen shows them too.
pub use twenty_questions_gui_types::{Side, Victory};

/// The first line of every episode: what a reader needs to know before it
/// reads the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// The shape of the log, [`LOG_FORMAT`] when this version wrote it.
    pub format: u32,
    /// The version of `twenty-questions` that wrote it.
    pub version: String,
    /// The game, [`GAME`].
    pub game: String,
}

impl Header {
    /// The header this version writes.
    pub fn new() -> Self {
        Header {
            format: LOG_FORMAT,
            version: env!("CARGO_PKG_VERSION").to_string(),
            game: GAME.to_string(),
        }
    }

    /// Check the header `line` holds, if it holds one: its game is this
    /// one, and its format one this version reads. Only what every format
    /// says is read, so a newer header of any shape is refused as newer.
    ///
    /// # Errors
    ///
    /// When the header is bad, is of another game, or is of a newer format
    /// than this version reads.
    pub fn check(line: &str) -> anyhow::Result<()> {
        let Ok(event) = serde_json::from_str::<Event<serde_json::Value>>(line) else {
            return Ok(());
        };
        let Some(header) = event.payload.get("Header") else {
            return Ok(());
        };
        /// What every format's header says: its format, who wrote it, and
        /// the game.
        #[derive(Deserialize)]
        struct Claimed {
            format: u32,
            version: String,
            game: String,
        }
        let Claimed {
            format,
            version,
            game,
        } = Claimed::deserialize(header).context("the log's header is bad")?;
        anyhow::ensure!(game == GAME, "This log is of {game}, not {GAME}.");
        anyhow::ensure!(
            format <= LOG_FORMAT,
            "This log was written by {GAME} {version} in log format {format}. \
             This version reads log formats up to {LOG_FORMAT}."
        );
        Ok(())
    }
}

impl Default for Header {
    fn default() -> Self {
        Self::new()
    }
}

/// Writes one episode to `sink`: its [`Header`] first, then its events,
/// each as [`Event::write`] writes it, and how it [`Ended`] last, every
/// line without its [`Secrets`].
pub struct Writer<W> {
    sink: W,
    episode: Uuid,
    secrets: Secrets,
}

impl<W: Write> Writer<W> {
    /// A writer of the episode `episode` to `sink`, which has written the
    /// episode's header, and writes no line with any of `secrets` in it.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn start(sink: W, episode: Uuid, secrets: Secrets) -> io::Result<Self> {
        let mut writer = Writer {
            sink,
            episode,
            secrets,
        };
        writer.write(&Event::now(episode, Entry::Header(Header::new())))?;
        Ok(writer)
    }

    /// Write `event`, one of the episode's, without its secrets, and give
    /// back the line written.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails, or when the event cannot be
    /// serialized.
    pub fn write(&mut self, event: &Event<Entry>) -> io::Result<String> {
        let mut line = Vec::new();
        event.write(&mut line)?;
        let line = self.secrets.redacted(&String::from_utf8_lossy(&line));
        self.sink.write_all(line.as_bytes())?;
        Ok(line)
    }

    /// Write how the episode `ended`, its last line, and give back the
    /// sink, flushed.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn end(self, ended: Ended) -> io::Result<W> {
        Ok(self.end_at(ended, SystemTime::now())?.0)
    }

    /// Write how the episode `ended`, stamped `time`, its last line, and
    /// give back the sink, flushed, and how it ended as written.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn end_at(mut self, ended: Ended, time: SystemTime) -> io::Result<(W, Ended)> {
        let line = self.write(&Event {
            time,
            episode: self.episode,
            payload: Entry::Ended(ended),
        })?;
        self.sink.flush()?;
        Ok((self.sink, Ended::read(&line)?))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use secrecy::SecretString;
    use serde_json::json;

    /// `payload`, an entry or a memory, as an event logged or remembered
    /// when the game began, in no episode in particular.
    pub(crate) fn logged<L>(payload: L) -> Event<L> {
        Event {
            time: SystemTime::UNIX_EPOCH,
            episode: Uuid::default(),
            payload,
        }
    }

    /// `payload`, logged `seconds` into the game.
    pub(crate) fn logged_at<L>(seconds: u64, payload: L) -> Event<L> {
        Event {
            time: SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
            ..logged(payload)
        }
    }

    fn episode() -> Uuid {
        Uuid::from_u128(7)
    }

    /// Each line `written` holds, read as an event.
    fn read(written: &[u8]) -> Vec<Event<Entry>> {
        std::str::from_utf8(written)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn an_episode_is_written_between_its_header_and_how_it_ended() {
        let thinking = logged(Entry::Thinking {
            by: "Alice".to_string(),
        });
        let mut writer = Writer::start(Vec::new(), episode(), Secrets::default()).unwrap();
        writer.write(&thinking).unwrap();
        let written = writer.end(Ended::Won).unwrap();
        let events = read(&written);
        let payloads: Vec<&Entry> = events.iter().map(|event| &event.payload).collect();
        assert_eq!(
            payloads,
            [
                &Entry::Header(Header::default()),
                &thinking.payload,
                &Entry::Ended(Ended::Won)
            ]
        );
        assert_eq!(events[0].episode, episode());
        assert_eq!(events[2].episode, episode());
        let header: serde_json::Value =
            serde_json::from_slice(written.split(|b| *b == b'\n').next().unwrap()).unwrap();
        assert_eq!(
            header["payload"],
            json!({"Header": {"format": 1, "version": env!("CARGO_PKG_VERSION"), "game": "twenty-questions"}})
        );
    }

    /// The header line of `payload`, as an event's.
    fn header_line(payload: serde_json::Value) -> String {
        serde_json::to_string(&logged(json!({ "Header": payload }))).unwrap()
    }

    #[test]
    fn a_header_this_version_wrote_and_a_line_that_is_no_header_pass() {
        let written = Writer::start(Vec::new(), episode(), Secrets::default()).unwrap();
        let written = written.end(Ended::Won).unwrap();
        let text = String::from_utf8(written).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for line in [lines[0], lines[1], "not a line"] {
            Header::check(line).unwrap();
        }
    }

    #[test]
    fn a_header_of_another_game_a_newer_format_or_a_bad_shape_is_refused() {
        let werewolf = header_line(json!({"format": 1, "version": "0.2.0", "game": "werewolf"}));
        assert_eq!(
            Header::check(&werewolf).unwrap_err().to_string(),
            "This log is of werewolf, not twenty-questions."
        );
        let newer = header_line(json!({"format": 2, "version": "0.3.0", "game": GAME, "new": 1}));
        assert_eq!(
            Header::check(&newer).unwrap_err().to_string(),
            "This log was written by twenty-questions 0.3.0 in log format 2. \
             This version reads log formats up to 1."
        );
        for bad in [
            json!({"format": 1}),
            json!({"format": 1, "version": 1, "game": GAME}),
        ] {
            let error = Header::check(&header_line(bad)).unwrap_err();
            assert_eq!(error.to_string(), "the log's header is bad");
        }
    }

    #[test]
    fn every_secret_is_written_redacted_and_the_log_still_reads() {
        let secrets: Secrets = ["sk-one", "sk-two"]
            .into_iter()
            .map(SecretString::from)
            .collect();
        let writer = Writer::start(Vec::new(), episode(), secrets).unwrap();
        let error = "sk-one was refused at https://user:sk-two@example.com/v1?key=sk-one";
        let written = writer.end(Ended::Failed(error.to_string())).unwrap();
        let text = String::from_utf8(written.clone()).unwrap();
        assert!(
            !text.contains("sk-one") && !text.contains("sk-two"),
            "{text}"
        );
        assert_eq!(
            read(&written)[1].payload,
            Entry::Ended(Ended::Failed(
                "[redacted] was refused at https://example.com/v1".to_string()
            ))
        );
    }

    #[test]
    fn the_end_says_who_won_and_how_and_round_trips() {
        let end = Entry::End {
            winner: Side::Host,
            by: Victory::OutOfTime,
            used: 7,
            object: "a teapot".to_string(),
            took: Duration::from_secs(600),
        };
        let written = serde_json::to_value(&end).unwrap();
        assert_eq!(
            written,
            json!({"End": {"winner": "host", "by": "out_of_time", "used": 7, "object": "a teapot", "took": "10m"}})
        );
        assert_eq!(serde_json::from_value::<Entry>(written).unwrap(), end);
    }
}
