//! Players who choose at random.

use crate::{Entry, Message, Observation, PlayerId};
use async_trait::async_trait;
use free_agent::{Behavior, Context};
use rand::seq::IndexedRandom;

/// A player who, asked to choose, picks at random among the living whose
/// role it does not know. That keeps a werewolf from choosing a werewolf and
/// the seer from asking about anyone twice.
pub struct Player {
    context: Context<Message, Entry>,
}

impl Player {
    /// A player holding `context`, built once the episode has made it.
    pub(crate) fn new(context: Context<Message, Entry>) -> Self {
        Player { context }
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Phase, Role};
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
