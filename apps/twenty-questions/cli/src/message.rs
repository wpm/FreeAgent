//! What the host and the players say to one another: a [`Message`]. Every
//! message an actor sends to others is numbered by its sender, from 1, so
//! that the pair of sender and number names a message in an episode. A
//! reminder an actor sends itself is not numbered.

use free_agent::ActorId;
use free_agent_game::seat::PlayerId;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The host's name in the episode.
pub const HOST: &str = "Host";

/// What a player is shown of the game as it begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Everyone at the table, in seat order.
    pub players: Vec<PlayerId>,
    /// How many guesses the players have among them.
    pub guesses: u64,
    /// How long the game lasts, written as `humantime` writes it.
    #[serde(with = "humantime_serde")]
    pub time_limit: Duration,
}

/// What the host and the players say to one another, and what each says
/// to itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// The host tells every player the game has begun.
    Begin {
        /// The host's number for this message.
        n: u64,
        /// What the players are shown.
        view: View,
    },
    /// Speech, from anyone to everyone else.
    Say {
        /// Who spoke, since a message does not carry its sender.
        from: ActorId,
        /// The speaker's number for this message.
        n: u64,
        /// What they said.
        text: String,
    },
    /// A yes-or-no question a player puts formally to the host, which
    /// everyone else hears too. It uses one of the guesses if any are left.
    Guess {
        /// Who asked.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// The question.
        text: String,
    },
    /// The host's answer to a guess, to every player.
    Answer {
        /// The host's number for this message.
        n: u64,
        /// Which guess this answers, numbered from 1 in the order the host
        /// received them.
        guess: u64,
        /// Who asked it.
        asked_by: PlayerId,
        /// The answer, in the host's words.
        text: String,
        /// Whether the guess names the secret object, which wins the game
        /// for the players.
        named: bool,
        /// How many guesses are left once those the host has received are
        /// counted.
        left: u64,
    },
    /// The host's own message to itself that the game's time is up.
    Over,
    /// An actor's own message to its think loop to call the model.
    Wake,
    /// An actor's own reminder, the `count`th it set, to be woken. A
    /// timer cannot be canceled, so one whose `count` is not the latest
    /// set is ignored.
    Remind {
        /// Which reminder of the actor's this is.
        count: u64,
    },
}

impl free_agent::Message for Message {}

impl Message {
    /// The sender's number for this message, which a message to itself
    /// does not have.
    pub fn number(&self) -> Option<u64> {
        match self {
            Message::Begin { n, .. }
            | Message::Say { n, .. }
            | Message::Guess { n, .. }
            | Message::Answer { n, .. } => Some(*n),
            Message::Over | Message::Wake | Message::Remind { .. } => None,
        }
    }

    /// Who sent this message: the speaker of speech, the player who
    /// guessed, and otherwise the host.
    pub fn sender(&self) -> ActorId {
        match self {
            Message::Say { from, .. } | Message::Guess { from, .. } => from.clone(),
            _ => HOST.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn said() -> Message {
        Message::Say {
            from: "Bob".to_string(),
            n: 3,
            text: "Is it alive?".to_string(),
        }
    }

    #[test]
    fn a_message_to_others_is_numbered_and_one_to_itself_is_not() {
        assert_eq!(said().number(), Some(3));
        let answer = Message::Answer {
            n: 2,
            guess: 1,
            asked_by: "Bob".to_string(),
            text: "No.".to_string(),
            named: false,
            left: 19,
        };
        assert_eq!(answer.number(), Some(2));
        for own in [Message::Over, Message::Wake, Message::Remind { count: 1 }] {
            assert_eq!(own.number(), None, "{own:?}");
        }
    }

    #[test]
    fn speech_and_guesses_are_sent_by_who_made_them_and_everything_else_by_the_host() {
        assert_eq!(said().sender(), "Bob");
        let guess = Message::Guess {
            from: "Carol".to_string(),
            n: 1,
            text: "Is it a teapot?".to_string(),
        };
        assert_eq!(guess.sender(), "Carol");
        let begin = Message::Begin {
            n: 1,
            view: View {
                players: vec!["Alice".to_string()],
                guesses: 20,
                time_limit: Duration::from_secs(600),
            },
        };
        assert_eq!(begin.sender(), HOST);
        assert_eq!(begin.number(), Some(1));
        assert_eq!(
            serde_json::to_value(&begin).unwrap(),
            json!({"Begin": {"n": 1, "view": {"players": ["Alice"], "guesses": 20, "time_limit": "10m"}}})
        );
    }
}
