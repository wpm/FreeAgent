//! How a player decides. A role fixes what a player is asked; the variant
//! fixes how it answers: uniformly at random, or by asking a model.

pub mod random;

use crate::state::State;
use crate::{Phase, PlayerId, Role};
use anyhow::Result;
use async_trait::async_trait;

/// The one choice every step comes down to: which of these players, if
/// any.
#[async_trait]
pub trait Decide: Send {
    /// Choose one of `candidates`, or nobody. The role and phase say what
    /// the choice is for: a werewolf's victim by night, anyone's vote by
    /// day. `seen` is the chooser's observation of the game.
    async fn choose(
        &mut self,
        role: Role,
        phase: Phase,
        seen: &State,
        candidates: &[PlayerId],
    ) -> Result<Option<PlayerId>>;
}
