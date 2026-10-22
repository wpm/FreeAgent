//! The types Twenty Questions' native code shares with its WASM frontend.
//!
//! The frontend builds for `wasm32-unknown-unknown`, so it cannot depend on
//! `twenty-questions`, which needs Tokio, reqwest and the keychain. A type
//! or helper belongs here when the frontend reads, produces or checks it
//! and the native side has to agree with it. Everything the frontend does
//! not touch stays in `twenty-questions`.
//!
//! The screen shows one game in God mode, as a group chat under the secret
//! object, the guesses used and the time left. Library code folds the
//! game's log into a stream of [`Cue`]s, and the frontend draws each one as
//! it arrives: what the screen shows at any cue follows from the cues
//! before it. A log's games cross as [`Recordings`]. The desktop app's
//! [`settings`] cross as they are, and the [`go`] menu's moves cross the
//! same way.

#![warn(missing_docs)]

pub mod settings;

pub use free_agent_gui_types::go;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A name at the table, `Alice` or `Host`, which keys a seat everywhere on
/// screen.
pub type PlayerId = String;

/// Milliseconds since the Unix epoch, as the log stamps events. Plain
/// integers, because `SystemTime::now()` panics in wasm.
pub type Millis = u64;

/// One thing for the screen to show, stamped with the log time it happened
/// at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    /// When it happened, in log time.
    pub at: Millis,
    /// What happened.
    pub beat: Beat,
}

/// A log's episodes, each read as its own game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Recordings {
    /// Every episode that reads, in the order of each one's first line.
    pub games: Vec<Recording>,
    /// Why each episode left out was left out, naming the line.
    pub problems: Vec<String>,
}

/// One episode of a log, folded into cues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// The episode's ID, which every one of its lines carries.
    pub episode: Uuid,
    /// Its cues, the last of them its one [`Beat::Over`].
    pub cues: Vec<Cue>,
}

/// A game log opened for review: the file's name and its games.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    /// The file's name, as the header shows it.
    pub name: String,
    /// The log's games.
    pub recordings: Recordings,
}

/// What a cue shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Beat {
    /// The table and the rules. Always first.
    Cast {
        /// The host, then every player, in seat order.
        seats: Vec<Seat>,
        /// How many guesses the players have among them: the `20` of
        /// `3 / 20`.
        guesses: u64,
        /// How long the game lasts once it has [`Beat::Began`], which the
        /// countdown counts down.
        time_limit_ms: u64,
    },
    /// What the host is thinking of, shown for the whole game. It comes
    /// before the game begins, as soon as the object is known.
    Secret {
        /// The secret object: `a teapot`.
        object: String,
    },
    /// How fast log time runs against the screen's clock: 1.0 live. Sent
    /// right after `Cast`, and again on any change (playback speed, later).
    Clock {
        /// Log milliseconds per screen millisecond.
        rate: f64,
    },
    /// The players are told the game has begun: the countdown starts at
    /// this cue's time.
    Began,
    /// What one narrator call told, blank lines dropped: one centered
    /// block.
    Told(Vec<String>),
    /// Speech, the host's included, shown as a bubble from its speaker.
    Said {
        /// Who spoke.
        who: PlayerId,
        /// What they said.
        text: String,
    },
    /// A counted guess, as the host received it: a bubble from its asker,
    /// tagged with its number.
    Guessed {
        /// Who asked it.
        who: PlayerId,
        /// The question.
        text: String,
        /// Its number, from 1, which is also how many guesses are used.
        guess: u64,
    },
    /// The host's answer to a guess: a bubble from the host, tagged with
    /// the number of the guess it answers.
    Answered {
        /// The number of the guess it answers.
        guess: u64,
        /// The answer, in the host's words.
        text: String,
        /// Whether the guess named the secret object.
        named: bool,
    },
    /// A guess after the last, neither counted nor answered.
    Uncounted {
        /// Who asked it.
        who: PlayerId,
        /// The question.
        text: String,
    },
    /// A model call went out, the host's included: show its thinking dots.
    Thinking {
        /// Whose call went out.
        who: PlayerId,
    },
    /// The call is over, however it ended: take the dots away.
    Thought {
        /// Whose call is over.
        who: PlayerId,
    },
    /// The game is over, however it ended: the countdown stops at this
    /// cue's time. Always last.
    Over {
        /// How it ended.
        outcome: Outcome,
    },
}

/// One seat at the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    /// The name: `Alice`, or `Host`.
    pub name: PlayerId,
    /// The part the seat plays.
    pub part: Part,
}

/// The part a seat plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Part {
    /// The host, which holds the secret object and is drawn in ink.
    Host,
    /// A player.
    Player {
        /// The player's bubble color, an index into the palette: the
        /// player's place in seat order, from 0.
        color: u8,
        /// The strategy the player was dealt.
        strategy: String,
    },
}

/// A side of the game, one of which wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The players, who win by naming the object.
    Players,
    /// The host, who wins by keeping it.
    Host,
}

/// How a game is won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Victory {
    /// A guess named the object.
    Named,
    /// The last guess was answered without naming it.
    OutOfGuesses,
    /// The time limit came first.
    OutOfTime,
}

impl Victory {
    /// The side that wins this way.
    pub fn winner(self) -> Side {
        match self {
            Victory::Named => Side::Players,
            Victory::OutOfGuesses | Victory::OutOfTime => Side::Host,
        }
    }
}

/// How a game ended, shown at the foot of the chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// A side won.
    Won {
        /// How it was won, which says which side won.
        by: Victory,
        /// How it was won, as the narrator tells it.
        how: String,
    },
    /// The game was stopped before it ended.
    Stopped,
    /// The game failed, for the reason given.
    Failed(String),
    /// The log ends before the game does.
    Unfinished,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_the_object_wins_for_the_players_and_running_out_for_the_host() {
        assert_eq!(Victory::Named.winner(), Side::Players);
        assert_eq!(Victory::OutOfGuesses.winner(), Side::Host);
        assert_eq!(Victory::OutOfTime.winner(), Side::Host);
    }
}
