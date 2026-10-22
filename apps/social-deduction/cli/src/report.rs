//! What Social Deduction's games share in telling their stories, besides
//! what every app shares: seat order, how a player is named with its role,
//! and whether a message comes late.

use crate::seat::PlayerId;
use free_agent_game::seat::PlayerName;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub use free_agent_game::report::{
    FORGOTTEN, Line, configured, in_order, in_prose, told_within, whole_seconds,
};

/// Seat order: the order of the [`PlayerName`]s, so that `Player 2`
/// comes before `Player 10`.
pub fn seat_order(a: &PlayerId, b: &PlayerId) -> Ordering {
    PlayerName::from(a.as_str()).cmp(&PlayerName::from(b.as_str()))
}

/// A player as an account names it: `player (role)` when the role is
/// known, the name alone when it is not.
#[derive(Debug, Clone, Copy)]
pub struct Seat<'a, R> {
    /// The player.
    pub player: &'a PlayerId,
    /// Its role, when the account knows it.
    pub role: Option<R>,
}

impl<'a, R: Copy> Seat<'a, R> {
    /// `player` as an account knowing `roles` names it.
    pub fn named(player: &'a PlayerId, roles: &HashMap<PlayerId, R>) -> Self {
        Seat {
            player,
            role: roles.get(player).copied(),
        }
    }
}

impl<R: fmt::Display> fmt::Display for Seat<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.role {
            Some(role) => write!(f, "{} ({role})", self.player),
            None => f.write_str(self.player),
        }
    }
}

/// Whether a later phase than the one numbered `seq` is among those
/// `announced`, by number: a message for that phase comes late.
pub fn late<V>(announced: &BTreeMap<u64, V>, seq: u64) -> bool {
    announced.range(seq + 1..).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seat::id;

    #[test]
    fn players_sort_in_seat_order_with_player_2_before_player_10() {
        let mut players = ["Player 10", "ann", "Player 2", "Player 1"].map(id);
        players.sort_by(seat_order);
        assert_eq!(players, ["ann", "Player 1", "Player 2", "Player 10"]);
    }

    #[test]
    fn a_player_is_named_with_its_role_when_the_account_knows_it() {
        let roles = HashMap::from([(id("ann"), "seer")]);
        let named = |player: &str| Seat::named(&id(player), &roles).to_string();
        assert_eq!(named("ann"), "ann (seer)");
        assert_eq!(named("bob"), "bob");
    }

    #[test]
    fn a_message_comes_late_once_a_later_phase_is_announced() {
        let announced = BTreeMap::from([(1, ()), (2, ())]);
        assert!(late(&announced, 1));
        assert!(!late(&announced, 2));
        assert!(!late(&announced, 3));
    }
}
