//! Werewolf as an episode in the [`free_agent`] framework, in the shape
//! of reinforcement learning.
//!
//! An environment has the whole `State` and runs the game on it. Each
//! player sees the game through the observations it is sent, each of
//! what that player is allowed to see, and answers with an action through
//! a `Policy`. Which policy a player has depends on its [`Role`]; the
//! `uniform_random` policies choose at random, as in Braverman, Etesami,
//! and Mossel's study of the game.

#![warn(missing_docs)]
// The game is still being rebuilt on the framework, so much of it is not
// yet reached from anywhere.
#![allow(dead_code)]

mod uniform_random;

use async_trait::async_trait;
use free_agent::ActorId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

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

/// The part of the game a player may be shown. The environment's own copy
/// has every role in it; the one sent to a player has the roles that
/// player knows.
#[derive(Debug, Clone)]
struct ObservableState {
    /// Rounds count from one. A round is a night and then a day.
    round: NonZero<u8>,
    /// Which half of the round it is.
    phase: Phase,
    /// The roles known to whoever holds this state.
    roles: HashMap<PlayerId, Role>,
    /// Everyone still in the game.
    alive: HashSet<PlayerId>,
}

/// The whole game, as the environment holds it.
struct State {
    /// Everything about the game that some player might be shown.
    observable: ObservableState,
    /// The players whose team the seer has learned.
    seer_discovered: HashSet<PlayerId>,
}

impl State {
    /// A game about to begin its first night, with everyone in `roles`
    /// alive and the seer's discoveries still to come.
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

    /// What `player_id` is allowed to see: the round, the phase, who is
    /// alive, and the roles it knows. A werewolf knows every werewolf, the
    /// seer knows itself and whoever it has discovered, and everyone else
    /// knows only itself.
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

    /// How many of `team` are alive.
    fn surviving(&self, team: Team) -> usize {
        self.observable
            .alive
            .iter()
            .filter(|player_id| self.observable.roles[*player_id].team() == team)
            .count()
    }

    /// Who has won, if anyone: the villagers when the last werewolf is
    /// dead, the werewolves when they are at least as many as the
    /// villagers.
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

/// How a player maps what it sees to what it does. The signature is the
/// reinforcement-learning one: an observation in, an action out, or none
/// for a phase the player sits out.
///
/// Policies run on a multi-threaded runtime, so they have to be shareable
/// across threads.
#[async_trait]
trait Policy: Send + Sync {
    /// Decide what to do about `observation`.
    async fn policy(&self, observation: Observation) -> Option<Action>;
}

/// What a player is sent: the game as it is allowed to see it.
type Observation = ObservableState;

/// What a player does: select another player. What selecting someone
/// means depends on the role and the phase. A werewolf by night selects
/// a victim, the doctor someone to save, the seer someone to learn
/// about, and anyone by day someone to vote out.
struct Action {
    /// The player selected.
    selection: ActorId,
}

/// The actor that runs the game. It holds the [`State`], shows each
/// player its observation, asks the players for their actions, and
/// applies the rules to what comes back.
#[async_trait]
trait Environment {
    /// The whole game.
    fn state(&self) -> State;
    /// Show `observation` to `players` and carry on.
    fn send(&self, observation: Observation, players: HashSet<PlayerId>) -> anyhow::Result<()>;
    /// Ask the players for their actions and collect what they answer.
    async fn request() -> anyhow::Result<HashMap<ActorId, Vec<Action>>>;

    /// The players whose team the seer has learned.
    fn seer_knows() -> HashSet<PlayerId>;
    /// Play one night: show everyone what they may see, then ask the
    /// doctor whom to save, the werewolves whom to kill, and the seer whom
    /// to learn about.
    async fn night(&mut self) -> anyhow::Result<()> {
        // Send out observations.
        // Request doctor selection.
        // Request werewolves selection.
        // Request seer selection.
        todo!()
    }
    /// Play one day: show everyone what they may see, then collect the
    /// village's votes.
    async fn day(&mut self) -> anyhow::Result<()> {
        // Send out observations.
        // Accumulate villager decisions.
        todo!()
    }
}
/// Set up a game of `players`, each with a role and the policy that plays
/// it, run by `environment`.
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
