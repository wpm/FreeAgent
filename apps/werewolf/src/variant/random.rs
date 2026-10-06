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
//! random. Every agent answers with a uniformly random choice. With
//! everyone choosing uniformly, a plurality vote with ties broken at
//! random eliminates a uniformly random candidate, which is exactly the
//! paper's rule. The doctor and the seer are never asked anything by
//! night, since the environment has no use for what they would say.

use crate::environment::{NAME, plurality, poll};
use crate::state::State;
use crate::variant::{self, Environment as _};
use crate::{Message, PlayerId, Role, Team};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context};
use rand::prelude::*;
use rand::rngs::StdRng;
use std::collections::HashMap;
use std::time::Duration;

/// The random variant.
#[derive(Debug, Clone)]
pub struct Random {
    /// Each actor's own seed, drawn from the game's seed when the variant
    /// is made, so that each actor's choices are independent of the
    /// others' and of the order the actors are made in.
    seeds: HashMap<ActorId, u64>,
    /// How long the environment waits for any one agent's answer.
    patience: Duration,
}

impl Random {
    /// The variant for a game among these seats, every random choice in
    /// which follows from the seed.
    pub fn new(seed: u64, seats: impl IntoIterator<Item = PlayerId>, patience: Duration) -> Self {
        let mut actors: Vec<_> = seats.into_iter().collect();
        actors.push(NAME.to_string());
        actors.sort();
        let mut rng = StdRng::seed_from_u64(seed);
        let seeds = actors
            .into_iter()
            .map(|actor| (actor, rng.random()))
            .collect();
        Random { seeds, patience }
    }

    /// The random stream of this actor's own.
    ///
    /// # Panics
    ///
    /// If the actor is not one the variant was made for.
    fn stream(&self, actor: &str) -> StdRng {
        let seed = self
            .seeds
            .get(actor)
            .unwrap_or_else(|| panic!("{actor} is not a seat the random variant was made for"));
        StdRng::seed_from_u64(*seed)
    }
}

impl variant::Variant for Random {
    fn environment(&self, state: State) -> Box<dyn variant::Environment> {
        Box::new(Environment {
            state,
            rng: self.stream(NAME),
            patience: self.patience,
        })
    }

    fn werewolf(&self, seat: &PlayerId) -> Box<dyn variant::Werewolf> {
        Box::new(Uniform(self.stream(seat)))
    }

    fn villager(&self, seat: &PlayerId) -> Box<dyn variant::Villager> {
        Box::new(Uniform(self.stream(seat)))
    }

    fn doctor(&self, seat: &PlayerId) -> Box<dyn variant::Doctor> {
        Box::new(Uniform(self.stream(seat)))
    }

    fn seer(&self, seat: &PlayerId) -> Box<dyn variant::Seer> {
        Box::new(Uniform(self.stream(seat)))
    }
}

/// The environment: a poll each phase, with ties broken at random.
pub struct Environment {
    state: State,
    rng: StdRng,
    patience: Duration,
}

impl Environment {
    /// Ask some agents to select among the living, and kill the plurality
    /// of the valid selections, with a tie broken at random.
    async fn eliminate(
        &mut self,
        voters: &[PlayerId],
        valid: impl Fn(&PlayerId, &PlayerId) -> bool,
        context: &Context<Message>,
    ) -> Result<()> {
        let votes = poll(&self.state, context, voters, self.patience, valid).await?;
        if let Some(victim) = plurality(&votes, &mut self.rng) {
            self.kill(&victim, context)?;
        }
        Ok(())
    }
}

#[async_trait]
impl variant::Environment for Environment {
    fn state(&self) -> &State {
        &self.state
    }

    fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// The living each vote for someone living other than themselves.
    async fn day(&mut self, context: &Context<Message>) -> Result<()> {
        let living = self.state.living();
        let valid =
            |voter: &PlayerId, chosen: &PlayerId| chosen != voter && living.contains(chosen);
        self.eliminate(&living, valid, context).await
    }

    /// The living werewolves each name a victim among the living
    /// villagers.
    async fn night(&mut self, context: &Context<Message>) -> Result<()> {
        let werewolves = self.state.living_on(Team::Werewolves);
        let villagers = self.state.living_on(Team::Villagers);
        let valid = |_: &PlayerId, chosen: &PlayerId| villagers.contains(chosen);
        self.eliminate(&werewolves, valid, context).await
    }
}

/// Every kind of agent, played the same way: a uniformly random choice
/// whenever there is one to make.
pub struct Uniform(StdRng);

impl Uniform {
    /// One of the candidates, uniformly, or nobody if there are none.
    fn pick(&mut self, candidates: &[PlayerId]) -> Option<PlayerId> {
        candidates.choose(&mut self.0).cloned()
    }

    /// A vote for a random living player other than oneself: every
    /// kind of agent's day.
    fn vote(&mut self, observation: &State, context: &Context<Message>) -> Option<PlayerId> {
        let me = context.id();
        let candidates: Vec<_> = observation
            .living()
            .into_iter()
            .filter(|id| id != me)
            .collect();
        self.pick(&candidates)
    }
}

#[async_trait]
impl variant::Werewolf for Uniform {
    async fn day(&mut self, seen: &State, context: &Context<Message>) -> Result<Option<PlayerId>> {
        Ok(self.vote(seen, context))
    }

    /// A victim chosen uniformly among the living outside the pack.
    async fn night(&mut self, seen: &State, _: &Context<Message>) -> Result<Option<PlayerId>> {
        let candidates: Vec<_> = seen
            .living()
            .into_iter()
            .filter(|id| seen.roles().get(id) != Some(&Role::Werewolf))
            .collect();
        Ok(self.pick(&candidates))
    }
}

#[async_trait]
impl variant::Villager for Uniform {
    async fn day(&mut self, seen: &State, context: &Context<Message>) -> Result<Option<PlayerId>> {
        Ok(self.vote(seen, context))
    }
}

/// The doctor is never asked anything by night in this variant.
#[async_trait]
impl variant::Doctor for Uniform {
    async fn day(&mut self, seen: &State, context: &Context<Message>) -> Result<Option<PlayerId>> {
        Ok(self.vote(seen, context))
    }

    async fn night(&mut self, _: &State, _: &Context<Message>) -> Result<Option<PlayerId>> {
        Ok(None)
    }
}

/// The seer is never asked anything by night in this variant.
#[async_trait]
impl variant::Seer for Uniform {
    async fn day(&mut self, seen: &State, context: &Context<Message>) -> Result<Option<PlayerId>> {
        Ok(self.vote(seen, context))
    }

    async fn night(&mut self, _: &State, _: &Context<Message>) -> Result<Option<PlayerId>> {
        Ok(None)
    }
}
