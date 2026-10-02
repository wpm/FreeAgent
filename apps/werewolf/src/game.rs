//! Who is playing, who is alive, and who has won.
//!
//! This is the environment's own bookkeeping. A player never sees a
//! [`Village`] — a player knows only what it has been told, which is what
//! makes the deception possible.

use crate::role::{Role, Team};
use free_agent::actor::ActorId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A player, as the environment knows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// The actor playing them.
    pub id: ActorId,
    /// What they are.
    pub role: Role,
    /// Whether they are still in the game.
    pub alive: bool,
}

impl Player {
    /// A living player.
    pub fn new(id: impl Into<ActorId>, role: Role) -> Self {
        Self {
            id: id.into(),
            role,
            alive: true,
        }
    }
}

/// Which half of the cycle the village is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    /// The night roles act.
    Night,
    /// Everyone votes on someone to lynch.
    Day,
}

/// Who has won, once somebody has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// Every werewolf is dead.
    Villagers,
    /// The werewolves are no fewer than everyone else.
    Werewolves,
}

/// The state of play: who is alive and whether anyone has won yet.
#[derive(Debug, Clone)]
pub struct Village {
    players: Vec<Player>,
}

impl Village {
    /// A village where everyone is alive.
    ///
    /// # Panics
    ///
    /// Panics if two players share an id, since an id is how the environment
    /// addresses them.
    pub fn new(players: impl IntoIterator<Item = Player>) -> Self {
        let players: Vec<Player> = players.into_iter().collect();
        let unique: HashSet<&ActorId> = players.iter().map(|p| &p.id).collect();
        assert_eq!(unique.len(), players.len(), "two players share an id");
        Self { players }
    }

    /// A village of named players with the roles `roles` gives, in order.
    ///
    /// The names are `Player0`, `Player1`, and so on, which is what the
    /// command line deals in and what a log reader will recognize.
    ///
    /// # Panics
    ///
    /// Panics if `roles` is empty, since a game needs players.
    pub fn numbered(roles: impl IntoIterator<Item = Role>) -> Self {
        let players: Vec<Player> = roles
            .into_iter()
            .enumerate()
            .map(|(n, role)| Player::new(format!("Player{n}"), role))
            .collect();
        assert!(!players.is_empty(), "a village needs players");
        Self::new(players)
    }

    /// Everyone, alive or not, in the order they joined.
    pub fn everyone(&self) -> &[Player] {
        &self.players
    }

    /// Everyone still in the game.
    pub fn living(&self) -> Vec<Player> {
        self.players.iter().filter(|p| p.alive).cloned().collect()
    }

    /// The living players of one role.
    pub fn living_with(&self, role: Role) -> Vec<Player> {
        self.players
            .iter()
            .filter(|p| p.alive && p.role == role)
            .cloned()
            .collect()
    }

    /// The living players woken by `phase`.
    pub fn woken_by(&self, phase: Phase) -> Vec<Player> {
        self.players
            .iter()
            .filter(|p| p.alive && p.role.acts_in(phase))
            .cloned()
            .collect()
    }

    /// What `id` is, whether or not they are alive.
    pub fn role_of(&self, id: &ActorId) -> Option<Role> {
        self.players.iter().find(|p| &p.id == id).map(|p| p.role)
    }

    /// Whether `id` is still in the game.
    pub fn is_alive(&self, id: &ActorId) -> bool {
        self.players.iter().any(|p| &p.id == id && p.alive)
    }

    /// Removes a player from the game. Killing someone already dead, or
    /// someone who was never here, changes nothing.
    pub fn kill(&mut self, id: &ActorId) {
        if let Some(player) = self.players.iter_mut().find(|p| &p.id == id) {
            player.alive = false;
        }
    }

    /// Who has won, if anyone has yet.
    ///
    /// The werewolves win by reaching parity rather than by killing
    /// everyone: once they are no fewer than the rest, no vote can go
    /// against them.
    pub fn outcome(&self) -> Option<Outcome> {
        let living = self.living();
        let wolves = living
            .iter()
            .filter(|p| p.role.team() == Team::Werewolves)
            .count();
        let others = living.len() - wolves;
        if wolves == 0 {
            Some(Outcome::Villagers)
        } else if wolves >= others {
            Some(Outcome::Werewolves)
        } else {
            None
        }
    }
}

/// Everyone tied for the most votes, in a stable order.
///
/// Empty if nobody voted. One name means a clear plurality; several mean a
/// tie, which the two votes in this game settle differently — see
/// [`plurality`] and [`Night::resolve`](crate::phase::Night::resolve).
pub fn leaders<'a>(votes: impl IntoIterator<Item = &'a ActorId>) -> Vec<ActorId> {
    let mut counts: HashMap<&ActorId, usize> = HashMap::new();
    for vote in votes {
        *counts.entry(vote).or_default() += 1;
    }
    let Some(most) = counts.values().copied().max() else {
        return Vec::new();
    };
    let mut tied: Vec<ActorId> = counts
        .into_iter()
        .filter(|(_, n)| *n == most)
        .map(|(id, _)| id.clone())
        .collect();
    // A map iterates in no particular order, and a game must replay the
    // same way twice.
    tied.sort();
    tied
}

/// Whoever got the most votes, or `None` if nobody did or the lead was tied.
///
/// This is the day's rule: the village must agree to hang someone, so a tie
/// passes without a lynching. The wolves settle a tie differently, since a
/// night that killed nobody whenever they disagreed would let them stall the
/// game forever.
pub fn plurality<'a>(votes: impl IntoIterator<Item = &'a ActorId>) -> Option<ActorId> {
    match leaders(votes).as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::village;

    #[test]
    fn a_village_starts_with_everyone_alive() {
        let village = village();
        assert_eq!(village.living().len(), 6);
        assert_eq!(village.living_with(Role::Werewolf).len(), 2);
    }

    #[test]
    fn only_the_night_roles_are_woken_at_night() {
        let woken: Vec<String> = village()
            .woken_by(Phase::Night)
            .iter()
            .map(|p| p.id.clone())
            .collect();
        assert_eq!(woken, ["Wolf1", "Wolf2", "Doc", "Seer"]);
    }

    #[test]
    fn everyone_living_is_woken_by_day() {
        assert_eq!(village().woken_by(Phase::Day).len(), 6);
    }

    #[test]
    fn killing_removes_a_player_from_the_living() {
        let mut village = village();
        village.kill(&"Villager1".to_string());
        assert_eq!(village.living().len(), 5);
        assert!(!village.is_alive(&"Villager1".to_string()));
        // Still known, just not alive.
        assert_eq!(
            village.role_of(&"Villager1".to_string()),
            Some(Role::Villager)
        );
    }

    #[test]
    fn killing_the_same_player_twice_changes_nothing() {
        let mut village = village();
        village.kill(&"Villager1".to_string());
        village.kill(&"Villager1".to_string());
        assert_eq!(village.living().len(), 5);
    }

    #[test]
    fn nobody_has_won_at_the_start() {
        assert_eq!(village().outcome(), None);
    }

    #[test]
    fn the_villagers_win_when_the_last_werewolf_dies() {
        let mut village = village();
        village.kill(&"Wolf1".to_string());
        assert_eq!(village.outcome(), None);
        village.kill(&"Wolf2".to_string());
        assert_eq!(village.outcome(), Some(Outcome::Villagers));
    }

    /// Parity is enough: with two of each, every vote the villagers bring
    /// can be matched, so the game is already decided.
    #[test]
    fn the_werewolves_win_on_reaching_parity() {
        let mut village = village();
        village.kill(&"Doc".to_string());
        assert_eq!(village.outcome(), None);
        // Two wolves against two others is parity, and parity is a win.
        village.kill(&"Seer".to_string());
        assert_eq!(village.outcome(), Some(Outcome::Werewolves));
    }

    #[test]
    fn a_plurality_is_whoever_got_the_most_votes() {
        let votes: Vec<ActorId> = ["Wolf1", "Wolf1", "Villager1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(plurality(votes.iter()), Some("Wolf1".to_string()));
    }

    #[test]
    fn leaders_are_everyone_tied_for_the_most_votes() {
        let votes: Vec<ActorId> = ["B", "B", "A", "A", "C"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(leaders(votes.iter()), ["A", "B"]);
    }

    #[test]
    fn leaders_of_nothing_is_nobody() {
        assert!(leaders(Vec::<&ActorId>::new()).is_empty());
    }

    #[test]
    fn a_tied_vote_kills_nobody() {
        let votes: Vec<ActorId> = ["Wolf1", "Villager1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(plurality(votes.iter()), None);
    }

    #[test]
    fn no_votes_kill_nobody() {
        assert_eq!(plurality(Vec::<&ActorId>::new()), None);
    }

    #[test]
    fn a_numbered_village_names_its_players_in_order() {
        let village = Village::numbered([Role::Werewolf, Role::Seer, Role::Villager]);
        let named: Vec<(String, Role)> = village
            .everyone()
            .iter()
            .map(|p| (p.id.clone(), p.role))
            .collect();
        assert_eq!(
            named,
            [
                ("Player0".to_string(), Role::Werewolf),
                ("Player1".to_string(), Role::Seer),
                ("Player2".to_string(), Role::Villager),
            ]
        );
    }

    #[test]
    #[should_panic(expected = "a village needs players")]
    fn a_village_needs_players() {
        Village::numbered([]);
    }

    #[test]
    #[should_panic(expected = "two players share an id")]
    fn two_players_cannot_share_an_id() {
        Village::new([
            Player::new("Wolf1", Role::Werewolf),
            Player::new("Wolf1", Role::Villager),
        ]);
    }
}
