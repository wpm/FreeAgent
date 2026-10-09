//! Players who choose at random, and the environment that runs their game.

use crate::{Observation, PlayerId, Role, State};
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context};
use rand::seq::IndexedRandom;
use std::collections::HashMap;

/// The actor that holds the game and tells each player what it may see.
pub(crate) struct Environment {
    context: Context<Message>,
    state: State,
}

impl Environment {
    /// Builds the environment for a game of `roles` once the episode has
    /// made its context.
    pub(crate) fn builder(roles: HashMap<PlayerId, Role>) -> Builder<Self> {
        Box::new(move |context| Environment {
            context,
            state: State::new(roles),
        })
    }
}

#[async_trait]
impl Behavior for Environment {
    type Message = Message;
    type Log = Message;

    fn context(&self) -> &Context<Message> {
        &self.context
    }
}

/// A player who, asked to choose, picks at random among the living whose
/// role it does not know. That keeps a werewolf from choosing a werewolf and
/// the seer from asking about anyone twice.
pub(crate) struct Player {
    context: Context<Message>,
}

impl Player {
    /// Builds a player once the episode has made its context.
    pub(crate) fn builder() -> Builder<Self> {
        Box::new(|context| Player { context })
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Message;

    fn context(&self) -> &Context<Message> {
        &self.context
    }

    /// Asked with an observation, choose from it. Asked anything else, say
    /// nothing.
    async fn answer(&mut self, message: &Message) -> anyhow::Result<Vec<Message>> {
        let Message::Observation(observation) = message else {
            return Ok(vec![]);
        };
        Ok(choose(&self.context.id, observation)
            .into_iter()
            .map(Message::Action)
            .collect())
    }
}

/// A living player chosen uniformly at random by `me` from `observation`,
/// among those whose role `me` does not know. None when there is nobody
/// to choose.
fn choose(me: &PlayerId, observation: &Observation) -> Option<PlayerId> {
    let mut candidates: Vec<&PlayerId> = observation
        .alive
        .iter()
        .filter(|player| *player != me && !observation.roles.contains_key(*player))
        .collect();
    // Sorted so the choice depends on the random number alone, not on
    // hash order.
    candidates.sort();
    candidates
        .choose(&mut rand::rng())
        .map(|player| (*player).clone())
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    /// What a player may see, from the environment.
    Observation(Observation),
    /// A player's choice of another player, to the environment.
    Action(PlayerId),
}
impl free_agent::Message for Message {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Phase;
    use std::collections::HashSet;
    use std::num::NonZero;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// What `me` sees of a village where everyone in `alive` lives and
    /// `me` knows the roles in `known`.
    fn seen(alive: &[&str], known: &[(&str, Role)]) -> Observation {
        Observation {
            round: NonZero::new(1).unwrap(),
            phase: Phase::Night,
            roles: known.iter().map(|(name, role)| (id(name), *role)).collect(),
            alive: alive.iter().map(|name| id(name)).collect(),
        }
    }

    #[test]
    fn a_player_chooses_someone_alive_whose_role_it_does_not_know() {
        let observation = seen(
            &["wolf1", "wolf2", "ann", "bob"],
            &[("wolf1", Role::Werewolf), ("wolf2", Role::Werewolf)],
        );
        let allowed = HashSet::from([id("ann"), id("bob")]);

        for _ in 0..50 {
            let chosen = choose(&id("wolf1"), &observation).unwrap();
            assert!(allowed.contains(&chosen), "{chosen}");
        }
    }

    #[test]
    fn a_player_never_chooses_itself() {
        let observation = seen(&["ann", "bob"], &[("ann", Role::Villager)]);

        for _ in 0..50 {
            assert_eq!(choose(&id("ann"), &observation), Some(id("bob")));
        }
    }

    #[test]
    fn a_player_with_no_one_to_choose_chooses_no_one() {
        let observation = seen(
            &["seer", "wolf"],
            &[("seer", Role::Seer), ("wolf", Role::Werewolf)],
        );

        assert_eq!(choose(&id("seer"), &observation), None);
    }

    #[test]
    fn a_player_does_not_choose_the_dead() {
        let observation = seen(&["ann"], &[("ann", Role::Villager)]);

        assert_eq!(choose(&id("ann"), &observation), None);
    }
}
