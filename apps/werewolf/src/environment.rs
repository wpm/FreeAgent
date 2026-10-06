//! The environment as an actor: any [`Environment`], boxed, is a policy
//! that runs one game from the first night to the end.
//!
//! What is the same for every environment is here: the rounds, with each
//! phase routed to the environment's day or night policy; the win check
//! between phases; and the end of the game. Also here are helpers an
//! environment may use to run a phase, though nothing obliges it to.

use crate::state::State;
use crate::variant::Environment;
use crate::{Message, Phase, PlayerId};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy, Recipient, Reply};
use rand::prelude::*;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// The environment's name as an actor.
pub const NAME: &str = "environment";

#[async_trait]
impl Policy for Box<dyn Environment> {
    type Message = Message;

    /// The whole game, start to finish.
    async fn start(&mut self, context: &Context<Message>) -> Result<()> {
        let winner = loop {
            if let Some(winner) = self.state().winner() {
                break winner;
            }
            match self.state().phase() {
                Phase::Day => self.day(context).await?,
                Phase::Night => self.night(context).await?,
            }
            if let Some(winner) = self.state().winner() {
                break winner;
            }
            self.state_mut().advance();
        };
        // The living are told, and nobody's acknowledgment is waited for:
        // the request goes out when it is made, and an agent's answer to
        // the news is to shut down.
        let living: HashSet<Recipient> = self.state().living().into_iter().map(Some).collect();
        drop(context.request(&living, Message::Over(winner), None));
        context.shutdown();
        Ok(())
    }

    /// Nobody asks the environment anything.
    async fn reply(
        &mut self,
        _from: ActorId,
        _message: Message,
        _context: &Context<Message>,
    ) -> Result<Option<Message>> {
        Ok(None)
    }
}

/// Send some agents their observations of `state` and collect the valid
/// selections that come back.
///
/// `valid` says whether an agent may select a candidate. An agent that
/// answers late, answers with something other than a selection, or
/// selects someone it may not, abstains.
pub async fn poll(
    state: &State,
    context: &Context<Message>,
    agents: &[PlayerId],
    patience: Duration,
    valid: impl Fn(&PlayerId, &PlayerId) -> bool,
) -> Result<Vec<PlayerId>> {
    let asked: Vec<_> = agents
        .iter()
        .map(|agent| {
            let observation = Message::Observation(state.observation_for(agent));
            let to: HashSet<Recipient> = HashSet::from([Some(agent.clone())]);
            (
                agent.clone(),
                context.request(&to, observation, Some(patience)),
            )
        })
        .collect();
    let mut selections = Vec::new();
    for (agent, pending) in asked {
        for (_, reply) in pending.await? {
            let Reply::Message(Message::Select(selected)) = reply else {
                continue;
            };
            if valid(&agent, &selected) {
                selections.push(selected);
            }
        }
    }
    Ok(selections)
}

/// Everyone tied for the most votes, in a fixed order. No votes, no
/// leaders.
pub fn leaders(votes: &[PlayerId]) -> Vec<PlayerId> {
    let mut tally: HashMap<&PlayerId, usize> = HashMap::new();
    for vote in votes {
        *tally.entry(vote).or_default() += 1;
    }
    let Some(most) = tally.values().max().copied() else {
        return vec![];
    };
    let mut leaders: Vec<_> = tally
        .into_iter()
        .filter(|(_, count)| *count == most)
        .map(|(id, _)| id.clone())
        .collect();
    leaders.sort();
    leaders
}

/// Whoever got the most votes, with a tie broken uniformly at random
/// among those tied. No votes choose nobody.
pub fn plurality(votes: &[PlayerId], rng: &mut impl Rng) -> Option<PlayerId> {
    leaders(votes).choose(rng).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variant::random::Random;
    use crate::variant::{Variant, Villager};
    use crate::{Role, Team};
    use free_agent::{Ending, Event, Log, Said, episode};
    use rand::rngs::StdRng;
    use tokio::sync::mpsc::UnboundedReceiver;

    /// The random variant for these seats, seeded.
    fn random(seed: u64, roles: &HashMap<PlayerId, Role>) -> Random {
        Random::new(seed, roles.keys().cloned(), Duration::from_secs(5))
    }

    fn votes(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn leaders_are_everyone_tied_for_the_most_votes() {
        assert_eq!(
            leaders(&votes(&["ann", "bob", "cat", "bob", "ann"])),
            vec!["ann", "bob"]
        );
        assert_eq!(leaders(&votes(&["ann"])), vec!["ann"]);
        assert!(leaders(&[]).is_empty());
    }

    #[test]
    fn no_votes_choose_nobody() {
        assert_eq!(plurality(&[], &mut StdRng::seed_from_u64(0)), None);
    }

    /// Roles for `werewolves` wolves and `villagers` villagers.
    fn roles(werewolves: u8, villagers: u8) -> HashMap<PlayerId, Role> {
        let wolves = (1..=werewolves).map(|n| (format!("wolf-{n}"), Role::Werewolf));
        let village = (1..=villagers).map(|n| (format!("villager-{n}"), Role::Villager));
        wolves.chain(village).collect()
    }

    type Boxed = Box<dyn Policy<Message = Message> + Send>;

    /// Play a game of the random variant and return every event on the
    /// wire.
    async fn play(roles: HashMap<PlayerId, Role>, seed: u64) -> Vec<Event<Message>> {
        let (log, events) = Log::new();
        let random = random(seed, &roles);
        let mut actors: Vec<(ActorId, Boxed)> = roles
            .iter()
            .map(|(id, role)| (id.clone(), random.agent(*role, id)))
            .collect();
        let environment: Boxed = Box::new(random.environment(State::new(roles)));
        actors.push((NAME.to_string(), environment));
        let ending = episode(actors, None, Duration::from_secs(30), Some(log))
            .await
            .unwrap();
        assert_eq!(ending, Ending::Finished);
        drain(events).await
    }

    async fn drain(mut events: UnboundedReceiver<Event<Message>>) -> Vec<Event<Message>> {
        let mut all = Vec::new();
        while let Some(event) = events.recv().await {
            all.push(event);
        }
        all
    }

    /// Who the environment told the ending to, and which side it said
    /// had won.
    fn told(events: &[Event<Message>]) -> Vec<(ActorId, Team)> {
        events
            .iter()
            .filter_map(|event| match &event.said {
                Said::Asked(Message::Over(team)) if event.from == NAME => {
                    Some((event.to.clone(), *team))
                }
                _ => None,
            })
            .collect()
    }

    /// The side the environment announced as the winner, once per agent
    /// told.
    fn announced(events: &[Event<Message>]) -> Vec<Team> {
        told(events).into_iter().map(|(_, team)| team).collect()
    }

    /// Whether an event sent `to` an observation of this phase.
    fn observed(event: &Event<Message>, to: &str, phase: Phase) -> bool {
        event.to == to
            && matches!(&event.said, Said::Asked(Message::Observation(seen)) if seen.phase() == phase)
    }

    #[tokio::test]
    async fn a_game_with_no_werewolves_is_won_before_it_starts() {
        let events = play(roles(0, 4), 1).await;
        assert_eq!(announced(&events), vec![Team::Villagers; 4]);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.said, Said::Asked(Message::Observation(_)))),
            "nobody should have been sent anything"
        );
    }

    #[tokio::test]
    async fn a_game_that_starts_at_parity_is_lost_before_it_starts() {
        let events = play(roles(2, 2), 1).await;
        assert_eq!(announced(&events), vec![Team::Werewolves; 4]);
    }

    #[tokio::test]
    async fn the_first_night_kills_a_villager() {
        let events = play(roles(1, 5), 2).await;
        let first_kill = events
            .iter()
            .find_map(|event| match &event.said {
                Said::Replied(Reply::Message(Message::Select(victim))) => Some(victim.clone()),
                _ => None,
            })
            .expect("the werewolf should have selected a victim");
        assert!(first_kill.starts_with("villager-"), "{first_kill}");
        // The victim never sees a day, and is not told the ending.
        assert!(!events.iter().any(|event| {
            observed(event, &first_kill, Phase::Day)
                || (event.to == first_kill && matches!(event.said, Said::Asked(Message::Over(_))))
        }));
    }

    #[tokio::test]
    async fn only_werewolves_see_the_night() {
        let events = play(roles(1, 5), 2).await;
        for event in &events {
            if let Said::Asked(Message::Observation(seen)) = &event.said
                && seen.phase() == Phase::Night
            {
                assert!(event.to.starts_with("wolf-"), "{} saw the night", event.to);
            }
        }
    }

    #[tokio::test]
    async fn a_game_plays_to_a_winner_and_tells_the_living() {
        // With one wolf among five, somebody dies the first night, so
        // fewer than six are told. The dead are never told and so never
        // shut themselves down; that the episode nonetheless finished,
        // which `play` asserts, shows the environment stopped them.
        let events = play(roles(1, 5), 3).await;
        let told = told(&events);
        assert!(!told.is_empty() && told.len() < 6, "{told:?}");
        let mut names: Vec<_> = told.iter().map(|(name, _)| name.clone()).collect();
        names.dedup();
        assert_eq!(names.len(), told.len(), "each living agent is told once");
        assert!(told.iter().all(|(_, team)| *team == told[0].1));
    }

    /// A villager who, whatever it is sent, sends the environment a
    /// `Select` naming its target, as a werewolf would in answer to the
    /// night, except unasked. It answers nothing itself.
    struct Rogue {
        target: PlayerId,
    }

    #[async_trait]
    impl Villager for Rogue {
        async fn day(
            &mut self,
            _observation: &State,
            context: &Context<Message>,
        ) -> Result<Option<PlayerId>> {
            let environment = HashSet::from([Some(NAME.to_string())]);
            // Sent and forgotten: awaiting the environment mid-game would
            // only wait out its patience.
            drop(context.request(&environment, Message::Select(self.target.clone()), None));
            Ok(None)
        }
    }

    #[tokio::test]
    async fn an_agent_cannot_kill_by_saying_so() {
        // One wolf and two rogue villagers who keep selecting it. The wolf
        // kills one of them the first night and reaches parity; if an
        // unasked `Select` counted, the villagers would win instead.
        let (log, events) = Log::new();
        let rogue = || -> Boxed {
            let rogue: Box<dyn Villager> = Box::new(Rogue {
                target: "wolf-1".to_string(),
            });
            Box::new(rogue)
        };
        let roles = roles(1, 2);
        let random = random(1, &roles);
        let environment: Boxed = Box::new(random.environment(State::new(roles)));
        let actors = [
            (
                "wolf-1".to_string(),
                random.agent(Role::Werewolf, &"wolf-1".to_string()),
            ),
            ("villager-1".to_string(), rogue()),
            ("villager-2".to_string(), rogue()),
            (NAME.to_string(), environment),
        ];
        let ending = episode(actors, None, Duration::from_secs(5), Some(log))
            .await
            .unwrap();
        assert_eq!(ending, Ending::Finished);
        let winners = announced(&drain(events).await);
        assert!(!winners.is_empty());
        assert!(
            winners.iter().all(|team| *team == Team::Werewolves),
            "{winners:?}"
        );
    }

    /// Everything said, as a sorted list of one-line summaries. Replies
    /// to one poll can land in the log in any order, and an observation
    /// prints its maps in any order, so neither is part of the summary.
    fn said(events: Vec<Event<Message>>) -> Vec<String> {
        let mut said: Vec<_> = events
            .into_iter()
            .map(|event| {
                let what = match event.said {
                    Said::Asked(Message::Observation(seen)) => {
                        format!("{:?} {}", seen.phase(), seen.round())
                    }
                    Said::Asked(other) => format!("{other:?}"),
                    Said::Replied(reply) => format!("{reply:?}"),
                };
                format!("{} -> {}: {what}", event.from, event.to)
            })
            .collect();
        said.sort();
        said
    }

    #[tokio::test]
    async fn the_same_seed_plays_the_same_game() {
        let first = said(play(roles(2, 6), 5).await);
        let second = said(play(roles(2, 6), 5).await);
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn different_seeds_play_different_games() {
        let first = said(play(roles(2, 6), 5).await);
        let second = said(play(roles(2, 6), 6).await);
        assert_ne!(first, second);
    }
}
