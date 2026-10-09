use crate::{State, Observation, PlayerId, Role};
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context, Message};
use std::collections::HashMap;

/// The actor that holds the game and tells each player what it may see.
pub(crate) struct Environment {
    context: Context<WerewolfMessage>,
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
    type Message = WerewolfMessage;
    type Log = WerewolfMessage;

    fn context(&self) -> &Context<WerewolfMessage> {
        &self.context
    }
}

#[derive(Debug, Clone)]
pub(crate) enum WerewolfMessage {
    /// What a player may see, from the environment.
    Observation(Observation),
    /// A player's choice of another player, to the environment.
    Action(PlayerId),
}
impl Message for WerewolfMessage {}
