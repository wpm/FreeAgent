//! The agents as actors: each kind of agent, boxed, is a policy that
//! routes an observation to its day or night policy by the phase.
//!
//! What is the same for every agent is here. An observation goes to the
//! policy for its phase and the selection comes back as the action; an
//! agent sent anything it has no policy for fails, since that is a bug
//! in the environment; and the end of the game is the end of the actor.

use crate::state::State;
use crate::variant::{Doctor, Seer, Villager, Werewolf};
use crate::{Message, Phase, PlayerId};
use anyhow::{Result, bail};
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy};

/// The observation in a message, or nothing if the message was the end
/// of the game, which shuts the actor down.
///
/// # Errors
///
/// Anything else sent to an agent is an error.
fn observed(message: Message, context: &Context<Message>) -> Result<Option<State>> {
    match message {
        Message::Observation(observation) => Ok(Some(observation)),
        Message::Over(_) => {
            context.shutdown();
            Ok(None)
        }
        other => bail!("{} is an agent and was sent {other:?}", context.id()),
    }
}

/// The selection as the action, if any.
fn selected(selection: Option<PlayerId>) -> Option<Message> {
    selection.map(Message::Select)
}

/// The policy for an agent kind that has both a day and a night: the
/// observation goes to whichever its phase names.
macro_rules! day_and_night {
    ($($agent:ident),* $(,)?) => {$(
        #[async_trait]
        impl Policy for Box<dyn $agent> {
            type Message = Message;

            async fn reply(
                &mut self,
                _from: ActorId,
                message: Message,
                context: &Context<Message>,
            ) -> Result<Option<Message>> {
                let Some(observation) = observed(message, context)? else {
                    return Ok(None);
                };
                let selection = match observation.phase() {
                    Phase::Day => self.day(&observation, context).await?,
                    Phase::Night => self.night(&observation, context).await?,
                };
                Ok(selected(selection))
            }
        }
    )*};
}

day_and_night!(Werewolf, Doctor, Seer);

/// A villager has only a day.
#[async_trait]
impl Policy for Box<dyn Villager> {
    type Message = Message;

    async fn reply(
        &mut self,
        _from: ActorId,
        message: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        let Some(observation) = observed(message, context)? else {
            return Ok(None);
        };
        let selection = match observation.phase() {
            Phase::Day => self.day(&observation, context).await?,
            Phase::Night => bail!("{} is a villager and has no night", context.id()),
        };
        Ok(selected(selection))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variant::Variant;
    use crate::variant::random::Random;
    use crate::{Role, Team};
    use free_agent::{Recipient, Reply, episode};
    use std::collections::{HashMap, HashSet};
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

    /// Sends one message to the agent and reports what came back.
    struct Pollster {
        message: Message,
        report: UnboundedSender<Reply<Message>>,
    }

    #[async_trait]
    impl Policy for Pollster {
        type Message = Message;

        async fn start(&mut self, context: &Context<Message>) -> Result<()> {
            let to: HashSet<Recipient> = HashSet::from([Some("agent".to_string())]);
            let mut replies = context
                .request(&to, self.message.clone(), Some(Duration::from_secs(5)))
                .await?;
            let (_, reply) = replies.pop().unwrap();
            self.report.send(reply)?;
            context
                .request(&to, Message::Over(Team::Villagers), None)
                .await?;
            context.shutdown();
            Ok(())
        }

        async fn reply(
            &mut self,
            _from: ActorId,
            _message: Message,
            _context: &Context<Message>,
        ) -> Result<Option<Message>> {
            Ok(None)
        }
    }

    /// The game as the agent sees it in this phase: it and the others
    /// named, everyone alive, with the roles the agent is entitled to
    /// know.
    fn observation(phase: Phase, role: Role, pack: &[&str], others: &[&str]) -> State {
        let mut roles = HashMap::from([("agent".to_string(), role)]);
        for wolf in pack {
            roles.insert(wolf.to_string(), Role::Werewolf);
        }
        for other in others {
            roles.insert(other.to_string(), Role::Villager);
        }
        let mut state = State::new(roles);
        if phase == Phase::Day {
            state.advance();
        }
        state.observation_for(&"agent".to_string())
    }

    /// Send one message to a random agent of this role: what it
    /// answered, or the error that ended the episode.
    async fn send(role: Role, message: Message) -> Result<Reply<Message>> {
        let (report, mut answers) = unbounded_channel();
        let random = Random::new(3, ["agent".to_string()], Duration::from_secs(5));
        let pollster: Box<dyn Policy<Message = Message> + Send> =
            Box::new(Pollster { message, report });
        episode(
            [
                ("pollster".to_string(), pollster),
                (
                    "agent".to_string(),
                    random.agent(role, &"agent".to_string()),
                ),
            ],
            None,
            Duration::from_secs(5),
            None,
        )
        .await?;
        Ok(answers.recv().await.unwrap())
    }

    /// The player a reply selected.
    fn selection(reply: Reply<Message>) -> PlayerId {
        match reply {
            Reply::Message(Message::Select(id)) => id,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_werewolf_selects_someone_outside_its_pack_by_night() {
        let seen = observation(Phase::Night, Role::Werewolf, &["wolf2"], &["ann", "bob"]);
        let reply = send(Role::Werewolf, Message::Observation(seen))
            .await
            .unwrap();
        let selected = selection(reply);
        assert!(["ann", "bob"].contains(&selected.as_str()), "{selected}");
    }

    #[tokio::test]
    async fn everyone_selects_someone_else_by_day() {
        for role in [Role::Villager, Role::Werewolf, Role::Seer, Role::Doctor] {
            let seen = observation(Phase::Day, role, &[], &["ann", "bob"]);
            let reply = send(role, Message::Observation(seen)).await.unwrap();
            let selected = selection(reply);
            assert!(
                ["ann", "bob"].contains(&selected.as_str()),
                "{role:?}: {selected}"
            );
        }
    }

    #[tokio::test]
    async fn a_random_doctor_or_seer_does_nothing_by_night() {
        for role in [Role::Doctor, Role::Seer] {
            let seen = observation(Phase::Night, role, &[], &["ann", "bob"]);
            let reply = send(role, Message::Observation(seen)).await.unwrap();
            assert_eq!(reply, Reply::Acknowledge, "{role:?}");
        }
    }

    #[tokio::test]
    async fn a_villager_sent_the_night_ends_the_episode() {
        let seen = observation(Phase::Night, Role::Villager, &[], &["ann", "bob"]);
        let error = send(Role::Villager, Message::Observation(seen))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("has no night"), "{error}");
    }

    #[tokio::test]
    async fn an_agent_sent_an_action_ends_the_episode() {
        for role in [Role::Villager, Role::Werewolf, Role::Seer, Role::Doctor] {
            let error = send(role, Message::Select("ann".to_string()))
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("is an agent"),
                "{role:?}: {error}"
            );
        }
    }
}
