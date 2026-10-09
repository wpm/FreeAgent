//! Players who choose at random, and the environment that runs their game.

use crate::{Observation, PlayerId, Role, State};
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context};
use futures_util::future::try_join_all;
use rand::seq::IndexedRandom;
use std::collections::{HashMap, HashSet};

/// What an actor in the game does: run it, or play in it. An episode holds
/// one kind of actor, so the two sides meet here and each method goes to
/// whichever side this is.
pub(crate) enum Actor {
    /// The side that holds the game.
    Environment(Environment),
    /// A side that sees only what it is shown.
    Player(Player),
}

impl Actor {
    /// Builds the environment for a game of `roles` once the episode has
    /// made its context.
    pub(crate) fn environment(roles: HashMap<PlayerId, Role>) -> Builder<Self> {
        Box::new(move |context| {
            Actor::Environment(Environment {
                context,
                state: State::new(roles),
            })
        })
    }

    /// Builds a player once the episode has made its context.
    pub(crate) fn player() -> Builder<Self> {
        Box::new(|context| Actor::Player(Player { context }))
    }
}

#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Message;

    fn context(&self) -> &Context<Message> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Player(player) => player.context(),
        }
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.initialize().await,
            Actor::Player(player) => player.initialize().await,
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
        }
    }

    async fn answer(&mut self, message: &Message) -> anyhow::Result<Vec<Message>> {
        match self {
            Actor::Environment(environment) => environment.answer(message).await,
            Actor::Player(player) => player.answer(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Player(player) => player.start().await,
        }
    }

    async fn clean_up(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.clean_up().await,
            Actor::Player(player) => player.clean_up().await,
        }
    }
}

/// The actor that holds the game and tells each player what it may see.
pub(crate) struct Environment {
    context: Context<Message>,
    state: State,
}

impl Environment {
    /// Ask everyone awake what they choose, all at once, and gather their
    /// replies by player.
    async fn night(&self) -> anyhow::Result<HashMap<PlayerId, Vec<Message>>> {
        let asked = self
            .state
            .awake()
            .into_iter()
            .map(|player| self.ask(player));
        let replies = try_join_all(asked).await?;
        Ok(replies.into_iter().flatten().collect())
    }

    async fn day(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Show `player` what it may see and wait for what it says back.
    async fn ask(&self, player: PlayerId) -> anyhow::Result<HashMap<PlayerId, Vec<Message>>> {
        let observation = self.state.observation(player.clone())?;
        self.context
            .request(Message::Observation(observation), HashSet::from([player]))
            .await
    }
}

#[async_trait]
impl Behavior for Environment {
    type Message = Message;
    type Log = Message;

    fn context(&self) -> &Context<Message> {
        &self.context
    }

    /// For now a game is one night: ask everyone awake, write down what
    /// they chose, and end the episode.
    async fn start(&mut self) -> anyhow::Result<()> {
        let replies = self.night().await?;
        for choice in replies.into_values().flatten() {
            self.context.log(choice);
        }
        for player in self.state.roles.keys() {
            self.context.stop(player)?;
        }
        self.context.shutdown();
        Ok(())
    }
}

/// A player who, asked to choose, picks at random among the living whose
/// role it does not know. That keeps a werewolf from choosing a werewolf and
/// the seer from asking about anyone twice.
pub(crate) struct Player {
    context: Context<Message>,
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
    use free_agent::{ActorInit, Episode};
    use std::collections::HashSet;
    use std::num::NonZero;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

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

    #[tokio::test]
    async fn a_night_gathers_one_choice_from_each_awake_player() {
        let roles = village();
        let players: HashSet<PlayerId> = roles.keys().cloned().collect();
        let mut init = HashMap::from([(
            id("environment"),
            ActorInit {
                behavior: Actor::environment(roles.clone()),
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

        let mut choices = Vec::new();
        while let Some(event) = log.recv().await {
            let Message::Action(chosen) = event.payload else {
                panic!("the environment logs choices, not {:?}", event.payload);
            };
            choices.push(chosen);
        }
        // The two werewolves and the seer each choose; the villager sleeps.
        assert_eq!(choices.len(), 3, "{choices:?}");
        assert!(choices.iter().all(|chosen| roles.contains_key(chosen)));
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
