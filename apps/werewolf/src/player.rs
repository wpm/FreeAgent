//! The players: actors that are asked to act and name somebody.
//!
//! [`Random`] is the baseline. It chooses uniformly among the choices the
//! environment offered it, minus whatever its own memory rules out — a seer
//! does not read the same player twice, since the second reading would tell
//! it nothing. That memory lives in the actor, as every player's does; the
//! log is for training, not for a player to read back.

use crate::protocol::{Action, ActionSpace, Assignment, Knowledge, Message, Observation};
use crate::role::{Role, Team};
use anyhow::{Result, anyhow};
use free_agent::actor::{Actor, ActorId, async_trait};
use free_agent::episode::Channels;
use rand::prelude::*;
use rand::rngs::StdRng;
use std::collections::{HashMap, HashSet};

/// What a player carries from one turn to the next.
///
/// The agent's own state, in the reinforcement-learning sense: the game is
/// partially observable, so a policy sees an
/// [`Observation`] of this turn plus whatever it remembers here of the ones
/// before.
///
/// A role can only know the kind of thing its role knows, so each variant
/// carries exactly that and nothing else: a villager has no readings to
/// hold, and a werewolf has no way to acquire any. Everything here was told
/// to this player privately. Nothing is read back out of the log, which
/// belongs to whoever is training a model rather than to the players.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum State {
    /// Before the environment has said what this player is.
    #[default]
    Unassigned,
    /// A werewolf, who was told who its fellows are.
    Werewolf {
        /// The other werewolves.
        allies: HashSet<ActorId>,
    },
    /// A seer, who accumulates readings.
    Seer {
        /// What each player it has read turned out to be.
        readings: HashMap<ActorId, Team>,
    },
    /// A doctor, who learns nothing it must remember.
    Doctor,
    /// A plain villager, who learns nothing at all.
    Villager,
}

impl State {
    /// A freshly assigned player's memory.
    pub fn assigned(role: Role, allies: impl IntoIterator<Item = ActorId>) -> Self {
        match role {
            Role::Werewolf => Self::Werewolf {
                allies: allies.into_iter().collect(),
            },
            Role::Seer => Self::Seer {
                readings: HashMap::new(),
            },
            Role::Doctor => Self::Doctor,
            Role::Villager => Self::Villager,
        }
    }

    /// What this player is, once it has been told.
    pub fn role(&self) -> Option<Role> {
        match self {
            Self::Unassigned => None,
            Self::Werewolf { .. } => Some(Role::Werewolf),
            Self::Seer { .. } => Some(Role::Seer),
            Self::Doctor => Some(Role::Doctor),
            Self::Villager => Some(Role::Villager),
        }
    }

    /// Takes in a reading. Only a seer has anywhere to put one, so this is
    /// ignored by everyone else — the environment sends readings only to
    /// seers, and a reading arriving anywhere else is not this player's
    /// business.
    pub fn remember_reading(&mut self, player: ActorId, team: Team) {
        if let Self::Seer { readings } = self {
            readings.insert(player, team);
        }
    }

    /// Narrows an action space to the actions this player still considers
    /// worth taking.
    ///
    /// The environment has already applied the rules; this applies sense. A
    /// seer that has read someone learns nothing by reading them again, and
    /// a werewolf will not vote to lynch a fellow wolf. Every policy wants
    /// this, so it belongs to the state rather than to any one of them.
    ///
    /// If sense would rule everything out, the whole space comes back: a
    /// player with nothing sensible to do still acts.
    pub fn worth_considering<'a>(&self, space: &'a ActionSpace) -> Vec<&'a ActorId> {
        let worth_it: Vec<&ActorId> = space
            .targets
            .iter()
            .filter(|choice| match self {
                Self::Seer { readings } => !readings.contains_key(*choice),
                Self::Werewolf { allies } => !allies.contains(*choice),
                Self::Unassigned | Self::Doctor | Self::Villager => true,
            })
            .collect();
        if worth_it.is_empty() {
            space.targets.iter().collect()
        } else {
            worth_it
        }
    }
}

/// How an agent decides: an observation in, an action out.
///
/// The one thing that differs between players. Everything else about an
/// agent — watching its channels, folding what it hears into its
/// [`State`], answering within the deadline — is the same whoever is
/// deciding, so [`Agent`] does all of it and calls a policy for the
/// decision alone.
///
/// A policy takes `&mut self` because deciding may change the decider: a
/// random policy advances its generator, and a learning one would update
/// its weights.
///
/// The action space comes alongside the observation because an action must
/// be drawn from it — a policy that returns anything else is logged as
/// silent and does not act.
pub trait Policy: Send + 'static {
    /// Chooses what to do.
    ///
    /// `state` is what this agent remembers of earlier turns, which the
    /// observation does not repeat. Most policies will want
    /// [`State::worth_considering`] to narrow the space before choosing.
    ///
    /// # Errors
    ///
    /// Whatever deciding failed. An error here ends the agent and fails
    /// the episode, so a policy that cannot decide should pick arbitrarily
    /// rather than give up.
    fn decide(
        &mut self,
        state: &State,
        observation: &Observation,
        space: &ActionSpace,
    ) -> Result<Action>;
}

/// Chooses uniformly from the actions its state thinks worth considering.
///
/// The baseline every other policy is measured against. Seeding it makes an
/// episode replay exactly.
#[derive(Debug)]
pub struct Random {
    rng: StdRng,
}

impl Random {
    /// A policy whose choices follow from `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
        }
    }
}

impl Policy for Random {
    /// Ignores the observation entirely: the baseline knows nothing and
    /// uses nothing.
    fn decide(
        &mut self,
        state: &State,
        observation: &Observation,
        space: &ActionSpace,
    ) -> Result<Action> {
        let _ = observation;
        let worth_it = state.worth_considering(space);
        let chosen = worth_it
            .choose(&mut self.rng)
            .ok_or_else(|| anyhow!("asked to act with an empty action space"))?;
        Ok(Action::on((*chosen).clone()))
    }
}

/// A player: an agent that keeps a [`State`], answers the environment, and
/// decides with a [`Policy`].
///
/// Everything here is the same whoever is deciding — the policy is the only
/// part that differs between a random player, a heuristic one, and one
/// driven by a model.
#[derive(Debug)]
pub struct Agent<P: Policy> {
    state: State,
    policy: P,
}

impl<P: Policy> Agent<P> {
    /// An agent that has not been told anything yet, deciding with
    /// `policy`.
    pub fn new(policy: P) -> Self {
        Self {
            state: State::default(),
            policy,
        }
    }

    /// What this agent remembers.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Folds what the environment said into this agent's state.
    fn observe(&mut self, message: &Message) {
        match message {
            Message::Assigned(Assignment { role, allies, .. }) => {
                self.state = State::assigned(*role, allies.iter().cloned());
            }
            Message::Learned(Knowledge::Reading { player, team }) => {
                self.state.remember_reading(player.clone(), *team);
            }
            _ => {}
        }
    }

    /// Observes something the environment said in passing, and reports
    /// whether it was the last thing there is to hear.
    fn hear(&mut self, message: &Message) -> Game {
        self.observe(message);
        if matches!(message, Message::Ended(_)) {
            Game::Over
        } else {
            Game::On
        }
    }
}

/// Whether there is still a game to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Game {
    /// More may be said and asked.
    On,
    /// The environment has announced the outcome.
    Over,
}

#[async_trait]
impl<P: Policy> Actor<Message> for Agent<P> {
    /// Answers the environment's questions until the game ends.
    ///
    /// A request to decide arrives on `requests`, because the environment
    /// waits for the answer. Anything else — an assignment, a reading, the
    /// outcome — arrives on the inbox and is only observed.
    async fn perceive(&mut self, channels: &mut Channels<Message>) -> Result<()> {
        loop {
            tokio::select! {
                request = channels.requests.recv() => {
                    let Some(request) = request else {
                        return Ok(());
                    };
                    // What it was told and what it is asked travel
                    // separately, and the question may be noticed first.
                    // Whatever was said before it was asked is in the inbox
                    // by now, so take that in before deciding.
                    while let Ok(told) = channels.inbox.try_recv() {
                        if self.hear(&told) == Game::Over {
                            return Ok(());
                        }
                    }
                    let (asked, reply) = request.open();
                    self.observe(&asked);
                    let answer = match &asked {
                        Message::Decide(observation, space) => {
                            Message::Act(self.policy.decide(&self.state, observation, space)?)
                        }
                        // Nothing else expects an answer, but something
                        // must go back or the asker waits out its
                        // deadline.
                        other => other.clone(),
                    };
                    // An asker that has stopped waiting has already counted
                    // this player silent. Missing a deadline costs the turn,
                    // not the game, so a late answer is simply lost.
                    let _ = reply.send(answer);
                }
                message = channels.inbox.recv() => match message {
                    Some(message) => {
                        if self.hear(&message) == Game::Over {
                            return Ok(());
                        }
                    }
                    None => return Ok(()),
                },
                () = channels.stop.cancelled() => return Ok(()),
            }
        }
    }
}

/// An agent that decides at random, the baseline for an episode.
pub fn random(seed: u64) -> Agent<Random> {
    Agent::new(Random::seeded(seed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use free_agent::actor::Acting;
    use free_agent::episode::{Discard, episode};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn ids(names: &[&str]) -> Vec<ActorId> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn observation(living: Vec<ActorId>) -> Observation {
        Observation {
            round: 1,
            phase: crate::game::Phase::Night,
            living,
        }
    }

    #[test]
    fn a_player_remembers_what_it_is_told() {
        let mut player = Agent::new(Random::seeded(1));
        player.observe(&Message::Assigned(Assignment {
            role: Role::Werewolf,
            players: ids(&["Wolf1", "Wolf2", "Doc"]),
            allies: ids(&["Wolf2"]),
        }));
        assert_eq!(player.state().role(), Some(Role::Werewolf));
        assert_eq!(
            *player.state(),
            State::Werewolf {
                allies: ["Wolf2".to_string()].into_iter().collect()
            }
        );
    }

    /// The whole point of the seer's memory: a second reading of the same
    /// player would tell it nothing.
    #[test]
    fn a_seer_does_not_read_the_same_player_twice() {
        let mut state = State::assigned(Role::Seer, []);
        state.remember_reading("Wolf1".to_string(), Team::Werewolves);

        let choices = ids(&["Wolf1", "Doc", "Villager1"]);
        let worth_it: Vec<String> = state
            .worth_considering(&ActionSpace::naming(choices))
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(worth_it, ["Doc", "Villager1"]);
    }

    #[test]
    fn a_werewolf_does_not_vote_for_its_ally() {
        let state = State::assigned(Role::Werewolf, ["Wolf2".to_string()]);
        let choices = ids(&["Wolf2", "Doc"]);
        let worth_it: Vec<String> = state
            .worth_considering(&ActionSpace::naming(choices))
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(worth_it, ["Doc"]);
    }

    /// Sense must not paint a player into a corner: with nothing sensible
    /// left, anything legal will do.
    #[test]
    fn a_player_with_no_sensible_choice_falls_back_to_any() {
        let mut state = State::assigned(Role::Seer, []);
        state.remember_reading("Doc".to_string(), Team::Villagers);
        let choices = ids(&["Doc"]);
        assert_eq!(
            state.worth_considering(&ActionSpace::naming(choices)).len(),
            1
        );
    }

    /// A role that cannot know a thing has nowhere to keep it. The enum
    /// says so: there is no villager variant with readings in it.
    #[test]
    fn only_a_seer_keeps_readings() {
        for role in [Role::Werewolf, Role::Doctor, Role::Villager] {
            let mut state = State::assigned(role, []);
            state.remember_reading("Wolf1".to_string(), Team::Werewolves);
            // Nothing was kept, and the memory is still what the role is.
            assert_eq!(state.role(), Some(role));
            assert_eq!(state, State::assigned(role, []));
        }
    }

    /// Likewise allies: only a werewolf is told of any.
    #[test]
    fn only_a_werewolf_keeps_allies() {
        let allies = ["Wolf2".to_string()];
        assert_eq!(
            State::assigned(Role::Werewolf, allies.clone()),
            State::Werewolf {
                allies: allies.iter().cloned().collect()
            }
        );
        // The others ignore what they were handed, having nowhere to put it.
        for role in [Role::Doctor, Role::Villager, Role::Seer] {
            assert_eq!(
                State::assigned(role, allies.clone()),
                State::assigned(role, [])
            );
        }
    }

    #[test]
    fn a_player_knows_nothing_before_it_is_assigned() {
        let state = State::default();
        assert_eq!(state, State::Unassigned);
        assert_eq!(state.role(), None);
        // And rules nothing out, having no grounds to.
        let space = ActionSpace::naming(ids(&["A", "B"]));
        assert_eq!(state.worth_considering(&space).len(), 2);
    }

    #[test]
    fn a_random_player_names_someone_it_was_offered() {
        let mut policy = Random::seeded(7);
        let state = State::assigned(Role::Villager, []);
        let observation = observation(ids(&["Wolf1", "Doc", "Seer"]));
        let space = ActionSpace::naming(ids(&["Doc", "Seer"]));
        for _ in 0..20 {
            let action = policy.decide(&state, &observation, &space).unwrap();
            assert!(space.allows(&action), "{action:?}");
        }
    }

    #[test]
    fn the_same_seed_makes_the_same_choices() {
        let state = State::assigned(Role::Villager, []);
        let observation = observation(ids(&["A", "B", "C", "D"]));
        let space = ActionSpace::naming(ids(&["A", "B", "C", "D"]));
        let decisions = |seed| {
            let mut policy = Random::seeded(seed);
            (0..10)
                .map(|_| policy.decide(&state, &observation, &space).unwrap().target)
                .collect::<Vec<_>>()
        };
        assert_eq!(decisions(42), decisions(42));
    }

    /// Tells a seer what it read and, in the same breath, asks it to choose
    /// between the player it just read and one it has not. Does so over and
    /// over, with fresh players each time, and keeps the answers.
    struct Quizzes {
        seer: ActorId,
        times: usize,
        named: Arc<Mutex<Vec<ActorId>>>,
    }

    #[async_trait]
    impl Actor<Message> for Quizzes {
        async fn wake(&mut self, channels: &mut Channels<Message>) -> Result<Acting> {
            channels.send(
                Message::Assigned(Assignment {
                    role: Role::Seer,
                    players: ids(&["Seer"]),
                    allies: Vec::new(),
                }),
                &self.seer,
            )?;
            for n in 0..self.times {
                let (read, unread) = (format!("Read{n}"), format!("Unread{n}"));
                channels.send(
                    Message::Learned(Knowledge::Reading {
                        player: read.clone(),
                        team: Team::Villagers,
                    }),
                    &self.seer,
                )?;
                let living = vec![read, unread];
                let answer = channels
                    .request(
                        Message::Decide(observation(living.clone()), ActionSpace::naming(living)),
                        &self.seer,
                    )
                    .await?;
                if let Message::Act(action) = answer {
                    self.named.lock().unwrap().push(action.target);
                }
            }
            channels.stop(&self.seer)?;
            Ok(Acting::Done)
        }
    }

    /// What a player is told and what it is asked travel separately, and can
    /// be noticed in either order. A player answers on everything it was
    /// told before it was asked all the same — otherwise what it decides
    /// would turn on which arrival it happened to notice first, and a seeded
    /// game would not replay.
    #[tokio::test]
    async fn a_player_takes_in_what_it_was_told_before_it_answers() {
        let named = Arc::new(Mutex::new(Vec::new()));
        let roster: Vec<(&str, Box<dyn Actor<Message>>)> = vec![
            (
                "Environment",
                Box::new(Quizzes {
                    seer: "Seer".to_string(),
                    times: 50,
                    named: named.clone(),
                }),
            ),
            ("Seer", Box::new(random(7))),
        ];
        let result = episode(roster, Discard, Some(Duration::from_secs(5))).await;
        assert!(result.is_ok(), "{:?}", result);

        // Having just read one of the two, a seer always names the other.
        let expected: Vec<ActorId> = (0..50).map(|n| format!("Unread{n}")).collect();
        assert_eq!(*named.lock().unwrap(), expected);
    }

    /// Asks a player to decide and gives up before it has had a chance to
    /// answer, then asks again and waits.
    struct LosesPatience {
        player: ActorId,
        answered: Arc<Mutex<Option<Message>>>,
    }

    impl LosesPatience {
        fn question() -> Message {
            let living = ids(&["Doc", "Seer"]);
            Message::Decide(observation(living.clone()), ActionSpace::naming(living))
        }
    }

    #[async_trait]
    impl Actor<Message> for LosesPatience {
        async fn wake(&mut self, channels: &mut Channels<Message>) -> Result<Acting> {
            {
                // Polled once, so the question is on its way, and dropped
                // while the player has yet to run.
                let mut question = Box::pin(channels.request(Self::question(), &self.player));
                assert!(futures::poll!(&mut question).is_pending());
            }
            let answer = channels.request(Self::question(), &self.player).await?;
            *self.answered.lock().unwrap() = Some(answer);
            channels.stop(&self.player)?;
            Ok(Acting::Done)
        }
    }

    /// An answer that comes after the asker has stopped waiting is simply
    /// lost. The round went on without this player, which is all a missed
    /// deadline costs it: it is still in the game and answers the next
    /// question as usual.
    #[tokio::test]
    async fn a_player_whose_answer_comes_too_late_plays_on() {
        let answered = Arc::new(Mutex::new(None));
        let roster: Vec<(&str, Box<dyn Actor<Message>>)> = vec![
            (
                "Environment",
                Box::new(LosesPatience {
                    player: "Player".to_string(),
                    answered: answered.clone(),
                }),
            ),
            ("Player", Box::new(random(7))),
        ];
        let result = episode(roster, Discard, Some(Duration::from_secs(5))).await;
        assert!(result.is_ok(), "{:?}", result);
        assert!(
            matches!(*answered.lock().unwrap(), Some(Message::Act(_))),
            "{answered:?}"
        );
    }
}
