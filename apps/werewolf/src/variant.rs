//! A variant of the game is what each participant does by night and by
//! day: the environment on one side, each role on the other.
//!
//! The rules that never vary live outside the variant: the state and who
//! may see what of it, the win condition, the round structure, and the
//! announcement of the winner. Everything within a night or a day is the
//! variant's. In the [random] variant that is a vote and nothing
//! more; a variant of talking players can run a discussion in the same
//! methods, since each has the actor's [`Context`] and can send whatever
//! it likes.

pub mod random;

use crate::state::State;
use crate::{Message, PlayerId, Role};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context};

/// The environment's side of a variant: the state of the game, and how
/// each phase is run on it.
///
/// Every environment holds the whole [`State`], which is why the trait
/// asks for it: there is no environment without one. Each phase is given
/// the environment's context to talk to the players through, and does
/// what it likes to the state. When it returns, the phase is over; the
/// environment checks for a winner and moves on.
#[async_trait]
pub trait Moderate: Send {
    /// The game as the environment knows it.
    fn state(&self) -> &State;

    /// The game as the environment knows it, to change.
    fn state_mut(&mut self) -> &mut State;

    /// Run the night.
    async fn night(&mut self, context: &Context<Message>) -> Result<()>;

    /// Run the day.
    async fn day(&mut self, context: &Context<Message>) -> Result<()>;

    /// Take a player out of the game. Dead is dead: the player's actor is
    /// stopped and the player leaves the living, together and nowhere
    /// else.
    fn kill(&mut self, victim: &PlayerId, context: &Context<Message>) -> Result<()> {
        context.stop(victim)?;
        self.state_mut().kill(victim);
        Ok(())
    }
}

/// A seat's side of a variant: how one player of one role acts on what it
/// is sent.
///
/// At this level there are only messages. A night or day prompt from the
/// environment, another player's words, anything else a variant puts on
/// the wire: each arrives here the same way, as an observation, and what
/// it means is the variant's business. The answer, if any, goes back to
/// whoever sent it.
///
/// A player can only talk. It is never given the state, so nothing it
/// does changes the game except as the environment's side of the variant
/// chooses to act on what it says.
#[async_trait]
pub trait Play: Send {
    /// Act on a message from `from`, answering it or not.
    async fn act(
        &mut self,
        from: ActorId,
        observation: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>>;
}

/// A whole variant: a way to build each participant.
pub trait Variant {
    /// The environment's side, holding this game.
    fn environment(&self, state: State, seed: u64) -> Box<dyn Moderate>;

    /// One player's side, for a seat dealt this role.
    fn player(&self, role: Role, seed: u64) -> Box<dyn Play>;
}
