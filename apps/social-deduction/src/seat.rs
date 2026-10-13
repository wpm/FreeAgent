//! What every game shares about its seats: a player's name, the
//! environment's, and whom an actor sends to.

use free_agent::ActorId;
use std::collections::HashSet;
use std::fmt;

/// A player is an actor, named as the episode names it.
pub type PlayerId = ActorId;

/// A player's name as seat order reads it: a word, and the number it ends
/// in, if any. Names order alphabetically by the word, whatever its case,
/// then by the number, so `ann` comes before `Player 2`, and `Player 2`
/// before `Player 10`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlayerName {
    /// The word in lower case, which orders it alphabetically.
    folded: String,
    /// The number the name ends in.
    number: Option<u64>,
    /// The name as written, which orders names the rest leaves tied.
    name: String,
}

impl PlayerName {
    /// The name Free Agent gives the seat numbered `number`: `Player 5`.
    pub fn seat(number: usize) -> Self {
        Self::from(format!("Player {number}").as_str())
    }
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

/// The seats of a table of `players`, named `Player 1` onward. The names
/// say nothing about the roles, since every player sees every name.
pub fn seats(players: usize) -> impl Iterator<Item = PlayerId> {
    (1..=players).map(|seat| PlayerName::seat(seat).to_string())
}

/// The environment's name in the episode.
pub const ENVIRONMENT: &str = "environment";

/// The environment alone, as the receivers of a message: whom a player
/// sends to when no other player is to hear.
pub fn to_the_environment() -> HashSet<ActorId> {
    HashSet::from([ENVIRONMENT.to_string()])
}

/// Everyone in `players` but `me`, as the receivers of a message: whom a
/// player speaks to, since the environment does not hear speech.
pub fn to_the_players_but<'a>(
    me: &PlayerId,
    players: impl IntoIterator<Item = &'a PlayerId>,
) -> HashSet<ActorId> {
    players
        .into_iter()
        .filter(|player| *player != me)
        .cloned()
        .collect()
}

/// The environment and everyone in `players` but `me`, as the receivers of
/// a message: whom a player sends to when what it says is public.
pub fn to_everyone_but<'a>(
    me: &PlayerId,
    players: impl IntoIterator<Item = &'a PlayerId>,
) -> HashSet<ActorId> {
    let mut to = to_the_players_but(me, players);
    to.insert(ENVIRONMENT.to_string());
    to
}

/// The player named `name`, as a test names it.
#[cfg(test)]
pub(crate) fn id(name: &str) -> PlayerId {
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seat_is_named_player_a_space_and_its_number() {
        assert_eq!(PlayerName::seat(5).to_string(), "Player 5");
        assert_eq!(PlayerName::seat(10).to_string(), "Player 10");
    }

    #[test]
    fn a_name_reads_as_its_word_and_the_number_it_ends_in() {
        assert_eq!(PlayerName::from("Player 10"), PlayerName::seat(10));
        assert_eq!(PlayerName::from("ann").to_string(), "ann");
        // Names the word and number leave tied order as written.
        assert!(PlayerName::from("Player 2") < PlayerName::from("Player2"));
    }

    #[test]
    fn a_table_is_seated_from_player_1() {
        assert_eq!(
            seats(3).collect::<Vec<_>>(),
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

    #[test]
    fn everyone_but_me_is_the_environment_and_the_other_players() {
        let players = ["ann".to_string(), "bob".to_string(), "cat".to_string()];
        assert_eq!(
            to_everyone_but(&"bob".to_string(), &players),
            HashSet::from([
                ENVIRONMENT.to_string(),
                "ann".to_string(),
                "cat".to_string()
            ])
        );
        assert_eq!(
            to_everyone_but(&"dan".to_string(), &[]),
            to_the_environment()
        );
        // The players alone, for speech, which the environment never hears.
        assert_eq!(
            to_the_players_but(&"bob".to_string(), &players),
            HashSet::from(["ann".to_string(), "cat".to_string()])
        );
        assert!(to_the_players_but(&"dan".to_string(), &[]).is_empty());
    }
}
