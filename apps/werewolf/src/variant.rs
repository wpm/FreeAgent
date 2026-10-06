//! A variant of the game is what the environment and each kind of agent
//! do by day and by night.
//!
//! The rules that never vary live outside the variant: the state and who
//! may see what of it, the round structure, the win condition, and the
//! end of the game. A variant fills in the policies: how the environment
//! runs a day and a night, and how a werewolf, villager, doctor, or seer
//! acts on what it sees in each. The [random] variant answers everything
//! uniformly at random; a variant of talking players can run a discussion
//! in the same methods, since each has the actor's [`Context`] and can
//! send whatever it likes.

pub mod random;

use crate::state::State;
use crate::{Message, PlayerId, Role};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{Context, Policy};

/// The environment: the actor with read and write access to the
/// [`State`], and a policy for each phase to run on it.
///
/// There is no environment without a state, which is why the trait asks
/// for it. Each phase is given the environment's context to talk to the
/// agents through and does what it likes to the state. When it returns,
/// the phase is over; the round structure checks for a winner and moves
/// on.
#[async_trait]
pub trait Environment: Send {
    /// The game as the environment knows it.
    fn state(&self) -> &State;

    /// The game as the environment knows it, to change.
    fn state_mut(&mut self) -> &mut State;

    /// Run the day.
    async fn day(&mut self, context: &Context<Message>) -> Result<()>;

    /// Run the night.
    async fn night(&mut self, context: &Context<Message>) -> Result<()>;

    /// Take a player out of the game. Dead is dead: the player's actor is
    /// stopped and the player leaves the living, together and nowhere
    /// else.
    fn kill(&mut self, victim: &PlayerId, context: &Context<Message>) -> Result<()> {
        context.stop(victim)?;
        self.state_mut().kill(victim);
        Ok(())
    }
}

/// A werewolf: kills by night and votes by day.
///
/// An agent has no access to the state. Each policy is given the agent's
/// observation of the game, its own context, and answers with the player
/// it selects, if any.
#[async_trait]
pub trait Werewolf: Send {
    /// Vote for someone.
    async fn day(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;

    /// Choose a victim.
    async fn night(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;
}

/// A villager: votes by day and nothing more. It has no night.
#[async_trait]
pub trait Villager: Send {
    /// Vote for someone.
    async fn day(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;
}

/// A doctor: votes by day and saves someone by night.
#[async_trait]
pub trait Doctor: Send {
    /// Vote for someone.
    async fn day(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;

    /// Choose someone to save.
    async fn night(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;
}

/// A seer: votes by day and looks at someone by night.
#[async_trait]
pub trait Seer: Send {
    /// Vote for someone.
    async fn day(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;

    /// Choose someone to look at.
    async fn night(
        &mut self,
        observation: &State,
        context: &Context<Message>,
    ) -> Result<Option<PlayerId>>;
}

/// A whole variant: a way to build the environment and each kind of
/// agent. Whatever randomness a variant uses follows from one seed it
/// holds, so the same variant plays the same game.
pub trait Variant {
    /// The environment, holding this game.
    fn environment(&self, state: State) -> Box<dyn Environment>;

    /// A werewolf, for this seat.
    fn werewolf(&self, seat: &PlayerId) -> Box<dyn Werewolf>;

    /// A villager, for this seat.
    fn villager(&self, seat: &PlayerId) -> Box<dyn Villager>;

    /// A doctor, for this seat.
    fn doctor(&self, seat: &PlayerId) -> Box<dyn Doctor>;

    /// A seer, for this seat.
    fn seer(&self, seat: &PlayerId) -> Box<dyn Seer>;

    /// The agent for a seat dealt this role, ready to act.
    fn agent(&self, role: Role, seat: &PlayerId) -> Box<dyn Policy<Message = Message> + Send> {
        match role {
            Role::Werewolf => Box::new(self.werewolf(seat)),
            Role::Villager => Box::new(self.villager(seat)),
            Role::Doctor => Box::new(self.doctor(seat)),
            Role::Seer => Box::new(self.seer(seat)),
        }
    }
}
