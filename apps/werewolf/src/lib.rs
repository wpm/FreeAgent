#![allow(dead_code)]

//! Werewolf as an episode in the [`free_agent`] framework, in the shape
//! of reinforcement learning.
//!
//! An environment has the whole `State` and runs the game on it. Each
//! player has no state of the game at all: it is sent observations, which
//! are what it is allowed to see, and answers with actions through a
//! `Policy`.

mod uniform_random;

use async_trait::async_trait;
use free_agent::ActorId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

type PlayerId = ActorId;

/// The two sides. A game ends when one of them has won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// Kill by night and win on reaching parity.
    Werewolves,
    /// Vote by day and win when the last werewolf is gone.
    Villagers,
}

/// What a player is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum Role {
    /// Kills by night, and knows the other werewolves.
    Werewolf,
    /// Votes by day and nothing more.
    Villager,
    /// Saves one player from the night's kill.
    Doctor,
    /// Learns one player's team each night.
    Seer,
}

impl Role {
    pub fn team(self) -> Team {
        match self {
            Role::Werewolf => Team::Werewolves,
            Role::Villager | Role::Doctor | Role::Seer => Team::Villagers,
        }
    }
}

#[derive(Debug, Clone)]
enum Phase {
    Night,
    Day,
}

#[derive(Debug, Clone)]
struct ObservableState {
    round: NonZero<u8>,
    phase: Phase,
    roles: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

struct State {
    observable: ObservableState,
    seer_discovered: HashSet<PlayerId>,
}

impl State {
    fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let alive: HashSet<PlayerId> = roles.keys().cloned().collect();
        let phase_round_players = ObservableState {
            round: NonZero::new(1).unwrap(),
            phase: Phase::Night,
            roles,
            alive,
        };
        Self {
            observable: phase_round_players,
            seer_discovered: HashSet::new(),
        }
    }

    fn observation(&self, player_id: PlayerId) -> anyhow::Result<Observation> {
        let role = self.observable.roles[&player_id];
        let roles = match role {
            Role::Werewolf => {
                // Werewolves know who all the other werewolves are.
                self.observable
                    .roles
                    .iter()
                    .filter(|(_, role)| **role == Role::Werewolf)
                    .map(|(id, role)| (id.clone(), *role))
                    .collect()
            }
            Role::Seer => {
                // Seers know their own role and that of anyone they have discovered.
                let mut roles = HashMap::from([(player_id, role)]);
                roles.extend(
                    self.seer_discovered
                        .iter()
                        .map(|id| (id.clone(), self.observable.roles[id])),
                );
                roles
            }
            _ => {
                // Everyone else just knows their own role.
                HashMap::from([(player_id, role)])
            }
        };
        Ok(ObservableState {
            round: self.observable.round,
            phase: self.observable.phase.clone(),
            roles,
            alive: self.observable.alive.clone(),
        })
    }

    fn surviving(&self, team: Team) -> usize {
        self.observable
            .alive
            .iter()
            .filter(|player_id| self.observable.roles[*player_id].team() == team)
            .count()
    }

    fn winner(&self) -> Option<Team> {
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
}

#[async_trait]
trait Policy: Send + Sync {
    async fn policy(&self, observation: Observation) -> Option<Action>;
}

type Observation = ObservableState;

struct Action {
    selection: ActorId,
}

#[async_trait]
trait Environment {
    fn state(&self) -> State;
    fn send(&self, observation: Observation, players: HashSet<PlayerId>) -> anyhow::Result<()>;
    async fn request() -> anyhow::Result<HashMap<ActorId, Vec<Action>>>;

    fn seer_knows() -> HashSet<PlayerId>;
    async fn night(&mut self) -> anyhow::Result<()> {
        // Send out observations.
        // Request doctor selection.
        // Request werewolves selection.
        // Request seer selection.
        todo!()
    }
    async fn day(&mut self) -> anyhow::Result<()> {
        // Send out observations.
        // Accumulate villager decisions.
        todo!()
    }
}
fn create<E: Environment>(_environment: E, _players: HashMap<PlayerId, (Role, Box<dyn Policy>)>) {}

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
        let observation = village().observation(observer.to_string()).unwrap();
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
    fn the_seer_sees_only_itself_until_it_has_discovered_someone() {
        assert_eq!(seen_by("seer"), ["seer"]);

        let mut state = village();
        state.seer_discovered.insert("wolf1".to_string());
        let observation = state.observation("seer".to_string()).unwrap();
        let mut seen: Vec<_> = observation.roles.keys().cloned().collect();
        seen.sort();
        assert_eq!(seen, ["seer", "wolf1"]);
        assert_eq!(observation.roles["wolf1"], Role::Werewolf);
    }

    #[test]
    fn observation_keeps_the_rest_of_the_state() {
        let state = village();
        let observation = state.observation("villager".to_string()).unwrap();
        assert_eq!(observation.round, state.observable.round);
        assert_eq!(observation.alive, state.observable.alive);
    }

    #[test]
    fn werewolves_win_at_parity_and_villagers_when_the_wolves_are_gone() {
        let mut state = State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]));
        assert_eq!(state.winner(), None);
        state.observable.alive.remove("ann");
        assert_eq!(state.winner(), Some(Team::Werewolves));

        let mut state = village();
        state.observable.alive.remove("wolf1");
        state.observable.alive.remove("wolf2");
        assert_eq!(state.winner(), Some(Team::Villagers));
    }
}
