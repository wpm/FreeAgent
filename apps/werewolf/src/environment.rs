use crate::{Phase, PlayerId, Role, Team};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    round: NonZero<u8>,
    phase: Phase,
    roles: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

impl Environment {
    fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let alive = roles.keys().cloned().collect();
        Self {
            round: NonZero::<u8>::MIN,
            phase: Phase::Night,
            roles,
            alive,
        }
    }

    /// What one player is allowed to see: the environment with the roles
    /// narrowed to the ones that player knows. Everyone knows their own
    /// role, and werewolves also know each other.
    fn observation_for(&self, observer: &PlayerId) -> Self {
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

    pub fn winner(&self) -> Option<Team> {
        let werewolves = self.surviving(Team::Werewolves);
        let villagers = self.surviving(Team::Villagers);
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }

    fn surviving(&self, team: Team) -> usize {
        self.alive
            .iter()
            .filter(|player_id| {
                let role = self.roles[*player_id];
                match team {
                    Team::Werewolves => role == Role::Werewolf,
                    Team::Villagers => role != Role::Werewolf,
                }
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn village() -> Environment {
        Environment::new(HashMap::from([
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
    fn observation_keeps_the_rest_of_the_environment() {
        let environment = village();
        let observation = environment.observation_for(&"villager".to_string());
        assert_eq!(observation.round, environment.round);
        assert_eq!(observation.alive, environment.alive);
    }
}
