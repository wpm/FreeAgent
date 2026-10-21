//! A game's log as it is written down: one episode after another in JSON
//! Lines, each opening with a [`Header`] that names the log's format, the
//! version that wrote it and the game, and closing with how its run
//! [`Ended`]. One [`Writer`] writes every log, so a log written on the
//! command line and one the desktop app saves are written alike.
//!
//! A log carries no secrets. The writer replaces every key the run loaded,
//! its [`Secrets`], with
//! [`REDACTED`](free_agent_game::log::REDACTED), and writes every URL without its
//! userinfo and query.

use crate::config::Name;
use crate::game::{Entry, Game};
use anyhow::Context as _;
use free_agent::Event;
use free_agent_game::log::{Ended, Secrets};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::marker::PhantomData;
use std::time::SystemTime;
use uuid::Uuid;

/// The shape of the log this version writes. It goes up whenever the
/// log's shape changes. A log with no [`Header`] is format 0.
pub const LOG_FORMAT: u32 = 1;

/// The first line of every episode: what a reader needs to know before it
/// reads the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// The shape of the log, [`LOG_FORMAT`] when this version wrote it.
    pub format: u32,
    /// The version of `social-deduction` that wrote it.
    pub version: String,
    /// The game the episode plays.
    pub game: Name,
}

impl Header {
    /// The header this version writes for an episode of `G`.
    pub fn of<G: Game>() -> Self {
        Header {
            format: LOG_FORMAT,
            version: env!("CARGO_PKG_VERSION").to_string(),
            game: G::KEY,
        }
    }

    /// The header `line` holds, read as an event of any game, and `None`
    /// when it holds none, as in a format 0 log.
    ///
    /// # Errors
    ///
    /// A header of a format newer than [`LOG_FORMAT`], naming both, or a
    /// header that is not well formed.
    pub fn read(line: &str) -> anyhow::Result<Option<Self>> {
        let Ok(event) = serde_json::from_str::<Event<serde_json::Value>>(line) else {
            return Ok(None);
        };
        let Some(header) = event.payload.get("Header") else {
            return Ok(None);
        };
        /// What every format's header says first: its format, and who
        /// wrote it.
        #[derive(Deserialize)]
        struct Claimed {
            format: u32,
            version: String,
        }
        let Claimed { format, version } =
            Claimed::deserialize(header).context("the log's header is bad")?;
        anyhow::ensure!(
            format <= LOG_FORMAT,
            "This log was written by social-deduction {version} in log format {format}. \
             This version reads log formats up to {LOG_FORMAT}."
        );
        let header = Header::deserialize(header).context("the log's header is bad")?;
        Ok(Some(header))
    }
}

/// Writes one episode of a game of `G` to `sink`: its [`Header`] first,
/// then its events, each as [`Event::write`] writes it, and how it
/// [`Ended`] last, every line without its [`Secrets`].
pub struct Writer<G, W> {
    sink: W,
    episode: Uuid,
    secrets: Secrets,
    game: PhantomData<fn() -> G>,
}

impl<G: Game, W: Write> Writer<G, W> {
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
            game: PhantomData,
        };
        writer.write(&Event::now(episode, Entry::Header(Header::of::<G>())))?;
        Ok(writer)
    }

    /// Write `event`, one of the episode's, without its secrets.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails, or when the event cannot be
    /// serialized.
    pub fn write(&mut self, event: &Event<Entry<G>>) -> io::Result<()> {
        let mut line = Vec::new();
        event.write(&mut line)?;
        let line = self.secrets.redacted(&String::from_utf8_lossy(&line));
        self.sink.write_all(line.as_bytes())
    }

    /// Write how the episode `ended`, its last line, and give back the
    /// sink, flushed.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn end(self, ended: Ended) -> io::Result<W> {
        self.end_at(ended, SystemTime::now())
    }

    /// Write how the episode `ended`, stamped `time`, its last line, and
    /// give back the sink, flushed.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn end_at(mut self, ended: Ended, time: SystemTime) -> io::Result<W> {
        self.write(&Event {
            time,
            episode: self.episode,
            payload: Entry::Ended(ended),
        })?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::Avalon;
    use crate::game::tests::logged;
    use crate::werewolf::Werewolf;
    use secrecy::SecretString;

    fn episode() -> Uuid {
        Uuid::from_u128(7)
    }

    /// Each line `written` holds, read as an event of `G`.
    fn read<G: Game>(written: &[u8]) -> Vec<Event<Entry<G>>> {
        std::str::from_utf8(written)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn an_episode_is_written_between_its_header_and_how_it_ended() {
        let thinking = logged(Entry::<Avalon>::Thinking {
            player: "Player 1".to_string(),
            seq: 1,
        });
        let mut writer =
            Writer::<Avalon, _>::start(Vec::new(), episode(), Secrets::default()).unwrap();
        writer.write(&thinking).unwrap();
        let written = writer.end(Ended::Won).unwrap();
        let events = read::<Avalon>(&written);
        let payloads: Vec<&Entry<Avalon>> = events.iter().map(|event| &event.payload).collect();
        let header = Header::of::<Avalon>();
        assert_eq!(header.format, LOG_FORMAT);
        assert_eq!(header.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(header.game, Name::Avalon);
        assert_eq!(
            payloads,
            [
                &Entry::Header(header),
                &thinking.payload,
                &Entry::Ended(Ended::Won)
            ]
        );
        assert_eq!(events[0].episode, episode());
        assert_eq!(events[2].episode, episode());
    }

    #[test]
    fn the_header_and_the_end_are_written_as_a_reader_of_any_game_reads_them() {
        let writer =
            Writer::<Werewolf, _>::start(Vec::new(), episode(), Secrets::default()).unwrap();
        let written = writer.end(Ended::Failed("no model".to_string())).unwrap();
        let lines: Vec<serde_json::Value> = std::str::from_utf8(&written)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines[0]["payload"],
            serde_json::json!({"Header": {
                "format": 1,
                "version": env!("CARGO_PKG_VERSION"),
                "game": "werewolf",
            }})
        );
        assert_eq!(
            lines[1]["payload"],
            serde_json::json!({"Ended": {"failed": "no model"}})
        );
        let won = serde_json::to_value(Entry::<Werewolf>::Ended(Ended::Won)).unwrap();
        assert_eq!(won, serde_json::json!({"Ended": "won"}));
        let stopped = serde_json::to_value(Entry::<Werewolf>::Ended(Ended::Stopped)).unwrap();
        assert_eq!(stopped, serde_json::json!({"Ended": "stopped"}));
    }

    /// `payload` as a line of a log.
    fn line(payload: serde_json::Value) -> String {
        serde_json::to_string(&logged(payload)).unwrap()
    }

    #[test]
    fn an_episode_ended_at_a_time_is_stamped_with_it() {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_234);
        let written = Writer::<Avalon, _>::start(Vec::new(), episode(), Secrets::default())
            .unwrap()
            .end_at(Ended::Stopped, time)
            .unwrap();
        let ended = read::<Avalon>(&written).pop().unwrap();
        assert_eq!(ended.time, time);
        assert_eq!(ended.payload, Entry::Ended(Ended::Stopped));
    }

    #[test]
    fn a_header_is_read_from_a_line_of_any_game() {
        let written = Writer::<Werewolf, _>::start(Vec::new(), episode(), Secrets::default())
            .unwrap()
            .end(Ended::Stopped)
            .unwrap();
        let first = std::str::from_utf8(&written)
            .unwrap()
            .lines()
            .next()
            .unwrap();
        assert_eq!(Header::read(first).unwrap(), Some(Header::of::<Werewolf>()));
    }

    #[test]
    fn a_line_with_no_header_is_a_format_0_log() {
        let start = line(serde_json::json!({"Start": {}}));
        assert_eq!(Header::read(&start).unwrap(), None);
        assert_eq!(Header::read("{}").unwrap(), None);
    }

    #[test]
    fn a_header_of_a_newer_format_is_refused_naming_both() {
        let header = line(serde_json::json!({"Header": {
            "format": 2,
            "version": "0.3.0",
            "game": "chess",
        }}));
        let error = Header::read(&header).unwrap_err();
        assert_eq!(
            error.to_string(),
            "This log was written by social-deduction 0.3.0 in log format 2. \
             This version reads log formats up to 1."
        );
    }

    /// The secrets `keys` are.
    fn secrets(keys: &[&str]) -> Secrets {
        keys.iter().copied().map(SecretString::from).collect()
    }

    /// What a writer with `secrets` writes of an episode that failed with
    /// `error`, as text.
    fn failed_with(secrets: Secrets, error: &str) -> String {
        let writer = Writer::<Werewolf, _>::start(Vec::new(), episode(), secrets).unwrap();
        let written = writer.end(Ended::Failed(error.to_string())).unwrap();
        String::from_utf8(written).unwrap()
    }

    #[test]
    fn every_secret_is_written_redacted_and_the_log_still_reads() {
        let written = failed_with(
            secrets(&["sk-one", "sk-two", "sk-one", ""]),
            "sk-one was refused, as was sk-two, and then sk-one again",
        );
        assert!(!written.contains("sk-one") && !written.contains("sk-two"));
        let ended = written.lines().nth(1).unwrap();
        let ended: Event<Entry<Werewolf>> = serde_json::from_str(ended).unwrap();
        assert_eq!(
            ended.payload,
            Entry::Ended(Ended::Failed(
                "[redacted] was refused, as was [redacted], and then [redacted] again".to_string()
            ))
        );
    }

    #[test]
    fn a_secret_with_characters_json_escapes_is_redacted() {
        let key = "sk-\"quoted\"\\back\tslash\u{1}";
        let written = failed_with(secrets(&[key]), &format!("the key {key} was refused"));
        assert!(
            written.contains("the key [redacted] was refused"),
            "{written}"
        );
        let escaped = serde_json::Value::from(key).to_string();
        assert!(
            !written.contains(&escaped[1..escaped.len() - 1]),
            "{written}"
        );
    }

    #[test]
    fn a_header_that_is_not_well_formed_is_an_error() {
        for header in [
            serde_json::json!({"Header": {"format": "one"}}),
            serde_json::json!({"Header": {"format": 1, "version": "0.1.0", "game": "chess"}}),
        ] {
            let error = Header::read(&line(header)).unwrap_err();
            assert_eq!(error.to_string(), "the log's header is bad");
        }
    }
}
