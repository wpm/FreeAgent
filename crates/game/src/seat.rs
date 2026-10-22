//! What every game shares about its seats: a player's name, the order
//! names are seated in, and how a seat's player is built.

use free_agent::{ActorId, Behavior, Builder, ThinkBuilder};
use std::fmt;

/// A player is an actor, named as the episode names it.
pub type PlayerId = ActorId;

/// How a seat is built: its player's behavior, an actor `A`, and its think
/// loop if it has one.
pub type Seat<A> = (
    Builder<A>,
    Option<ThinkBuilder<<A as Behavior>::Message, <A as Behavior>::Log>>,
);

/// A player's name as seat order reads it: a word, and the number it ends
/// in, if any. Names order alphabetically by the word, whatever its case,
/// then by the number, so `alice` comes before `Bob`, `Bob` before
/// `Player 2`, and `Player 2` before `Player 10`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlayerName {
    /// The word in lower case, which orders it alphabetically.
    folded: String,
    /// The number the name ends in.
    number: Option<u64>,
    /// The name as written, which orders names the rest leaves tied.
    name: String,
}

impl From<&str> for PlayerName {
    /// `name` read as its word and the number it ends in, so that
    /// `Player 10` is the word `Player` and the number 10, and `ann` the
    /// word alone.
    fn from(name: &str) -> Self {
        let word = name.trim_end_matches(|c: char| c.is_ascii_digit());
        Self {
            folded: word.trim_end().to_lowercase(),
            number: name[word.len()..].parse().ok(),
            name: name.to_string(),
        }
    }
}

impl fmt::Display for PlayerName {
    /// The name as written: `Player 5`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// The seats of a table of `players`, numbered from 1, each named as
/// `named` names its number. The names say nothing about the roles, since
/// every player sees every name.
pub fn seats(players: usize, named: impl Fn(usize) -> PlayerId) -> impl Iterator<Item = PlayerId> {
    (1..=players).map(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_reads_as_its_word_and_the_number_it_ends_in() {
        let player_10 = PlayerName::from("Player 10");
        assert_eq!(
            (player_10.folded.as_str(), player_10.number),
            ("player", Some(10))
        );
        assert_eq!(PlayerName::from("ann").to_string(), "ann");
        // Names the word and number leave tied order as written.
        assert!(PlayerName::from("Player 2") < PlayerName::from("Player2"));
    }

    #[test]
    fn a_table_is_seated_from_seat_1_as_the_game_names_its_seats() {
        let cast = |number: usize| ["Alice", "Bob"][number - 1].to_string();
        assert_eq!(seats(2, cast).collect::<Vec<_>>(), ["Alice", "Bob"]);
        let numbered = |number: usize| format!("Player {number}");
        assert_eq!(
            seats(3, numbered).collect::<Vec<_>>(),
            ["Player 1", "Player 2", "Player 3"]
        );
    }

    #[test]
    fn names_order_by_word_then_number() {
        let mut names: Vec<PlayerName> = ["Player 10", "ann", "Player 2", "Player 1"]
            .map(PlayerName::from)
            .into();
        names.sort();
        assert_eq!(
            names.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["ann", "Player 1", "Player 2", "Player 10"]
        );
    }
}
