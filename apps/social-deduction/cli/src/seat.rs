//! Social Deduction's seats: those every app shares, each numbered, the
//! environment's name, whom an actor sends to, and what a seat is dealt.

use free_agent::ActorId;
use std::collections::HashSet;

pub use free_agent_game::seat::{PlayerId, Seat, seats};

/// What a seat is dealt: a role of the game's `R`, and the strategy its
/// player plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dealt<R> {
    /// The role.
    pub role: R,
    /// The name of the strategy.
    pub strategy: String,
}

/// The seat numbered `number` named by its number: `Player 5`.
pub fn numbered(number: usize) -> PlayerId {
    format!("Player {number}")
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
    fn a_seat_named_by_its_number_is_player_a_space_and_the_number() {
        assert_eq!(numbered(5), "Player 5");
        assert_eq!(numbered(10), "Player 10");
        assert_eq!(
            seats(3, numbered).collect::<Vec<_>>(),
            ["Player 1", "Player 2", "Player 3"]
        );
    }

    #[test]
    fn everyone_but_me_is_the_environment_and_the_other_players() {
        let players = [id("ann"), id("bob"), id("cat")];
        assert_eq!(
            to_everyone_but(&id("bob"), &players),
            HashSet::from([ENVIRONMENT.to_string(), id("ann"), id("cat")])
        );
        assert_eq!(to_everyone_but(&id("dan"), &[]), to_the_environment());
        // The players alone, for speech, which the environment never hears.
        assert_eq!(
            to_the_players_but(&id("bob"), &players),
            HashSet::from([id("ann"), id("cat")])
        );
        assert!(to_the_players_but(&id("dan"), &[]).is_empty());
    }
}
