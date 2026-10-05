//! The actor that referees the game.

use crate::state::State;
use crate::{Message, Phase, PlayerId, Role, Team};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy, Recipient, Reply};
use rand::prelude::*;
use rand::rngs::StdRng;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// The environment's name as an actor.
pub const NAME: &str = "environment";

/// Runs one game from the first night to the announcement of the winner.
/// It alone holds the whole [`State`]; each player is sent only its own
/// observation.
pub struct Environment {
    state: State,
    rng: StdRng,
    /// How long to wait for any one player's answer.
    patience: Duration,
}

impl Environment {
    /// An environment for a game among players dealt these roles. Ties
    /// are broken by a random source seeded here.
    pub fn new(roles: HashMap<PlayerId, Role>, seed: u64, patience: Duration) -> Self {
        Environment {
            state: State::new(roles),
            rng: StdRng::seed_from_u64(seed),
            patience,
        }
    }

    /// The game as the environment knows it.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Everyone in the game, dead or alive, in a fixed order.
    fn everyone(&self) -> Vec<PlayerId> {
        let mut everyone: Vec<_> = self.state.roles().keys().cloned().collect();
        everyone.sort();
        everyone
    }

    /// Put the phase's question to some players, each seeing its own
    /// observation, and collect the valid choices that come back. A player
    /// that answers late, or names someone not among the candidates,
    /// abstains.
    async fn poll(
        &self,
        context: &Context<Message>,
        voters: &[PlayerId],
        question: fn(State) -> Message,
        candidates: impl Fn(&PlayerId, &PlayerId) -> bool,
    ) -> Result<Vec<PlayerId>> {
        let asked: Vec<_> = voters
            .iter()
            .map(|voter| {
                let seen = self.state.observation_for(voter);
                let to: HashSet<Recipient> = HashSet::from([Some(voter.clone())]);
                (
                    voter.clone(),
                    context.request(&to, question(seen), Some(self.patience)),
                )
            })
            .collect();
        let mut choices = Vec::new();
        for (voter, pending) in asked {
            for (_, reply) in pending.await? {
                let chosen = match reply {
                    Reply::Message(Message::Kill(id)) | Reply::Message(Message::Vote(id)) => id,
                    _ => continue,
                };
                if candidates(&voter, &chosen) {
                    choices.push(chosen);
                }
            }
        }
        Ok(choices)
    }

    /// The night: the living werewolves each name a victim among the
    /// living villagers, and the plurality dies.
    async fn night(&mut self, context: &Context<Message>) -> Result<()> {
        let werewolves = self.state.living_on(Team::Werewolves);
        let villagers = self.state.living_on(Team::Villagers);
        let votes = self
            .poll(context, &werewolves, Message::Night, |_, chosen| {
                villagers.contains(chosen)
            })
            .await?;
        if let Some(victim) = plurality(&votes, &mut self.rng) {
            self.state.kill(&victim);
        }
        Ok(())
    }

    /// The day: the living each vote for someone living other than
    /// themselves, and the plurality is eliminated.
    async fn day(&mut self, context: &Context<Message>) -> Result<()> {
        let living = self.state.living();
        let votes = self
            .poll(context, &living, Message::Day, |voter, chosen| {
                chosen != voter && living.contains(chosen)
            })
            .await?;
        if let Some(victim) = plurality(&votes, &mut self.rng) {
            self.state.kill(&victim);
        }
        Ok(())
    }
}

#[async_trait]
impl Policy for Environment {
    type Message = Message;

    /// The whole game, start to finish.
    async fn start(&mut self, context: &Context<Message>) -> Result<()> {
        let winner = loop {
            if let Some(winner) = self.state.winner() {
                break winner;
            }
            match self.state.phase() {
                Phase::Night => self.night(context).await?,
                Phase::Day => self.day(context).await?,
            }
            if let Some(winner) = self.state.winner() {
                break winner;
            }
            self.state.advance();
        };
        let everyone: HashSet<Recipient> = self.everyone().into_iter().map(Some).collect();
        context
            .request(&everyone, Message::Over(winner), Some(self.patience))
            .await?;
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

/// Everyone tied for the most votes, in a fixed order. No votes, no
/// leaders.
fn leaders(votes: &[PlayerId]) -> Vec<PlayerId> {
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
fn plurality(votes: &[PlayerId], rng: &mut impl Rng) -> Option<PlayerId> {
    leaders(votes).choose(rng).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;
    use crate::variant::random::Uniform;
    use free_agent::{Ending, Event, Log, Said, episode};
    use tokio::sync::mpsc::UnboundedReceiver;

    const PATIENCE: Duration = Duration::from_secs(5);

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

    /// Play a game of random players and return every event on the wire.
    async fn play(roles: HashMap<PlayerId, Role>, seed: u64) -> Vec<Event<Message>> {
        let (log, events) = Log::new();
        let mut seats: Vec<_> = roles.iter().collect();
        seats.sort_by(|a, b| a.0.cmp(b.0));
        let mut actors: Vec<(ActorId, Boxed)> = seats
            .into_iter()
            .enumerate()
            .map(|(n, (id, role))| {
                let player: Boxed = Box::new(Player::new(*role, Uniform::new(seed + n as u64)));
                (id.clone(), player)
            })
            .collect();
        let environment: Boxed = Box::new(Environment::new(roles, seed, PATIENCE));
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

    /// The side the environment announced as the winner, once per player
    /// told.
    fn announced(events: &[Event<Message>]) -> Vec<Team> {
        events
            .iter()
            .filter_map(|event| match &event.said {
                Said::Asked(Message::Over(team)) if event.from == NAME => Some(*team),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_game_with_no_werewolves_is_won_before_it_starts() {
        let events = play(roles(0, 4), 1).await;
        assert_eq!(announced(&events), vec![Team::Villagers; 4]);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.said, Said::Asked(Message::Night(_)))),
            "nobody should have been asked anything"
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
                Said::Replied(Reply::Message(Message::Kill(victim))) => Some(victim.clone()),
                _ => None,
            })
            .expect("the werewolf should have chosen a victim");
        assert!(first_kill.starts_with("villager-"), "{first_kill}");
        // The victim is never asked to vote.
        assert!(!events.iter().any(|event| {
            event.to == first_kill && matches!(event.said, Said::Asked(Message::Day(_)))
        }));
    }

    #[tokio::test]
    async fn a_game_plays_to_a_winner_and_tells_everyone() {
        let events = play(roles(1, 5), 3).await;
        let winners = announced(&events);
        assert_eq!(winners.len(), 6);
        assert!(winners.iter().all(|team| *team == winners[0]));
    }

    /// Everything said, as a sorted list of one-line summaries. Replies
    /// to one poll can land in the log in any order, and an observation
    /// prints its maps in any order, so neither is part of the summary.
    fn said(events: Vec<Event<Message>>) -> Vec<String> {
        let mut said: Vec<_> = events
            .into_iter()
            .map(|event| {
                let what = match event.said {
                    Said::Asked(Message::Night(seen)) => format!("night {}", seen.round()),
                    Said::Asked(Message::Day(seen)) => format!("day {}", seen.round()),
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
