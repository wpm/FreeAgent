//! The player who has no idea.

use super::{Message, PlayerId};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy};
use rand::prelude::*;
use rand::rngs::StdRng;

/// Nominates a uniformly random candidate whenever asked, never itself,
/// and says nothing.
///
/// This is the paper's player. It learns its role and forgets it, hears
/// the village and ignores it, and votes and kills without a thought.
pub struct UniformRandom {
    rng: StdRng,
}

impl UniformRandom {
    /// A player whose choices follow from the seed.
    pub fn new(seed: u64) -> Self {
        UniformRandom {
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// One of the candidates other than `me`, chosen uniformly.
    fn pick(&mut self, candidates: &[PlayerId], me: &PlayerId) -> Option<PlayerId> {
        candidates
            .iter()
            .filter(|name| *name != me)
            .choose(&mut self.rng)
            .cloned()
    }
}

#[async_trait]
impl Policy for UniformRandom {
    type Message = Message;

    async fn reply(
        &mut self,
        _from: ActorId,
        message: Message,
        context: &Context<Message>,
    ) -> Result<Option<Message>> {
        match message {
            Message::Turn { candidates } => Ok(Some(Message::Statement {
                said: None,
                nominated: self.pick(&candidates, context.id()),
            })),
            Message::Kill { candidates } => {
                Ok(self.pick(&candidates, context.id()).map(Message::Choice))
            }
            Message::GameOver { .. } => {
                context.shutdown();
                Ok(None)
            }
            Message::YouAre { .. }
            | Message::Heard { .. }
            | Message::Died { .. }
            | Message::Statement { .. }
            | Message::Choice(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Team;
    use free_agent::{Reply, episode};
    use std::collections::HashSet;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

    /// Puts one question to a player and reports what came back.
    struct Pollster {
        question: Message,
        report: UnboundedSender<Option<Message>>,
    }

    #[async_trait]
    impl Policy for Pollster {
        type Message = Message;

        async fn start(&mut self, context: &Context<Message>) -> Result<()> {
            let player = HashSet::from([Some("player".to_string())]);
            let mut replies = context
                .request(&player, self.question.clone(), None)
                .await?;
            let answer = match replies.pop() {
                Some((_, Reply::Message(message))) => Some(message),
                _ => None,
            };
            self.report.send(answer)?;
            let over = Message::GameOver {
                winner: Team::Villagers,
            };
            context.request(&player, over, None).await?;
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

    fn names(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| name.to_string()).collect()
    }

    async fn ask(question: Message, seed: u64) -> Option<Message> {
        let (report, mut reported) = unbounded_channel();
        let pollster = Pollster { question, report };
        let actors: [(ActorId, Box<dyn Policy<Message = Message> + Send>); 2] = [
            ("pollster".to_string(), Box::new(pollster)),
            ("player".to_string(), Box::new(UniformRandom::new(seed))),
        ];
        episode(actors, None, Duration::from_secs(5), None)
            .await
            .unwrap();
        reported.recv().await.unwrap()
    }

    #[tokio::test]
    async fn on_its_turn_a_random_player_nominates_a_candidate_and_says_nothing() {
        for seed in 0..10 {
            let turn = Message::Turn {
                candidates: names(&["ann", "bob", "cat"]),
            };
            match ask(turn, seed).await {
                Some(Message::Statement {
                    said: None,
                    nominated: Some(name),
                }) => assert!(["ann", "bob", "cat"].contains(&name.as_str())),
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_random_player_never_nominates_itself() {
        for seed in 0..10 {
            let turn = Message::Turn {
                candidates: names(&["player", "ann"]),
            };
            let statement = ask(turn, seed).await;
            assert!(
                matches!(&statement, Some(Message::Statement { nominated: Some(name), .. }) if name == "ann"),
                "{statement:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_random_werewolf_names_a_victim() {
        let kill = Message::Kill {
            candidates: names(&["ann", "bob"]),
        };

        let choice = ask(kill, 0).await;

        assert!(
            matches!(&choice, Some(Message::Choice(name)) if name == "ann" || name == "bob"),
            "{choice:?}"
        );
    }

    #[tokio::test]
    async fn a_random_player_with_no_one_to_name_abstains() {
        let kill = Message::Kill {
            candidates: names(&["player"]),
        };

        assert_eq!(ask(kill, 0).await, None);
    }
}
