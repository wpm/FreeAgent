//! Villages the tests share, so one roster is described once.

use crate::game::{Player, Village};
use crate::role::Role;

/// The standard table these tests reason about: two werewolves against a
/// doctor, a seer, and two plain villagers.
///
/// Six is the smallest roster where every role is represented and neither
/// side has already won, so most rules can be shown on it.
pub fn village() -> Village {
    Village::new([
        Player::new("Wolf1", Role::Werewolf),
        Player::new("Wolf2", Role::Werewolf),
        Player::new("Doc", Role::Doctor),
        Player::new("Seer", Role::Seer),
        Player::new("Villager1", Role::Villager),
        Player::new("Villager2", Role::Villager),
    ])
}
