//! Werewolf as an episode of [free agents](free_agent): the rules in one
//! actor, the environment, and a player in each of the others.
//!
//! The environment alone holds the state of the game: the round and whether
//! it is night or day, every player's [`Role`], who is alive, and what the
//! seer has learned. A player sees the game only through the `Observation`
//! the environment sends it, which carries the round, the phase, who is
//! alive, and the roles that player may know. A werewolf knows every
//! werewolf, the seer knows itself and whoever it has discovered, and
//! everyone else knows only itself. Holding no state, a player can only
//! talk.
//!
//! Play alternates between night and day, starting with night, until one
//! [`Team`] has won.

#![warn(missing_docs)]
// The game is still being rebuilt on the framework, so much of it is not
// yet reached from anywhere.
#![allow(dead_code)]

mod uniform_random;

use free_agent::ActorId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

// use async_trait::async_trait;
// #[async_trait]

/// A player is an actor, named as the episode names it.
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
    /// Votes by day.
    Villager,
    /// Saves one player from the night's kill.
    Doctor,
    /// Learns one player's team each night.
    Seer,
}

impl Role {
    /// Which side this role wins with.
    pub fn team(self) -> Team {
        match self {
            Role::Werewolf => Team::Werewolves,
            Role::Villager | Role::Doctor | Role::Seer => Team::Villagers,
        }
    }
}

/// The two halves of a round. A game begins with night.
#[derive(Debug, Clone)]
enum Phase {
    /// The werewolves choose a victim, the doctor someone to save, and the
    /// seer someone to learn about.
    Night,
    /// The village votes someone out.
    Day,
}

/// The state of a game, held by the environment alone.
struct State {
    /// Rounds count from one. A round is a night and then a day.
    round: NonZero<u8>,
    /// Which half of the round it is.
    phase: Phase,
    /// The roles known to whoever holds this state.
    roles: HashMap<PlayerId, Role>,
    /// Everyone still in the game.
    alive: HashSet<PlayerId>,
    /// The players whose team a seer has learned.
    seers_discovered: HashMap<PlayerId, HashSet<PlayerId>>,
}

impl State {
    /// A game of `roles`, at the first night with everyone alive.
    fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let alive: HashSet<PlayerId> = roles.keys().cloned().collect();
        let seers_discovered = roles
            .iter()
            .filter(|(_, role)| **role == Role::Seer)
            .map(|(player_id, _)| (player_id.clone(), HashSet::from([player_id.clone()])))
            .collect();
        Self {
            round: NonZero::new(1).unwrap(),
            phase: Phase::Night,
            roles,
            alive,
            seers_discovered,
        }
    }

    /// What `player_id` is allowed to see: the round, the phase, who is
    /// alive, and the roles it knows. A werewolf knows every werewolf, the
    /// seer knows itself and whoever it has discovered, and everyone else
    /// knows only itself.
    fn observation(&self, player_id: PlayerId) -> anyhow::Result<Observation> {
        let role = self.roles[&player_id];
        let roles = match role {
            Role::Werewolf => {
                // Werewolves know who all the other werewolves are.
                self.roles
                    .iter()
                    .filter(|(_, role)| **role == Role::Werewolf)
                    .map(|(id, role)| (id.clone(), *role))
                    .collect()
            }
            Role::Seer => {
                // Seers know their own role and that of anyone they have discovered.
                let mut roles = HashMap::from([(player_id.clone(), role)]);
                roles.extend(
                    self.seers_discovered[&player_id]
                        .iter()
                        .map(|id| (id.clone(), self.roles[id])),
                );
                roles
            }
            _ => {
                // Everyone else just knows their own role.
                HashMap::from([(player_id, role)])
            }
        };
        Ok(Observation {
            round: self.round,
            phase: self.phase.clone(),
            roles,
            alive: self.alive.clone(),
        })
    }

    /// Which players of `role` are alive?
    /// The players who act in the current phase: by night the werewolves,
    /// the doctor, and the seer; by day everyone alive.
    fn awake(&self) -> HashSet<PlayerId> {
        let acts = |role: Role| match self.phase {
            Phase::Night => matches!(role, Role::Werewolf | Role::Doctor | Role::Seer),
            Phase::Day => true,
        };
        self.alive
            .iter()
            .filter(|player_id| acts(self.roles[*player_id]))
            .cloned()
            .collect()
    }

    fn surviving(&self, role: Role) -> HashSet<PlayerId> {
        self.alive
            .iter()
            .filter(|player_id| self.roles[*player_id] == role)
            .cloned()
            .collect()
    }

    /// How many of `team` are alive?
    fn num_surviving_on(&self, team: Team) -> usize {
        self.alive
            .iter()
            .filter(|player_id| self.roles[*player_id].team() == team)
            .count()
    }

    /// Who has won, if anyone: the villagers when the last werewolf is
    /// dead, the werewolves when they are at least as many as the
    /// villagers.
    fn winner(&self) -> Option<Team> {
        let werewolves = self.num_surviving_on(Team::Werewolves);
        let villagers = self.num_surviving_on(Team::Villagers);
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
struct Observation {
    /// Rounds count from one. A round is a night and then a day.
    round: NonZero<u8>,
    /// Which half of the round it is.
    phase: Phase,
    /// The roles known to whoever holds this state.
    roles: HashMap<PlayerId, Role>,
    /// Everyone still in the game.
    alive: HashSet<PlayerId>,
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
        state
            .seers_discovered
            .get_mut("seer")
            .unwrap()
            .insert("wolf1".to_string());
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
        assert_eq!(observation.round, state.round);
        assert_eq!(observation.alive, state.alive);
    }

    fn sorted(players: HashSet<PlayerId>) -> Vec<PlayerId> {
        let mut players: Vec<_> = players.into_iter().collect();
        players.sort();
        players
    }

    #[test]
    fn by_night_the_werewolves_doctor_and_seer_are_awake() {
        let mut state = State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("doctor".to_string(), Role::Doctor),
            ("seer".to_string(), Role::Seer),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]));
        state.alive.remove("bob");
        assert_eq!(sorted(state.awake()), ["doctor", "seer", "wolf"]);
    }

    #[test]
    fn by_day_everyone_alive_is_awake() {
        let mut state = village();
        state.phase = Phase::Day;
        state.alive.remove("wolf2");
        assert_eq!(sorted(state.awake()), ["seer", "villager", "wolf1"]);
    }

    #[test]
    fn werewolves_win_at_parity_and_villagers_when_the_wolves_are_gone() {
        let mut state = State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]));
        assert_eq!(state.winner(), None);
        state.alive.remove("ann");
        assert_eq!(state.winner(), Some(Team::Werewolves));

        let mut state = village();
        state.alive.remove("wolf1");
        state.alive.remove("wolf2");
        assert_eq!(state.winner(), Some(Team::Villagers));
    }
}
