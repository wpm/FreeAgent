#![allow(dead_code)]

//! Werewolf as an episode in the [`free_agent`] framework, in the shape
//! of reinforcement learning.
//!
//! An [environment] actor has the whole [`State`](state::State) and runs
//! the game on it. Each [agent] actor has no state of the game at all: it
//! is sent [observations](state::State::observation_for), which are what
//! it is allowed to see, and answers with actions. Both have a policy for
//! the day and one for the night, and which kind of agent a player is,
//! werewolf, villager, doctor, or seer, fixes which it has. The
//! [variant] is what those policies do.

use free_agent::ActorId;
use serde::{Deserialize, Serialize};

pub mod agent;
pub mod config;
pub mod environment;
pub mod state;
pub mod variant;

pub type PlayerId = ActorId;

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
    /// Which side this role wins with.
    pub fn team(self) -> Team {
        match self {
            Role::Werewolf => Team::Werewolves,
            Role::Villager | Role::Doctor | Role::Seer => Team::Villagers,
        }
    }
}

/// The two halves of a round.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum Phase {
    /// The werewolves choose a victim.
    Night,
    /// The village votes someone out.
    Day,
}

/// Everything that crosses the wire between the environment and the
/// agents. The environment sends observations; the agents send actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Environment to an agent: what it can see of the game, phase
    /// included. The agent acts on it.
    Observation(state::State),
    /// An agent's action: the player it selects. What selecting someone
    /// does is the environment's business.
    Select(PlayerId),
    /// Environment to the living: the game is over and this side won.
    /// Pending the reward going to the log instead, this is how the
    /// living agents learn to shut down.
    Over(Team),
}
