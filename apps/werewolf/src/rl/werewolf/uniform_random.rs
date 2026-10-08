use crate::rl::werewolf::{Action, Observation, Policy};
use async_trait::async_trait;

struct Werewolf {}
struct Villager {}

struct Doctor {}
struct Seer {}

#[async_trait]
impl Policy for Werewolf {
    async fn policy(&self, observation: Observation) -> Option<Action> {
        todo!()
    }
}
#[async_trait]
impl Policy for Villager {
    async fn policy(&self, observation: Observation) -> Option<Action> {
        todo!()
    }
}
#[async_trait]
impl Policy for Doctor {
    async fn policy(&self, observation: Observation) -> Option<Action> {
        todo!()
    }
}
#[async_trait]
impl Policy for Seer {
    async fn policy(&self, observation: Observation) -> Option<Action> {
        todo!()
    }
}
