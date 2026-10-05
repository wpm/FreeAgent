//! The player who has no idea, after Braverman, Etesami, and Mossel.

use super::Decide;
use crate::state::State;
use crate::{Phase, PlayerId, Role};
use anyhow::Result;
use async_trait::async_trait;
use rand::prelude::*;
use rand::rngs::StdRng;

/// Chooses uniformly among the candidates, whatever the question.
pub struct Uniform {
    rng: StdRng,
}

impl Uniform {
    /// A chooser whose choices follow from the seed.
    pub fn new(seed: u64) -> Self {
        Uniform {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

#[async_trait]
impl Decide for Uniform {
    async fn choose(
        &mut self,
        _role: Role,
        _phase: Phase,
        _seen: &State,
        candidates: &[PlayerId],
    ) -> Result<Option<PlayerId>> {
        Ok(candidates.choose(&mut self.rng).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn names(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[tokio::test]
    async fn chooses_one_of_the_candidates() {
        let seen = State::new(HashMap::new());
        let candidates = names(&["ann", "bob", "cat"]);
        let mut uniform = Uniform::new(1);
        for _ in 0..20 {
            let chosen = uniform
                .choose(Role::Villager, Phase::Day, &seen, &candidates)
                .await
                .unwrap()
                .unwrap();
            assert!(candidates.contains(&chosen));
        }
    }

    #[tokio::test]
    async fn chooses_nobody_from_no_candidates() {
        let seen = State::new(HashMap::new());
        let chosen = Uniform::new(1)
            .choose(Role::Villager, Phase::Day, &seen, &[])
            .await
            .unwrap();
        assert_eq!(chosen, None);
    }

    #[tokio::test]
    async fn the_same_seed_makes_the_same_choices() {
        let seen = State::new(HashMap::new());
        let candidates = names(&["ann", "bob", "cat", "dan"]);
        let mut first = Uniform::new(7);
        let mut second = Uniform::new(7);
        for _ in 0..10 {
            let a = first
                .choose(Role::Villager, Phase::Day, &seen, &candidates)
                .await
                .unwrap();
            let b = second
                .choose(Role::Villager, Phase::Day, &seen, &candidates)
                .await
                .unwrap();
            assert_eq!(a, b);
        }
    }
}
