//! Werewolf played at random, after "Mafia: A Theoretical Study of Players
//! and Coalitions in a Partial Information Environment" by Braverman,
//! Etesami, and Mossel.
//!
//! The paper strips the game to its arithmetic. Nobody has any information
//! to act on, so everyone chooses uniformly at random, and the only
//! question is how many werewolves it takes to make the game fair. The
//! paper's answer is about the square root of the number of players.
//!
//! The environment's night asks the werewolves for a victim and its day
//! asks the living for a vote; the plurality dies, with ties broken at
//! random. Every player answers with a uniformly random choice. With
//! everyone choosing uniformly, a plurality vote with ties broken at
//! random eliminates a uniformly random candidate, which is exactly the
//! paper's rule. The seer and the doctor have nothing to act on either,
//! so they play as villagers.

use super::{Moderate, Play, Variant};
use crate::environment::{plurality, poll};
use crate::state::State;
use crate::{Message, PlayerId, Role, Team};
use anyhow::{Result, bail};
use async_trait::async_trait;
use free_agent::{ActorId, Context};
use rand::prelude::*;
use rand::rngs::StdRng;
use std::time::Duration;

/// The random variant.
#[derive(Debug, Clone, Copy)]
pub struct Random {
    /// How long the environment waits for any one player's answer.
    pub patience: Duration,
}

impl Variant for Random {
    fn environment(&self, seed: u64) -> Box<dyn Moderate> {
        Box::new(Environment::new(seed, self.patience))
    }

    fn player(&self, role: Role, seed: u64) -> Box<dyn Play> {
        match role {
            Role::Werewolf => Box::new(Werewolf::new(seed)),
            Role::Villager | Role::Seer | Role::Doctor => Box::new(Villager::new(seed)),
        }
    }
}

/// The environment's side: a poll each phase, with ties broken at random.
pub struct Environment {
    rng: StdRng,
    patience: Duration,
}

impl Environment {
    /// An environment whose tie-breaks follow from the seed.
    pub fn new(seed: u64, patience: Duration) -> Self {
        Environment {
            rng: StdRng::seed_from_u64(seed),
            patience,
        }
    }
}

#[async_trait]
impl Moderate for Environment {
    /// The living werewolves each name a victim among the living
    /// villagers, and the plurality dies.
    async fn night(&mut self, state: &mut State, context: &Context<Message>) -> Result<()> {
        let werewolves = state.living_on(Team::Werewolves);
        let villagers = state.living_on(Team::Villagers);
        let votes = poll(
            state,
            context,
            &werewolves,
            Message::Night,
            self.patience,
            |_, chosen| villagers.contains(chosen),
        )
        .await?;
        if let Some(victim) = plurality(&votes, &mut self.rng) {
            self.kill(state, &victim, context)?;
        }
        Ok(())
    }

    /// The living each vote for someone living other than themselves,
    /// and the plurality is eliminated.
    async fn day(&mut self, state: &mut State, context: &Context<Message>) -> Result<()> {
        let living = state.living();
        let votes = poll(
            state,
            context,
            &living,
            Message::Day,
            self.patience,
            |voter, chosen| chosen != voter && living.contains(chosen),
        )
        .await?;
        if let Some(victim) = plurality(&votes, &mut self.rng) {
            self.kill(state, &victim, context)?;
        }
        Ok(())
    }
}

/// A werewolf: kills a random villager by night and votes at random by
/// day.
pub struct Werewolf {
    rng: StdRng,
}

impl Werewolf {
    /// A werewolf whose choices follow from the seed.
    pub fn new(seed: u64) -> Self {
        Werewolf {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

#[async_trait]
impl Play for Werewolf {
    /// By night, a victim chosen uniformly among the living outside the
    /// pack; by day, a vote.
    async fn act(
        &mut self,
        _from: ActorId,
        observation: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        Ok(match observation {
            Message::Night(seen) => {
                let candidates: Vec<_> = seen
                    .living()
                    .into_iter()
                    .filter(|id| seen.roles().get(id) != Some(&Role::Werewolf))
                    .collect();
                candidates
                    .choose(&mut self.rng)
                    .cloned()
                    .map(Message::Select)
            }
            Message::Day(seen) => vote(&mut self.rng, &seen, context.id()),
            other => bail!("a werewolf was sent neither a night nor a day: {other:?}"),
        })
    }
}

/// A villager: sleeps by night and votes at random by day.
pub struct Villager {
    rng: StdRng,
}

impl Villager {
    /// A villager whose choices follow from the seed.
    pub fn new(seed: u64) -> Self {
        Villager {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

#[async_trait]
impl Play for Villager {
    /// By day, a vote. A villager is never asked anything by night, so
    /// being sent anything else is an error.
    async fn act(
        &mut self,
        _from: ActorId,
        observation: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        Ok(match observation {
            Message::Day(seen) => vote(&mut self.rng, &seen, context.id()),
            other => bail!("a villager was sent something other than a day: {other:?}"),
        })
    }
}

/// A vote for a uniformly random living player other than `me`.
fn vote(rng: &mut StdRng, seen: &State, me: &PlayerId) -> Option<Message> {
    let candidates: Vec<_> = seen.living().into_iter().filter(|id| id != me).collect();
    candidates.choose(rng).cloned().map(Message::Select)
}
