mod uniform_random;

use crate::rl::actor::ActorId;
use async_trait::async_trait;
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
        let mut observation = self.observable.clone();
        match role {
            Role::Werewolf => {
                // Werewolves know who all the other werewolves are.
                let werewolves = observation
                    .roles
                    .into_iter()
                    .filter(|(_, role)| *role == Role::Werewolf)
                    .map(|(id, role)| (id.clone(), role))
                    .collect();
                observation.roles = werewolves;
            }
            Role::Seer => {
                // Seers know their own role and that of anyone they have discovered.
                let oneself = HashMap::from([(player_id, role)]);
                observation.roles = oneself;
                let seer_discovered: HashMap<PlayerId, Role> = self
                    .seer_discovered
                    .clone()
                    .into_iter()
                    .map(|id| (id.clone(), self.observable.roles[&id]))
                    .collect();
                observation.roles.extend(seer_discovered);
            }
            _ => {
                // Everyone else just knows their own role.
                let oneself = HashMap::from([(player_id, role)]);
                observation.roles = oneself;
            }
        };
        Ok(observation)
    }

    fn surviving(&self, team: Team) -> usize {
        self.observable
            .alive
            .iter()
            .cloned()
            .filter(|player_id| self.observable.roles[player_id].team() == team)
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

trait Environment {
    fn state() -> State;

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
fn create<E: Environment>(environment: E, players: HashMap<PlayerId, (Role, Box<dyn Policy>)>) {}
