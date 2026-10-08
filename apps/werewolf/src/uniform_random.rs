//! Players who choose uniformly at random among the other candidates. This is the game of Braverman, Etesami, and Mossel, *Mafia:
//! A theoretical study of players and coalitions in a partial information
//! environment*, whose result is that the game is balanced when the
//! werewolves number about the square root of the players.

use crate::{Action, Observation, Policy};
use async_trait::async_trait;

/// Kills a random villager by night and votes at random by day.
struct Werewolf {}
/// Votes at random by day and sleeps through the night.
struct Villager {}

/// Saves a random player by night and votes at random by day.
struct Doctor {}
/// Learns about a random player by night and votes at random by day.
struct Seer {}

#[async_trait]
impl Policy for Werewolf {
    async fn policy(&self, _observation: Observation) -> Option<Action> {
        todo!()
    }
}
#[async_trait]
impl Policy for Villager {
    async fn policy(&self, _observation: Observation) -> Option<Action> {
        todo!()
    }
}
#[async_trait]
impl Policy for Doctor {
    async fn policy(&self, _observation: Observation) -> Option<Action> {
        todo!()
    }
}
#[async_trait]
impl Policy for Seer {
    async fn policy(&self, _observation: Observation) -> Option<Action> {
        todo!()
    }
}
