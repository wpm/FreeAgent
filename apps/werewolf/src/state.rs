//! What the referee knows, and what each player is allowed to see of it.

use crate::{Phase, PlayerId, Role, Team};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

/// The whole game as the environment knows it. A player's observation is
/// a `State` too, with the roles narrowed to what that player knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    round: NonZero<u8>,
    phase: Phase,
    roles: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

impl State {
    /// A game about to begin: the first night, with everyone alive.
    pub fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let alive = roles.keys().cloned().collect();
        Self {
            round: NonZero::<u8>::MIN,
            phase: Phase::Night,
            roles,
            alive,
        }
    }

    /// Rounds are counted from one.
    pub fn round(&self) -> NonZero<u8> {
        self.round
    }

    /// Which half of the round is being played.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The roles this state knows: all of them for the environment, only
    /// the observer's own and perhaps its pack's for a player.
    pub fn roles(&self) -> &HashMap<PlayerId, Role> {
        &self.roles
    }

    /// Who is still in the game.
    pub fn alive(&self) -> &HashSet<PlayerId> {
        &self.alive
    }

    /// Everyone still in the game, in a fixed order.
    pub fn living(&self) -> Vec<PlayerId> {
        let mut living: Vec<_> = self.alive.iter().cloned().collect();
        living.sort();
        living
    }

    /// Everyone still in the game on one side, in a fixed order. Only
    /// meaningful for a state that knows the roles in question.
    pub fn living_on(&self, team: Team) -> Vec<PlayerId> {
        self.living()
            .into_iter()
            .filter(|id| self.roles.get(id).is_some_and(|role| role.team() == team))
            .collect()
    }

    /// Take a player out of the game.
    pub fn kill(&mut self, id: &PlayerId) {
        self.alive.remove(id);
    }

    /// Move on to the next phase: day follows night in the same round,
    /// and night follows day in the next.
    pub fn advance(&mut self) {
        self.phase = match self.phase {
            Phase::Night => Phase::Day,
            Phase::Day => {
                self.round = self.round.saturating_add(1);
                Phase::Night
            }
        };
    }

    /// What one player is allowed to see: the state with the roles
    /// narrowed to the ones that player knows. Everyone knows their own
    /// role, and werewolves also know each other.
    pub fn observation_for(&self, observer: &PlayerId) -> Self {
        let observer_role = self.roles[observer];
        let roles = self
            .roles
            .iter()
            .filter(|(id, role)| {
                *id == observer || (observer_role == Role::Werewolf && **role == Role::Werewolf)
            })
            .map(|(id, role)| (id.clone(), *role))
            .collect();
        Self {
            roles,
            ..self.clone()
        }
    }

    /// Who has won, if anyone. The werewolves win on reaching parity,
    /// since from there no vote can go against them; the villagers win
    /// when the last werewolf is gone.
    pub fn winner(&self) -> Option<Team> {
        let werewolves = self.living_on(Team::Werewolves).len();
        let villagers = self.living_on(Team::Villagers).len();
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn village() -> State {
        State::new(HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
            ("seer".to_string(), Role::Seer),
            ("villager".to_string(), Role::Villager),
        ]))
    }

    fn seen_by(observer: &str) -> Vec<String> {
        let observation = village().observation_for(&observer.to_string());
        let mut seen: Vec<_> = observation.roles.keys().cloned().collect();
        seen.sort();
        seen
    }

    #[test]
    fn werewolves_see_each_other_and_nobody_else() {
        assert_eq!(seen_by("wolf1"), ["wolf1", "wolf2"]);
    }

    #[test]
    fn villagers_see_only_themselves() {
        assert_eq!(seen_by("villager"), ["villager"]);
    }

    #[test]
    fn the_seer_sees_only_itself() {
        assert_eq!(seen_by("seer"), ["seer"]);
    }

    #[test]
    fn observation_keeps_the_rest_of_the_state() {
        let state = village();
        let observation = state.observation_for(&"villager".to_string());
        assert_eq!(observation.round, state.round);
        assert_eq!(observation.alive, state.alive);
    }

    #[test]
    fn a_round_is_a_night_and_then_a_day() {
        let mut state = village();
        assert_eq!((state.round().get(), state.phase()), (1, Phase::Night));
        state.advance();
        assert_eq!((state.round().get(), state.phase()), (1, Phase::Day));
        state.advance();
        assert_eq!((state.round().get(), state.phase()), (2, Phase::Night));
    }

    #[test]
    fn the_dead_are_no_longer_living() {
        let mut state = village();
        state.kill(&"seer".to_string());
        assert_eq!(state.living(), ["villager", "wolf1", "wolf2"]);
        assert_eq!(state.living_on(Team::Villagers), ["villager"]);
    }

    #[test]
    fn werewolves_win_at_parity_and_villagers_when_the_wolves_are_gone() {
        let mut state = State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]));
        assert_eq!(state.winner(), None);
        state.kill(&"ann".to_string());
        assert_eq!(state.winner(), Some(Team::Werewolves));

        let mut state = village();
        state.kill(&"wolf1".to_string());
        state.kill(&"wolf2".to_string());
        assert_eq!(state.winner(), Some(Team::Villagers));
    }
}
