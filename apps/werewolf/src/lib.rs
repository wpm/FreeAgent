#![allow(dead_code)]

//! Werewolf as an episode in the [`free_agent`] framework.
//!
//! An [environment] actor referees: it asks the werewolves for a kill by
//! night and the village for a vote by day, and it alone knows the whole
//! [`State`](state::State). Each [player] actor answers what it is asked,
//! seeing only its own [observation](state::State::observation_for). What
//! each of them does is the game's [variant]: the environment's side is a
//! [`Moderate`](variant::Moderate) and a player's side is a
//! [`Play`](variant::Play), and either, boxed, is an actor.

use free_agent::ActorId;
use serde::{Deserialize, Serialize};

pub mod config;
pub mod environment;
pub mod player;
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
/// players. The environment asks; the players answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Environment to a player: it is night, here is what you can see.
    /// A werewolf answers with a [`Select`](Message::Select).
    Night(state::State),
    /// Environment to a player: it is day, here is what you can see.
    /// Everyone answers with a [`Select`](Message::Select).
    Day(state::State),
    /// A player's answer to a prompt: the player it selects. What
    /// selecting someone does is the asker's business.
    Select(PlayerId),
    /// Environment to everyone: the game is over and this side won.
    Over(Team),
}
