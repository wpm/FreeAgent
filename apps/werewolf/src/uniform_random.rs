//! Players who choose at random.

use crate::{Message, Observation, PlayerId};
use async_trait::async_trait;
use free_agent::{Behavior, Context};
use rand::seq::IndexedRandom;

/// A player who, asked to choose, picks at random among the living whose
/// role it does not know. That keeps a werewolf from choosing a werewolf and
/// the seer from asking about anyone twice.
pub struct Player {
    context: Context<Message>,
}

impl Player {
    /// A player holding `context`, built once the episode has made it.
    pub(crate) fn new(context: Context<Message>) -> Self {
        Player { context }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, NoTieBreak, Phase, RandomTieBreak, Role, Team};
    use free_agent::{ActorInit, Episode};
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::num::NonZero;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// Two werewolves, a seer, and a villager, each played at random, and
    /// the environment that runs them.
    fn village() -> HashMap<PlayerId, Role> {
        HashMap::from([
            (id("wolf1"), Role::Werewolf),
            (id("wolf2"), Role::Werewolf),
            (id("seer"), Role::Seer),
            (id("villager"), Role::Villager),
        ])
    }

    /// Run a game of `roles`, each played at random, to its end. Returns
    /// the winner and everything the environment logged.
    async fn play(roles: HashMap<PlayerId, Role>) -> (Team, Vec<Message>) {
        let players: HashSet<PlayerId> = roles.keys().cloned().collect();
        let (winner, won) = oneshot::channel();
        let mut init = HashMap::from([(
            id("environment"),
            ActorInit {
                behavior: Actor::environment(
                    roles,
                    Box::new(RandomTieBreak),
                    Box::new(NoTieBreak),
                    winner,
                ),
                can_send_to: players.clone(),
                can_shut_down: players.clone(),
                has_logger: true,
            },
        )]);
        for player in players {
            init.insert(
                player,
                ActorInit {
                    behavior: Actor::player(),
                    can_send_to: HashSet::new(),
                    can_shut_down: HashSet::new(),
                    has_logger: false,
                },
            );
        }
        let (logger, mut log) = unbounded_channel();

        Episode::new(init, logger)
            .run(Duration::from_secs(60))
            .await
            .unwrap();

        let mut logged = Vec::new();
        while let Some(event) = log.recv().await {
            logged.push(event.payload);
        }
        (won.await.unwrap(), logged)
    }

    #[tokio::test]
    async fn a_game_runs_until_a_team_has_won() {
        let roles = village();
        let (winner, logged) = play(roles.clone()).await;

        assert!(matches!(winner, Team::Werewolves | Team::Villagers));
        // By the first night three players choose, so there is at least
        // that much on the record, and every choice names a player.
        assert!(logged.len() >= 3, "{logged:?}");
        for choice in &logged {
            let Message::Action(chosen) = choice else {
                panic!("the environment logs choices, not {choice:?}");
            };
            assert!(roles.contains_key(chosen), "{chosen}");
        }
    }

    #[tokio::test]
    async fn the_werewolves_win_on_reaching_parity() {
        // One werewolf against two: whoever it kills the first night, the
        // werewolf then equals the village.
        let roles = HashMap::from([
            (id("wolf"), Role::Werewolf),
            (id("ann"), Role::Villager),
            (id("bob"), Role::Villager),
        ]);
        let (winner, _) = play(roles).await;
        assert_eq!(winner, Team::Werewolves);
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
