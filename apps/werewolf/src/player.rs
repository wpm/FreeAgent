//! The actor that plays one seat.

use crate::Message;
use crate::variant::Play;
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy};

/// Any [`Play`], boxed, is a player actor. What it does with what it is
/// sent is the variant's; what is the same in every variant is here: the
/// end of the game is the end of the actor.
#[async_trait]
impl Policy for Box<dyn Play> {
    type Message = Message;

    async fn reply(
        &mut self,
        from: ActorId,
        message: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        if let Message::Over(_) = message {
            context.shutdown();
            return Ok(None);
        }
        self.act(from, message, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use crate::variant::Variant;
    use crate::variant::random::Random;
    use crate::{Role, Team};
    use free_agent::{Recipient, Reply, episode};
    use std::collections::{HashMap, HashSet};
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

    /// Puts one question to a player and reports what came back.
    struct Pollster {
        question: Message,
        report: UnboundedSender<Reply<Message>>,
    }

    #[async_trait]
    impl Policy for Pollster {
        type Message = Message;

        async fn start(&mut self, context: &Context<Message>) -> Result<()> {
            let to: HashSet<Recipient> = HashSet::from([Some("player".to_string())]);
            let mut replies = context
                .request(&to, self.question.clone(), Some(Duration::from_secs(5)))
                .await?;
            let (_, reply) = replies.pop().unwrap();
            self.report.send(reply).unwrap();
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

    /// A game as `player` sees it: it and the others named, everyone
    /// alive, with the roles the player is entitled to know.
    fn seen(player_role: Role, pack: &[&str], others: &[&str]) -> State {
        let mut roles = HashMap::from([("player".to_string(), player_role)]);
        for wolf in pack {
            roles.insert(wolf.to_string(), Role::Werewolf);
        }
        for other in others {
            roles.insert(other.to_string(), Role::Villager);
        }
        State::new(roles).observation_for(&"player".to_string())
    }

    /// Put one question to a random player of this role: what it
    /// answered, or the error that ended the episode.
    async fn ask(role: Role, question: Message) -> Result<Reply<Message>> {
        let (report, mut answers) = unbounded_channel();
        let random = Random {
            patience: Duration::from_secs(5),
        };
        let pollster: Box<dyn Policy<Message = Message> + Send> =
            Box::new(Pollster { question, report });
        let player: Box<dyn Policy<Message = Message> + Send> = Box::new(random.player(role, 3));
        episode(
            [
                ("pollster".to_string(), pollster),
                ("player".to_string(), player),
            ],
            None,
            Duration::from_secs(5),
            None,
        )
        .await?;
        Ok(answers.recv().await.unwrap())
    }

    #[tokio::test]
    async fn a_werewolf_kills_someone_outside_its_pack_by_night() {
        let seen = seen(Role::Werewolf, &["wolf2"], &["ann", "bob"]);
        let reply = ask(Role::Werewolf, Message::Night(seen)).await.unwrap();
        let Reply::Message(Message::Select(selected)) = reply else {
            panic!("{reply:?}");
        };
        assert!(["ann", "bob"].contains(&selected.as_str()), "{selected}");
    }

    #[tokio::test]
    async fn sending_anyone_but_a_werewolf_the_night_ends_the_episode() {
        for role in [Role::Villager, Role::Seer, Role::Doctor] {
            let seen = seen(role, &[], &["ann", "bob"]);
            let error = ask(role, Message::Night(seen)).await.unwrap_err();
            assert!(
                error.to_string().contains("something other than a day"),
                "{role:?}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn everyone_votes_for_someone_else_by_day() {
        for role in [Role::Villager, Role::Werewolf, Role::Seer, Role::Doctor] {
            let seen = seen(role, &[], &["ann", "bob"]);
            let reply = ask(role, Message::Day(seen)).await.unwrap();
            let Reply::Message(Message::Select(selected)) = reply else {
                panic!("{role:?}: {reply:?}");
            };
            assert!(["ann", "bob"].contains(&selected.as_str()), "{selected}");
        }
    }

    #[tokio::test]
    async fn sending_a_random_player_anything_but_a_prompt_ends_the_episode() {
        let error = ask(Role::Villager, Message::Select("ann".to_string()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("a villager was sent"), "{error}");
        let error = ask(Role::Werewolf, Message::Select("ann".to_string()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("a werewolf was sent"), "{error}");
    }
}
