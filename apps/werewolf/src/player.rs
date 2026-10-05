//! The actor that plays one seat.

use crate::state::State;
use crate::variant::Decide;
use crate::{Message, Phase, PlayerId, Role};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy};

/// One player: a role, which fixes what it is asked to do each phase, and
/// a way of deciding, which is the game's variant.
pub struct Player<D: Decide> {
    role: Role,
    decide: D,
}

impl<D: Decide> Player<D> {
    /// A player of this role that decides this way.
    pub fn new(role: Role, decide: D) -> Self {
        Player { role, decide }
    }

    /// The night step. Only a werewolf acts, choosing a victim from the
    /// living outside its pack.
    async fn night(&mut self, seen: &State) -> Result<Option<Message>> {
        if self.role != Role::Werewolf {
            return Ok(None);
        }
        let candidates: Vec<_> = seen
            .living()
            .into_iter()
            .filter(|id| seen.roles().get(id) != Some(&Role::Werewolf))
            .collect();
        let victim = self
            .decide
            .choose(self.role, Phase::Night, seen, &candidates)
            .await?;
        Ok(victim.map(Message::Kill))
    }

    /// The day step. Everyone votes for someone living other than
    /// themselves.
    async fn day(&mut self, seen: &State, me: &PlayerId) -> Result<Option<Message>> {
        let candidates: Vec<_> = seen.living().into_iter().filter(|id| id != me).collect();
        let vote = self
            .decide
            .choose(self.role, Phase::Day, seen, &candidates)
            .await?;
        Ok(vote.map(Message::Vote))
    }
}

#[async_trait]
impl<D: Decide> Policy for Player<D> {
    type Message = Message;

    async fn reply(
        &mut self,
        _from: ActorId,
        message: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        match message {
            Message::Night(seen) => self.night(&seen).await,
            Message::Day(seen) => self.day(&seen, context.id()).await,
            Message::Over(_) => {
                context.shutdown();
                Ok(None)
            }
            Message::Kill(_) | Message::Vote(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variant::random::Uniform;
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
                .request(&to, Message::Over(crate::Team::Villagers), None)
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
        let full = State::new(roles);
        full.observation_for(&"player".to_string())
    }

    async fn ask(role: Role, question: Message) -> Reply<Message> {
        let (report, mut answers) = unbounded_channel();
        let pollster: Box<dyn Policy<Message = Message> + Send> =
            Box::new(Pollster { question, report });
        let player: Box<dyn Policy<Message = Message> + Send> =
            Box::new(Player::new(role, Uniform::new(3)));
        episode(
            [
                ("pollster".to_string(), pollster),
                ("player".to_string(), player),
            ],
            None,
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        answers.recv().await.unwrap()
    }

    #[tokio::test]
    async fn a_werewolf_kills_someone_outside_its_pack_by_night() {
        let seen = seen(Role::Werewolf, &["wolf2"], &["ann", "bob"]);
        let reply = ask(Role::Werewolf, Message::Night(seen)).await;
        let Reply::Message(Message::Kill(victim)) = reply else {
            panic!("{reply:?}");
        };
        assert!(["ann", "bob"].contains(&victim.as_str()), "{victim}");
    }

    #[tokio::test]
    async fn a_villager_sleeps_through_the_night() {
        let seen = seen(Role::Villager, &[], &["ann", "bob"]);
        let reply = ask(Role::Villager, Message::Night(seen)).await;
        assert_eq!(reply, Reply::Acknowledge);
    }

    #[tokio::test]
    async fn everyone_votes_for_someone_else_by_day() {
        for role in [Role::Villager, Role::Werewolf] {
            let seen = seen(role, &[], &["ann", "bob"]);
            let reply = ask(role, Message::Day(seen)).await;
            let Reply::Message(Message::Vote(vote)) = reply else {
                panic!("{reply:?}");
            };
            assert!(["ann", "bob"].contains(&vote.as_str()), "{vote}");
        }
    }
}
