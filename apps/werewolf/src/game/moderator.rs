//! The actor that runs the game.

use super::{Message, Outcome, Phase, PlayerId, Role, State, Team, leaders, plurality};
use anyhow::Result;
use async_trait::async_trait;
use free_agent::{ActorId, Context, Policy, Recipient, Reply};
use rand::prelude::*;
use rand::rngs::StdRng;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// The moderator's name as an actor.
pub const NAME: &str = "moderator";

/// How long the moderator lets things take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// How long the village may talk before the day ends regardless.
    pub day: Duration,
    /// How long to wait for any one player's answer.
    pub patience: Duration,
}

/// Runs one game from dealing the roles to announcing the winner, then
/// reports how it ended.
pub struct Moderator {
    state: State,
    rng: StdRng,
    settings: Settings,
    report: Option<oneshot::Sender<Outcome>>,
}

impl Moderator {
    /// A moderator for a game among `players`, `werewolves` of whom are
    /// dealt that role at random. The outcome is sent on `report` when the
    /// game is over.
    pub fn new(
        mut players: Vec<PlayerId>,
        werewolves: u8,
        seed: u64,
        settings: Settings,
        report: oneshot::Sender<Outcome>,
    ) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        players.sort();
        players.shuffle(&mut rng);
        let role = players
            .into_iter()
            .enumerate()
            .map(|(n, id)| {
                let role = if n < werewolves as usize {
                    Role::Werewolf
                } else {
                    Role::Villager
                };
                (id, role)
            })
            .collect();
        Moderator {
            state: State::new(role),
            rng,
            settings,
            report: Some(report),
        }
    }

    /// Everyone in the game, dead or alive, in a fixed order.
    fn everyone(&self) -> Vec<PlayerId> {
        let mut everyone: Vec<_> = self.state.role.keys().cloned().collect();
        everyone.sort();
        everyone
    }

    /// Tell every player what they are. The werewolves also learn one
    /// another.
    async fn deal(&self, context: &Context<Message>) -> Result<()> {
        let players = self.everyone();
        let werewolves = self.state.living_on(Team::Werewolves);
        let dealt: Vec<_> = self
            .state
            .role
            .iter()
            .map(|(id, role)| {
                let message = Message::YouAre {
                    role: *role,
                    players: players.clone(),
                    werewolves: match role.team() {
                        Team::Werewolves => werewolves.clone(),
                        Team::Villagers => vec![],
                    },
                };
                context.request(&recipients([id]), message, Some(self.settings.patience))
            })
            .collect();
        for dealing in dealt {
            dealing.await?;
        }
        Ok(())
    }

    /// Tell some players something, without wanting an answer.
    async fn tell(
        &self,
        context: &Context<Message>,
        to: &[PlayerId],
        message: Message,
    ) -> Result<()> {
        if to.is_empty() {
            return Ok(());
        }
        context
            .request(&recipients(to), message, Some(self.settings.patience))
            .await?;
        Ok(())
    }

    /// Put a question to some players and collect the valid choices that
    /// come back. A player who answers late, or names someone not among
    /// the candidates, abstains.
    async fn poll(
        &self,
        context: &Context<Message>,
        voters: &[PlayerId],
        question: Message,
        candidates: &[PlayerId],
    ) -> Result<Vec<PlayerId>> {
        if voters.is_empty() {
            return Ok(vec![]);
        }
        let replies = context
            .request(&recipients(voters), question, Some(self.settings.patience))
            .await?;
        let votes = replies
            .into_iter()
            .filter_map(|(_, reply)| match reply {
                Reply::Message(Message::Choice(id)) if candidates.contains(&id) => Some(id),
                _ => None,
            })
            .collect();
        Ok(votes)
    }

    /// The day: the living take turns speaking and nominating until
    /// everyone has nominated and one player leads, or time runs out.
    ///
    /// Each turn goes to the rest of the village as soon as it is taken.
    /// A nomination replaces the nominator's earlier one, so the tally at
    /// any moment is everyone's latest word. If time runs out, the
    /// current leader is eliminated, with a tie broken at random; with no
    /// nominations at all, nobody is.
    async fn day(&mut self, context: &Context<Message>) -> Result<()> {
        let deadline = Instant::now() + self.settings.day;
        let mut nominations: HashMap<PlayerId, PlayerId> = HashMap::new();
        let eliminated = loop {
            let candidates = self.state.living();
            let mut order = candidates.clone();
            order.shuffle(&mut self.rng);
            for speaker in &order {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let question = Message::Turn {
                    candidates: candidates.clone(),
                };
                let patience = self.settings.patience.min(remaining);
                let mut replies = context
                    .request(&recipients([speaker]), question, Some(patience))
                    .await?;
                let (said, nominated) = match replies.pop() {
                    Some((_, Reply::Message(Message::Statement { said, nominated }))) => {
                        (said, nominated)
                    }
                    _ => (None, None),
                };
                let nominated =
                    nominated.filter(|name| name != speaker && candidates.contains(name));
                if let Some(name) = &nominated {
                    nominations.insert(speaker.clone(), name.clone());
                }
                if said.is_some() || nominated.is_some() {
                    let audience: Vec<_> = candidates
                        .iter()
                        .filter(|name| *name != speaker)
                        .cloned()
                        .collect();
                    let heard = Message::Heard {
                        from: speaker.clone(),
                        said,
                        nominated,
                    };
                    self.tell(context, &audience, heard).await?;
                }
            }
            let votes: Vec<_> = nominations.values().cloned().collect();
            let everyone_nominated = candidates.iter().all(|name| nominations.contains_key(name));
            let leaders = leaders(&votes);
            if everyone_nominated && leaders.len() == 1 {
                break leaders.into_iter().next();
            }
            if Instant::now() >= deadline {
                break plurality(&votes, &mut self.rng);
            }
        };
        if let Some(victim) = eliminated {
            self.kill(context, &victim, Phase::Day).await?;
        }
        Ok(())
    }

    /// The night: the werewolves choose a villager to kill.
    async fn night(&mut self, context: &Context<Message>) -> Result<()> {
        let werewolves = self.state.living_on(Team::Werewolves);
        let villagers = self.state.living_on(Team::Villagers);
        let question = Message::Kill {
            candidates: villagers.clone(),
        };
        let votes = self
            .poll(context, &werewolves, question, &villagers)
            .await?;
        if let Some(victim) = plurality(&votes, &mut self.rng) {
            self.kill(context, &victim, Phase::Night).await?;
        }
        Ok(())
    }

    /// Take a player out of the game and tell everyone still in it, the
    /// victim included.
    async fn kill(
        &mut self,
        context: &Context<Message>,
        victim: &PlayerId,
        phase: Phase,
    ) -> Result<()> {
        let audience = self.state.living();
        self.state.kill(victim);
        let died = Message::Died {
            player: victim.clone(),
            phase,
        };
        self.tell(context, &audience, died).await
    }
}

#[async_trait]
impl Policy for Moderator {
    type Message = Message;

    /// The whole game, start to finish.
    async fn start(&mut self, context: &Context<Message>) -> Result<()> {
        self.deal(context).await?;
        let winner = loop {
            if let Some(winner) = self.state.winner() {
                break winner;
            }
            match self.state.turn.phase {
                Phase::Day => self.day(context).await?,
                Phase::Night => self.night(context).await?,
            }
            if let Some(winner) = self.state.winner() {
                break winner;
            }
            self.state.turn = self.state.turn.next();
        };

        let everyone = self.everyone();
        self.tell(context, &everyone, Message::GameOver { winner })
            .await?;

        let outcome = Outcome {
            winner,
            rounds: self.state.turn.round,
        };
        if let Some(report) = self.report.take() {
            // Nobody listening for the outcome is not the game's problem.
            let _ = report.send(outcome);
        }
        context.shutdown();
        Ok(())
    }

    /// Nobody asks the moderator anything.
    async fn reply(
        &mut self,
        _from: ActorId,
        _message: Message,
        _context: &Context<Message>,
    ) -> Result<Option<Message>> {
        Ok(None)
    }
}

/// A set of recipients from some player names.
fn recipients<'a>(names: impl IntoIterator<Item = &'a PlayerId>) -> HashSet<Recipient> {
    names.into_iter().cloned().map(Some).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use free_agent::{Event, Log, Said, episode};
    use tokio::time::sleep;

    const PATIENT: Settings = Settings {
        day: Duration::from_secs(300),
        patience: Duration::from_secs(60),
    };

    fn names(n: u8) -> Vec<PlayerId> {
        (1..=n).map(|n| format!("player-{n}")).collect()
    }

    fn roles(moderator: &Moderator) -> &HashMap<PlayerId, Role> {
        &moderator.state.role
    }

    fn moderator(players: Vec<PlayerId>, werewolves: u8, seed: u64) -> Moderator {
        let (report, _) = oneshot::channel();
        Moderator::new(players, werewolves, seed, PATIENT, report)
    }

    #[test]
    fn the_moderator_deals_exactly_as_many_werewolves_as_asked() {
        let moderator = moderator(names(10), 3, 7);

        let werewolves = roles(&moderator)
            .values()
            .filter(|role| **role == Role::Werewolf)
            .count();

        assert_eq!(werewolves, 3);
        assert_eq!(roles(&moderator).len(), 10);
    }

    #[test]
    fn the_same_seed_deals_the_same_roles() {
        let first = moderator(names(10), 3, 7);
        let again = moderator(names(10), 3, 7);

        assert_eq!(roles(&first), roles(&again));
    }

    #[test]
    fn different_seeds_deal_differently() {
        let first = moderator(names(10), 3, 7);
        let other = moderator(names(10), 3, 8);

        assert_ne!(roles(&first), roles(&other));
    }

    /// How a scripted player nominates on its turn.
    #[derive(Clone, Copy)]
    enum Plan {
        /// The first living player other than itself.
        FirstOther,
        /// The living player after itself, around the circle, so that a
        /// full round is a tie among everyone.
        Next,
        /// Nobody, ever.
        Nobody,
    }

    /// A player that follows a plan, taking a moment over each turn so
    /// that the clock moves.
    struct Scripted {
        plan: Plan,
        delay: Duration,
    }

    #[async_trait]
    impl Policy for Scripted {
        type Message = Message;

        async fn reply(
            &mut self,
            _from: ActorId,
            message: Message,
            context: &Context<Message>,
        ) -> Result<Option<Message>> {
            match message {
                Message::Turn { candidates } => {
                    sleep(self.delay).await;
                    let me = context.id();
                    let nominated = match self.plan {
                        Plan::FirstOther => candidates.iter().find(|name| *name != me).cloned(),
                        Plan::Next => candidates
                            .iter()
                            .position(|name| name == me)
                            .map(|at| candidates[(at + 1) % candidates.len()].clone()),
                        Plan::Nobody => None,
                    };
                    Ok(Some(Message::Statement {
                        said: Some(format!("{me} has spoken")),
                        nominated,
                    }))
                }
                Message::Kill { candidates } => {
                    Ok(candidates.first().cloned().map(Message::Choice))
                }
                Message::GameOver { .. } => {
                    context.shutdown();
                    Ok(None)
                }
                _ => Ok(None),
            }
        }
    }

    /// Play a game among four scripted players, one of them a werewolf,
    /// and return how it ended and everything that was said.
    async fn play(plan: Plan, day: Duration) -> (Outcome, Vec<Event<Message>>) {
        let players = vec![
            "ann".to_string(),
            "bob".to_string(),
            "cat".to_string(),
            "dan".to_string(),
        ];
        let (report, outcome) = oneshot::channel();
        let settings = Settings { day, ..PATIENT };
        let moderator = Moderator::new(players.clone(), 1, 1, settings, report);
        let mut actors: Vec<(ActorId, Box<dyn Policy<Message = Message> + Send>)> =
            vec![(NAME.to_string(), Box::new(moderator))];
        for name in players {
            let player = Scripted {
                plan,
                delay: Duration::from_secs(1),
            };
            actors.push((name, Box::new(player)));
        }
        let (log, mut events) = Log::new();
        let time_limit = Duration::from_secs(3600);
        episode(actors, None, time_limit, Some(log)).await.unwrap();
        let mut record = Vec::new();
        while let Ok(event) = events.try_recv() {
            record.push(event);
        }
        (outcome.await.unwrap(), record)
    }

    /// The deaths announced, each once, in order.
    fn deaths(record: &[Event<Message>]) -> Vec<(PlayerId, Phase)> {
        let mut seen = HashSet::new();
        record
            .iter()
            .filter_map(|event| match &event.said {
                Said::Asked(Message::Died { player, phase }) if seen.insert(event.request) => {
                    Some((player.clone(), *phase))
                }
                _ => None,
            })
            .collect()
    }

    /// How many turns were taken before the first death.
    fn turns_before_first_death(record: &[Event<Message>]) -> usize {
        record
            .iter()
            .take_while(|event| !matches!(event.said, Said::Asked(Message::Died { .. })))
            .filter(|event| matches!(event.said, Said::Asked(Message::Turn { .. })))
            .count()
    }

    #[tokio::test(start_paused = true)]
    async fn the_day_ends_once_everyone_has_nominated_and_someone_leads() {
        let (_, record) = play(Plan::FirstOther, PATIENT.day).await;

        // Ann nominates Bob; everyone else nominates Ann.
        assert_eq!(deaths(&record)[0], ("ann".to_string(), Phase::Day));
        assert_eq!(turns_before_first_death(&record), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_the_village_cannot_settle_ends_at_the_time_limit() {
        let (_, record) = play(Plan::Next, Duration::from_secs(10)).await;

        // Every round is a four-way tie, so the time limit decides.
        assert_eq!(deaths(&record)[0].1, Phase::Day);
        assert!(turns_before_first_death(&record) > 4, "{record:#?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_without_nominations_eliminates_nobody() {
        let (outcome, record) = play(Plan::Nobody, Duration::from_secs(10)).await;

        assert!(
            deaths(&record)
                .iter()
                .all(|(_, phase)| *phase == Phase::Night)
        );
        assert_eq!(outcome.winner, Team::Werewolves);
    }

    #[tokio::test(start_paused = true)]
    async fn the_village_hears_every_turn_but_the_speakers_own() {
        let (_, record) = play(Plan::FirstOther, PATIENT.day).await;

        let first_turn = record
            .iter()
            .find(|event| matches!(event.said, Said::Asked(Message::Turn { .. })))
            .unwrap();
        let speaker = &first_turn.to;
        let heard: Vec<_> = record
            .iter()
            .filter(|event| matches!(&event.said, Said::Asked(Message::Heard { from, .. }) if from == speaker))
            .map(|event| event.to.clone())
            .take(3)
            .collect();
        assert_eq!(heard.len(), 3);
        assert!(!heard.contains(speaker));
    }
}
