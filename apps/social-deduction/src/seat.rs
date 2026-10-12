//! What every game shares about its seats: a player's name, the
//! environment's, and whom an actor sends to.

use free_agent::ActorId;
use std::collections::HashSet;

/// A player is an actor, named as the episode names it.
pub type PlayerId = ActorId;

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

#[cfg(test)]
mod tests {
    use super::*;

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
