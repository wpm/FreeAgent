//! Werewolf, the social deception game, played by free agents.
//!
//! A village holds some werewolves who know each other and some villagers who
//! do not. Play alternates between a night, when the werewolves agree on
//! someone to kill, and a day, when everyone votes on someone to lynch. The
//! werewolves win when they are no fewer than the villagers; the villagers
//! win when the last werewolf is gone.
//!
//! Every player is an [`Actor`](free_agent::actor::Actor) with its own state
//! and its own task, so a player knows only what it has been told. An
//! environment actor runs the game: it asks the players what they want to do,
//! counts the answers, and tells everyone what happened.

#![warn(missing_docs)]

pub mod environment;
#[cfg(test)]
mod fixtures;
pub mod game;
pub mod phase;
pub mod player;
pub mod protocol;
pub mod role;

pub use environment::{Environment, Event, standard_roles};
pub use game::{Outcome, Phase, Player, Village, plurality};
pub use player::{Agent, Policy, Random, State};
pub use protocol::{Action, ActionSpace, Observation};
pub use role::{NightAction, Role, Team};
