//! A game's log as it is written down: one episode after another in JSON
//! Lines, each opening with a [`Header`] that names the log's format, the
//! version that wrote it and the game, and closing with how its run
//! [`Ended`]. One [`Writer`] writes every log, so a log written on the
//! command line and one the desktop app saves are written alike.

use crate::config::Name;
use crate::game::{Entry, Game};
use anyhow::Context as _;
use free_agent::Event;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::marker::PhantomData;
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

/// The last line of every episode: how its run came back. An episode with
/// no `Ended` line is cut off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    /// A side won.
    Won,
    /// The episode was stopped before anyone won.
    Stopped,
    /// The episode failed, with the error written as it says itself,
    /// causes included.
    Failed(String),
}

impl Ended {
    /// How an episode whose run came back `ran` ended, a side having won
    /// when `won`. A failure is a failure even after a win.
    pub fn of(ran: &anyhow::Result<()>, won: bool) -> Self {
        match ran {
            Err(error) => Ended::Failed(format!("{error:#}")),
            Ok(()) if won => Ended::Won,
            Ok(()) => Ended::Stopped,
        }
    }
}

/// Writes one episode of a game of `G` to `sink`: its [`Header`] first,
/// then its events, each as [`Event::write`] writes it, and how it
/// [`Ended`] last.
pub struct Writer<G, W> {
    sink: W,
    /// Each line as it is written, kept between lines for its capacity.
    line: Vec<u8>,
    episode: Uuid,
    game: PhantomData<fn() -> G>,
}

impl<G: Game, W: Write> Writer<G, W> {
    /// A writer of the episode `episode` to `sink`, which has written the
    /// episode's header.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn start(sink: W, episode: Uuid) -> io::Result<Self> {
        let mut writer = Writer {
            sink,
            line: Vec::new(),
            episode,
            game: PhantomData,
        };
        writer.stamped(Entry::Header(Header::of::<G>()))?;
        Ok(writer)
    }

    /// Write `event`, one of the episode's.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails, or when the event cannot be
    /// serialized.
    pub fn write(&mut self, event: &Event<Entry<G>>) -> io::Result<()> {
        self.line.clear();
        event.write(&mut self.line)?;
        self.sink.write_all(&self.line)
    }

    /// Write how the episode `ended`, its last line, and give back the
    /// sink, flushed.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails.
    pub fn end(mut self, ended: Ended) -> io::Result<W> {
        self.stamped(Entry::Ended(ended))?;
        self.sink.flush()?;
        Ok(self.sink)
    }

    /// Write `entry` as an event of the episode, stamped now.
    fn stamped(&mut self, entry: Entry<G>) -> io::Result<()> {
        self.write(&Event::now(self.episode, entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::Avalon;
    use crate::game::tests::logged;
    use crate::werewolf::Werewolf;

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
        let mut writer = Writer::<Avalon, _>::start(Vec::new(), episode()).unwrap();
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
        let writer = Writer::<Werewolf, _>::start(Vec::new(), episode()).unwrap();
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

    #[test]
    fn a_run_that_fails_ends_failed_one_that_comes_back_after_a_win_won_and_otherwise_stopped() {
        let failed = Err(anyhow::anyhow!("no model").context("actor Player 1 failed"));
        assert_eq!(
            Ended::of(&failed, true),
            Ended::Failed("actor Player 1 failed: no model".to_string())
        );
        assert_eq!(Ended::of(&Ok(()), true), Ended::Won);
        assert_eq!(Ended::of(&Ok(()), false), Ended::Stopped);
    }

    /// `payload` as a line of a log.
    fn line(payload: serde_json::Value) -> String {
        serde_json::to_string(&logged(payload)).unwrap()
    }

    #[test]
    fn a_header_is_read_from_a_line_of_any_game() {
        let written = Writer::<Werewolf, _>::start(Vec::new(), episode())
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
