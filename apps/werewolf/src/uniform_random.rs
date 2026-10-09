use crate::{Environment, Observation, PlayerId};
use free_agent::{Behavior, Context, Message};

impl Behavior for Environment {
    type Message = WerewolfMessage;
    type Log = WerewolfMessage;

    fn context(&self) -> &Context<Self::Message, Self::Log> {
        todo!()
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
