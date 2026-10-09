//! Werewolf as an episode of [free agents](free_agent): the rules in one
//! actor, the environment, and a player in each of the others.
//!
//! The environment alone holds the state of the game: the round and whether
//! it is night or day, every player's [`Role`], who is alive, and what the
//! seer has learned. A player sees the game only through the `Observation`
//! the environment sends it, which carries the round, the phase, who is
//! alive, and the roles that player may know. A werewolf knows every
//! werewolf, the seer knows itself and whoever it has discovered, and
//! everyone else knows only itself. Holding no state, a player can only
//! talk.
//!
//! Play alternates between night and day, starting with night, until one
//! [`Team`] has won. The environment logs the deal, every message to or
//! from a player as it was, and the result, as an [`Entry`] each.

#![warn(missing_docs)]
// The game is still being rebuilt on the framework, so much of it is not
// yet reached from anywhere.
#![allow(dead_code)]

pub mod report;
mod uniform_random;

use async_trait::async_trait;
use free_agent::{ActorId, ActorInit, Behavior, Builder, Context, Episode, Logger};
use futures_util::future::try_join_all;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZero;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;

/// A player is an actor, named as the episode names it.
pub type PlayerId = ActorId;

/// What the awake players chose in a phase: each player's choice of another.
type Choices = HashMap<PlayerId, PlayerId>;

/// The two sides. A game ends when one of them has won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// Kill by night and win on reaching parity.
    Werewolves,
    /// Vote by day and win when the last werewolf is gone.
    Villagers,
}

impl fmt::Display for Team {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Team::Werewolves => "werewolves",
            Team::Villagers => "villagers",
        })
    }
}

/// What a player is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub enum Role {
    /// Kills by night, and knows the other werewolves.
    Werewolf,
    /// Votes by day.
    Villager,
    /// Saves one player from the night's kill.
    Doctor,
    /// Learns one player's team each night.
    Seer,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Werewolf => "werewolf",
            Role::Villager => "villager",
            Role::Doctor => "doctor",
            Role::Seer => "seer",
        })
    }
}

impl Role {
    /// Which side this role wins with.
    pub fn team(self) -> Team {
        match self {
            Role::Werewolf => Team::Werewolves,
            Role::Villager | Role::Doctor | Role::Seer => Team::Villagers,
        }
    }
}

/// The two halves of a round. A game begins with night.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Phase {
    /// The werewolves choose a victim, the doctor someone to save, and the
    /// seer someone to learn about.
    Night,
    /// The village votes someone out.
    Day,
}

/// The state of a game, held by the environment alone.
struct State {
    /// Rounds count from one. A round is a night and then a day.
    round: NonZero<u8>,
    /// Which half of the round it is.
    phase: Phase,
    /// The roles known to whoever holds this state.
    roles: HashMap<PlayerId, Role>,
    /// Everyone still in the game.
    alive: HashSet<PlayerId>,
    /// The players whose team a seer has learned.
    seers_discovered: HashMap<PlayerId, HashSet<PlayerId>>,
}

impl State {
    /// A game of `roles`, at the first night with everyone alive.
    fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let alive: HashSet<PlayerId> = roles.keys().cloned().collect();
        let seers_discovered = roles
            .iter()
            .filter(|(_, role)| **role == Role::Seer)
            .map(|(player_id, _)| (player_id.clone(), HashSet::from([player_id.clone()])))
            .collect();
        Self {
            round: NonZero::new(1).unwrap(),
            phase: Phase::Night,
            roles,
            alive,
            seers_discovered,
        }
    }

    /// What `player_id` is allowed to see: the round, the phase, who is
    /// alive, and the roles it knows. A werewolf knows every werewolf, the
    /// seer knows itself and whoever it has discovered, and everyone else
    /// knows only itself.
    fn observation(&self, player_id: PlayerId) -> anyhow::Result<Observation> {
        let role = self.roles[&player_id];
        let roles = match role {
            Role::Werewolf => {
                // Werewolves know who all the other werewolves are.
                self.roles
                    .iter()
                    .filter(|(_, role)| **role == Role::Werewolf)
                    .map(|(id, role)| (id.clone(), *role))
                    .collect()
            }
            Role::Seer => {
                // Seers know their own role and that of anyone they have discovered.
                let mut roles = HashMap::from([(player_id.clone(), role)]);
                roles.extend(
                    self.seers_discovered[&player_id]
                        .iter()
                        .map(|id| (id.clone(), self.roles[id])),
                );
                roles
            }
            _ => {
                // Everyone else just knows their own role.
                HashMap::from([(player_id, role)])
            }
        };
        Ok(Observation {
            round: self.round,
            phase: self.phase.clone(),
            roles,
            alive: self.alive.clone(),
        })
    }

    /// Resolve the night from what the awake players chose. The werewolves'
    /// choices are ballots under `vote`, and whoever they elect dies unless
    /// a doctor chose to protect them. Each seer learns the role of whom it
    /// chose. Returns who died.
    fn resolve_night(&mut self, choices: &Choices, vote: &dyn Vote) -> Option<PlayerId> {
        for (seer, seen) in self.chosen_by(choices, Role::Seer) {
            self.seers_discovered.entry(seer).or_default().insert(seen);
        }
        let protected: HashSet<PlayerId> = self
            .chosen_by(choices, Role::Doctor)
            .into_values()
            .collect();
        let victim = vote.elect(&self.chosen_by(choices, Role::Werewolf))?;
        if protected.contains(&victim) {
            return None;
        }
        self.kill(&victim);
        Some(victim)
    }

    /// Resolve the day from what the awake players chose. Every choice is
    /// a ballot under `vote`, and whoever they elect dies. Returns who died.
    fn resolve_day(&mut self, choices: &Choices, vote: &dyn Vote) -> Option<PlayerId> {
        let victim = vote.elect(choices)?;
        self.kill(&victim);
        Some(victim)
    }

    /// The choices made by players of `role`.
    fn chosen_by(&self, choices: &Choices, role: Role) -> Choices {
        choices
            .iter()
            .filter(|(player, _)| self.roles.get(*player) == Some(&role))
            .map(|(player, chosen)| (player.clone(), chosen.clone()))
            .collect()
    }

    /// Remove `player` from the living.
    fn kill(&mut self, player: &PlayerId) {
        self.alive.remove(player);
    }

    /// How many days have been played through. A day counts once it is
    /// over, so the night of round `n` has seen `n - 1` and its day `n`.
    fn days(&self) -> u8 {
        match self.phase {
            Phase::Night => self.round.get() - 1,
            Phase::Day => self.round.get(),
        }
    }

    /// Move to the other half of the round: night turns to day, and day to
    /// the next round's night.
    fn next(&mut self) {
        self.phase = match self.phase {
            Phase::Night => Phase::Day,
            Phase::Day => {
                self.round = self.round.checked_add(1).expect("fewer than 255 rounds");
                Phase::Night
            }
        };
    }

    /// The players who act in the current phase: by night the werewolves,
    /// the doctor, and the seer; by day everyone alive.
    fn awake(&self) -> HashSet<PlayerId> {
        let acts = |role: Role| match self.phase {
            Phase::Night => matches!(role, Role::Werewolf | Role::Doctor | Role::Seer),
            Phase::Day => true,
        };
        self.alive
            .iter()
            .filter(|player_id| acts(self.roles[*player_id]))
            .cloned()
            .collect()
    }

    /// How many of `team` are alive?
    fn num_surviving_on(&self, team: Team) -> usize {
        self.alive
            .iter()
            .filter(|player_id| self.roles[*player_id].team() == team)
            .count()
    }

    /// Who has won, if anyone: the villagers when the last werewolf is
    /// dead, the werewolves when they are at least as many as the
    /// villagers.
    fn winner(&self) -> Option<Team> {
        let werewolves = self.num_surviving_on(Team::Werewolves);
        let villagers = self.num_surviving_on(Team::Villagers);
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }
}

/// How a game is played: the vote each phase is decided by and how long
/// each phase waits for the players.
pub struct Rules {
    /// The name of the variation played, for the log.
    pub variation: String,
    /// How the werewolves' choices become a victim.
    pub night_vote: Box<dyn Vote>,
    /// How the village's choices become a victim.
    pub day_vote: Box<dyn Vote>,
    /// How long the night waits for a player. A choice that comes later
    /// is ignored.
    pub night_limit: Duration,
    /// How long the day waits for a player. A choice that comes later is
    /// ignored.
    pub day_limit: Duration,
}

impl Default for Rules {
    /// Uniform Random: the werewolves break ties at random, the village
    /// does not, and each phase waits a minute.
    fn default() -> Self {
        Self {
            variation: "Uniform Random".to_string(),
            night_vote: Box::new(RandomTieBreak),
            day_vote: Box::new(NoTieBreak),
            night_limit: Duration::from_secs(60),
            day_limit: Duration::from_secs(60),
        }
    }
}

/// An episode of a game of `roles` under `rules`: the environment, which
/// may reach and stop every player and holds `logger`, and a player for
/// each role built by `player`. The winning team is sent on `winner` when
/// the game ends.
pub fn game(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    winner: oneshot::Sender<Team>,
    logger: Logger<Entry>,
    mut player: impl FnMut(&PlayerId) -> Builder<Actor>,
) -> Episode<Actor> {
    let players: HashSet<PlayerId> = roles.keys().cloned().collect();
    let mut init = HashMap::from([(
        "environment".to_string(),
        ActorInit {
            behavior: Actor::environment(roles, rules, winner),
            can_send_to: players.clone(),
            can_shut_down: players.clone(),
            has_logger: true,
        },
    )]);
    for id in players {
        let behavior = player(&id);
        init.insert(
            id,
            ActorInit {
                behavior,
                can_send_to: HashSet::new(),
                can_shut_down: HashSet::new(),
                has_logger: false,
            },
        );
    }
    Episode::new(init, logger)
}

/// What an actor in the game does: run it, or play in it. An episode holds
/// one kind of actor, so the two sides meet here and each method goes to
/// whichever side this is.
pub enum Actor {
    /// The side that holds the game.
    Environment(Box<Environment>),
    /// A side that sees only what it is shown.
    Player(uniform_random::Player),
    /// A player who never answers.
    #[cfg(test)]
    Mute(tests::Mute),
}

impl Actor {
    /// Builds the environment for a game of `roles` under `rules` once the
    /// episode has made its context. The winning team is sent on `winner`
    /// when the game ends.
    pub fn environment(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        winner: oneshot::Sender<Team>,
    ) -> Builder<Self> {
        Box::new(move |context| {
            Actor::Environment(Box::new(Environment {
                context,
                state: State::new(roles),
                rules,
                winner: Some(winner),
            }))
        })
    }

    /// Builds a player once the episode has made its context.
    pub fn player() -> Builder<Self> {
        Box::new(|context| Actor::Player(uniform_random::Player::new(context)))
    }
}

#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Player(player) => player.context(),
            #[cfg(test)]
            Actor::Mute(mute) => mute.context(),
        }
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.initialize().await,
            Actor::Player(player) => player.initialize().await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.initialize().await,
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.receive(message).await,
        }
    }

    async fn answer(&mut self, message: &Message) -> anyhow::Result<Vec<Message>> {
        match self {
            Actor::Environment(environment) => environment.answer(message).await,
            Actor::Player(player) => player.answer(message).await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.answer(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Player(player) => player.start().await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.start().await,
        }
    }

    async fn clean_up(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.clean_up().await,
            Actor::Player(player) => player.clean_up().await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.clean_up().await,
        }
    }
}

/// The actor that holds the game and tells each player what it may see.
pub struct Environment {
    context: Context<Message, Entry>,
    state: State,
    rules: Rules,
    /// Where the winning team goes when the game ends. Taken when sent.
    winner: Option<oneshot::Sender<Team>>,
}

impl Environment {
    /// The werewolves, the doctor, and the seer each choose, and the night
    /// is resolved from what they chose.
    async fn night(&mut self) -> anyhow::Result<()> {
        let choices = self.gather(self.rules.night_limit).await?;
        let dead = self
            .state
            .resolve_night(&choices, self.rules.night_vote.as_ref());
        self.bury(dead)
    }

    /// Everyone alive chooses, and the day is resolved from what they chose.
    async fn day(&mut self) -> anyhow::Result<()> {
        let choices = self.gather(self.rules.day_limit).await?;
        let dead = self
            .state
            .resolve_day(&choices, self.rules.day_vote.as_ref());
        self.bury(dead)
    }

    /// Ask everyone awake what they choose, all at once, and gather their
    /// choices by player, writing each one down. A player who has not
    /// answered within `limit` is left out.
    async fn gather(&self, limit: Duration) -> anyhow::Result<Choices> {
        let asked = self
            .state
            .awake()
            .into_iter()
            .map(|player| self.ask(player, limit));
        let replies = try_join_all(asked).await?;
        let mut choices = Choices::new();
        for (player, said) in replies.into_iter().flatten() {
            for message in said {
                if let Message::Action(chosen) = &message {
                    choices.insert(player.clone(), chosen.clone());
                }
                self.context.log(Entry::Replied {
                    from: player.clone(),
                    message,
                });
            }
        }
        Ok(choices)
    }

    /// Show `player` what it may see and wait up to `limit` for what it
    /// says back. Past the limit it is taken to have said nothing.
    async fn ask(
        &self,
        player: PlayerId,
        limit: Duration,
    ) -> anyhow::Result<HashMap<PlayerId, Vec<Message>>> {
        let message = Message::Observation(self.state.observation(player.clone())?);
        self.context.log(Entry::Sent {
            to: player.clone(),
            message: message.clone(),
        });
        let asked = self.context.request(message, HashSet::from([player]));
        match timeout(limit, asked).await {
            Ok(replied) => replied,
            Err(_) => Ok(HashMap::new()),
        }
    }

    /// Stop the actor of a player the state has just killed, so a death
    /// in the state and the end of the actor go together.
    fn bury(&self, dead: Option<PlayerId>) -> anyhow::Result<()> {
        match dead {
            Some(player) => self.context.stop(&player),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Behavior for Environment {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log the deal, play night and day in turn until a team has won, log
    /// the result, then stop the survivors, report the winner, and end the
    /// episode.
    async fn start(&mut self) -> anyhow::Result<()> {
        self.context.log(Entry::Start {
            variation: self.rules.variation.clone(),
            roles: self.state.roles.clone(),
        });
        let winner = loop {
            self.night().await?;
            if let Some(team) = self.state.winner() {
                break team;
            }
            self.state.next();
            self.day().await?;
            if let Some(team) = self.state.winner() {
                break team;
            }
            self.state.next();
        };
        self.context.log(Entry::End {
            winner,
            days: self.state.days(),
            survivors: self.state.alive.clone(),
        });
        for player in &self.state.alive {
            self.context.stop(player)?;
        }
        if let Some(report) = self.winner.take() {
            // The owner of the episode may have stopped listening.
            let _ = report.send(winner);
        }
        self.context.shutdown();
        Ok(())
    }
}

/// What the environment and the players say to one another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// What a player may see, from the environment.
    Observation(Observation),
    /// A player's choice of another player, to the environment.
    Action(PlayerId),
}
impl free_agent::Message for Message {}

/// What the environment writes to the log: the deal, every message to or
/// from a player as it was, and the result. Only the result summarizes
/// anything; the rest is kept whole for whatever reads the log later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    /// The game begins: who plays what, under which variation.
    Start {
        /// The name of the variation played, such as "Uniform Random".
        variation: String,
        /// Every player's role.
        roles: HashMap<PlayerId, Role>,
    },
    /// The environment sent `message` to `to`.
    Sent {
        /// Who was sent to.
        to: PlayerId,
        /// What was sent.
        message: Message,
    },
    /// `from` replied with `message`.
    Replied {
        /// Who replied.
        from: PlayerId,
        /// What they said.
        message: Message,
    },
    /// The game is over.
    End {
        /// Who won.
        winner: Team,
        /// How many days were played through.
        days: u8,
        /// Who was still alive.
        survivors: HashSet<PlayerId>,
    },
}

/// How a phase's ballots, each voter's choice of a player, become one
/// player or nobody. The werewolves and the village each vote under a rule
/// of their own, and a game may bring a rule of its own.
pub trait Vote: Send + Sync {
    /// Who the ballots elect, if anyone.
    fn elect(&self, ballots: &HashMap<PlayerId, PlayerId>) -> Option<PlayerId>;
}

/// The players with the most votes, in name order so a tie-break depends
/// on the random number alone. Empty when there are no ballots.
fn leaders(ballots: &HashMap<PlayerId, PlayerId>) -> Vec<PlayerId> {
    let mut votes: HashMap<&PlayerId, usize> = HashMap::new();
    for chosen in ballots.values() {
        *votes.entry(chosen).or_default() += 1;
    }
    let Some(most) = votes.values().copied().max() else {
        return vec![];
    };
    let mut leaders: Vec<PlayerId> = votes
        .into_iter()
        .filter(|(_, count)| *count == most)
        .map(|(player, _)| player.clone())
        .collect();
    leaders.sort();
    leaders
}

/// A plurality wins, and a tie is broken at random. How the werewolves
/// vote.
pub struct RandomTieBreak;

impl Vote for RandomTieBreak {
    fn elect(&self, ballots: &HashMap<PlayerId, PlayerId>) -> Option<PlayerId> {
        leaders(ballots).choose(&mut rand::rng()).cloned()
    }
}

/// A plurality wins, and a tie elects nobody. How the village votes.
pub struct NoTieBreak;

impl Vote for NoTieBreak {
    fn elect(&self, ballots: &HashMap<PlayerId, PlayerId>) -> Option<PlayerId> {
        match leaders(ballots).as_slice() {
            [leader] => Some(leader.clone()),
            _ => None,
        }
    }
}

/// What a player is shown of the game: everything in the state that it
/// may know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Rounds count from one. A round is a night and then a day.
    round: NonZero<u8>,
    /// Which half of the round it is.
    phase: Phase,
    /// The roles known to whoever holds this state.
    roles: HashMap<PlayerId, Role>,
    /// Everyone still in the game.
    alive: HashSet<PlayerId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    /// A player who never answers, for a game with a time limit to wait
    /// out.
    pub struct Mute {
        context: Context<Message, Entry>,
    }

    #[async_trait]
    impl Behavior for Mute {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.context
        }

        /// Clears its throat to itself, the one statement in a game of
        /// players who otherwise only answer, so a statement goes through
        /// [`Actor`] too. It does so while initializing, before anyone can
        /// ask it anything, so the statement is delivered before the
        /// request it never answers.
        async fn initialize(&mut self) -> anyhow::Result<()> {
            let me = self.context.id.clone();
            self.context
                .send(Message::Action(me.clone()), HashSet::from([me]))
        }

        async fn answer(&mut self, _: &Message) -> anyhow::Result<Vec<Message>> {
            std::future::pending().await
        }
    }

    impl Actor {
        fn mute() -> Builder<Self> {
            Box::new(|context| Actor::Mute(Mute { context }))
        }
    }

    /// Run a game of `roles` under `rules`, each player choosing at random
    /// except those in `mute`, who never answer. Returns the winner and
    /// everything the environment logged.
    async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        mute: &[&str],
    ) -> anyhow::Result<(Team, Vec<Entry>)> {
        let (winner, won) = oneshot::channel();
        let (logger, mut log) = unbounded_channel();
        let episode = game(roles, rules, winner, logger, |player| {
            if mute.contains(&player.as_str()) {
                Actor::mute()
            } else {
                Actor::player()
            }
        });

        episode.run(Duration::from_secs(60)).await?;

        let mut logged = Vec::new();
        while let Some(event) = log.recv().await {
            logged.push(event.payload);
        }
        Ok((won.await?, logged))
    }

    #[tokio::test]
    async fn a_game_is_logged_from_the_deal_to_the_end() {
        let roles = village().roles;
        let (winner, logged) = play(roles.clone(), Rules::default(), &[]).await.unwrap();

        assert_eq!(
            logged.first(),
            Some(&Entry::Start {
                variation: "Uniform Random".to_string(),
                roles: roles.clone(),
            })
        );

        let last = logged.last();
        assert!(
            matches!(
                last,
                Some(Entry::End { winner: won, survivors, .. })
                    if *won == winner
                        && !survivors.is_empty()
                        && survivors.iter().all(|player| roles.contains_key(player))
            ),
            "{last:?}"
        );

        // Everything between is a message to or from a player, as it was.
        let mut shown = 0;
        for entry in &logged[1..logged.len() - 1] {
            match entry {
                Entry::Sent {
                    to,
                    message: Message::Observation(_),
                } => {
                    assert!(roles.contains_key(to), "{to}");
                    shown += 1;
                }
                Entry::Replied {
                    from,
                    message: Message::Action(chosen),
                } => {
                    assert!(roles.contains_key(from), "{from}");
                    assert!(roles.contains_key(chosen), "{chosen}");
                }
                other => panic!("not a message to or from a player: {other:?}"),
            }
        }
        // By the first night three players are shown the game.
        assert!(shown >= 3, "{logged:?}");
    }

    #[test]
    fn an_entry_survives_a_trip_through_json() {
        let state = village();
        let entry = Entry::Sent {
            to: "seer".to_string(),
            message: Message::Observation(state.observation("seer".to_string()).unwrap()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn days_are_counted_as_they_are_completed() {
        let mut state = village();
        assert_eq!(state.days(), 0);
        state.next();
        assert_eq!(state.days(), 1);
        state.next();
        assert_eq!(state.days(), 1);
        state.next();
        assert_eq!(state.days(), 2);
    }

    fn one_wolf_against(villagers: &[&str]) -> HashMap<PlayerId, Role> {
        let mut roles = HashMap::from([("wolf".to_string(), Role::Werewolf)]);
        for villager in villagers {
            roles.insert(villager.to_string(), Role::Villager);
        }
        roles
    }

    #[tokio::test]
    async fn the_werewolves_win_on_reaching_parity() {
        // One werewolf against two: whoever it kills the first night, the
        // werewolf then equals the village.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, _) = play(roles, Rules::default(), &[]).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_who_misses_the_time_limit_is_left_out() {
        // The werewolf never answers, so no night kills anyone, and the
        // villagers vote by day until the game ends one way or the other.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let rules = Rules {
            night_limit: Duration::from_secs(1),
            day_limit: Duration::from_secs(1),
            ..Rules::default()
        };
        let (winner, _) = play(roles, rules, &["wolf"]).await.unwrap();
        assert!(matches!(winner, Team::Werewolves | Team::Villagers));
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_time_limit_a_silent_player_holds_up_the_game() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let rules = Rules {
            night_limit: Duration::from_secs(120),
            ..Rules::default()
        };
        let error = play(roles, rules, &["wolf"]).await.unwrap_err();
        assert!(error.to_string().contains("patience"), "{error}");
    }

    fn village() -> State {
        State::new(HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
            ("seer".to_string(), Role::Seer),
            ("villager".to_string(), Role::Villager),
        ]))
    }

    fn seen_by(observer: &str) -> Vec<String> {
        let observation = village().observation(observer.to_string()).unwrap();
        let mut seen: Vec<_> = observation.roles.keys().cloned().collect();
        seen.sort();
        seen
    }

    #[test]
    fn werewolves_see_each_other_and_nobody_else() {
        assert_eq!(seen_by("wolf1"), ["wolf1", "wolf2"]);
    }

    #[test]
    fn villagers_see_only_themselves() {
        assert_eq!(seen_by("villager"), ["villager"]);
    }

    #[test]
    fn the_seer_sees_only_itself_until_it_has_discovered_someone() {
        assert_eq!(seen_by("seer"), ["seer"]);
        let mut state = village();
        state
            .seers_discovered
            .get_mut("seer")
            .unwrap()
            .insert("wolf1".to_string());
        let observation = state.observation("seer".to_string()).unwrap();
        let mut seen: Vec<_> = observation.roles.keys().cloned().collect();
        seen.sort();
        assert_eq!(seen, ["seer", "wolf1"]);
        assert_eq!(observation.roles["wolf1"], Role::Werewolf);
    }

    #[test]
    fn observation_keeps_the_rest_of_the_state() {
        let state = village();
        let observation = state.observation("villager".to_string()).unwrap();
        assert_eq!(observation.round, state.round);
        assert_eq!(observation.alive, state.alive);
    }

    fn sorted(players: HashSet<PlayerId>) -> Vec<PlayerId> {
        let mut players: Vec<_> = players.into_iter().collect();
        players.sort();
        players
    }

    #[test]
    fn by_night_the_werewolves_doctor_and_seer_are_awake() {
        let mut state = State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("doctor".to_string(), Role::Doctor),
            ("seer".to_string(), Role::Seer),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]));
        state.alive.remove("bob");
        assert_eq!(sorted(state.awake()), ["doctor", "seer", "wolf"]);
    }

    #[test]
    fn by_day_everyone_alive_is_awake() {
        let mut state = village();
        state.phase = Phase::Day;
        state.alive.remove("wolf2");
        assert_eq!(sorted(state.awake()), ["seer", "villager", "wolf1"]);
    }

    #[test]
    fn night_turns_to_day_and_day_to_the_next_nights() {
        let mut state = village();
        assert_eq!(state.round.get(), 1);
        assert!(matches!(state.phase, Phase::Night));
        state.next();
        assert_eq!(state.round.get(), 1);
        assert!(matches!(state.phase, Phase::Day));
        state.next();
        assert_eq!(state.round.get(), 2);
        assert!(matches!(state.phase, Phase::Night));
    }

    /// Ballots in which each voter chooses the player beside its name.
    fn ballots(votes: &[(&str, &str)]) -> HashMap<PlayerId, PlayerId> {
        votes
            .iter()
            .map(|(voter, chosen)| (voter.to_string(), chosen.to_string()))
            .collect()
    }

    #[test]
    fn a_plurality_is_elected_under_either_rule() {
        let votes = ballots(&[("ann", "bob"), ("bob", "cat"), ("cat", "bob")]);
        assert_eq!(RandomTieBreak.elect(&votes), Some("bob".to_string()));
        assert_eq!(NoTieBreak.elect(&votes), Some("bob".to_string()));
    }

    #[test]
    fn nobody_is_elected_without_ballots() {
        assert_eq!(RandomTieBreak.elect(&HashMap::new()), None);
        assert_eq!(NoTieBreak.elect(&HashMap::new()), None);
    }

    #[test]
    fn a_tie_elects_nobody_without_a_tie_break() {
        let votes = ballots(&[("ann", "bob"), ("bob", "ann"), ("cat", "dan")]);
        assert_eq!(NoTieBreak.elect(&votes), None);
    }

    #[test]
    fn a_tie_is_broken_at_random_among_the_tied() {
        let votes = ballots(&[
            ("ann", "bob"),
            ("bob", "ann"),
            ("cat", "ann"),
            ("dan", "bob"),
        ]);
        let mut elected = HashSet::new();
        for _ in 0..50 {
            elected.insert(RandomTieBreak.elect(&votes).unwrap());
        }
        assert_eq!(
            elected,
            HashSet::from(["ann".to_string(), "bob".to_string()])
        );
    }

    /// A village with a doctor, for the night to have everyone in it.
    fn full_village() -> State {
        State::new(HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
            ("seer".to_string(), Role::Seer),
            ("doctor".to_string(), Role::Doctor),
            ("villager".to_string(), Role::Villager),
        ]))
    }

    #[test]
    fn by_night_the_werewolves_kill_whom_they_elect() {
        let mut state = full_village();
        let choices = ballots(&[
            ("wolf1", "villager"),
            ("wolf2", "villager"),
            ("doctor", "seer"),
        ]);
        assert_eq!(
            state.resolve_night(&choices, &RandomTieBreak),
            Some("villager".to_string())
        );
        assert!(!state.alive.contains("villager"));
    }

    #[test]
    fn the_doctor_saves_whom_it_chooses() {
        let mut state = full_village();
        let choices = ballots(&[
            ("wolf1", "villager"),
            ("wolf2", "villager"),
            ("doctor", "villager"),
        ]);
        assert_eq!(state.resolve_night(&choices, &RandomTieBreak), None);
        assert!(state.alive.contains("villager"));
    }

    #[test]
    fn the_seer_learns_the_role_of_whom_it_chooses() {
        let mut state = full_village();
        let choices = ballots(&[("wolf1", "villager"), ("seer", "wolf1")]);
        state.resolve_night(&choices, &RandomTieBreak);
        let observation = state.observation("seer".to_string()).unwrap();
        assert_eq!(observation.roles["wolf1"], Role::Werewolf);
    }

    #[test]
    fn a_night_without_a_werewolf_choice_kills_nobody() {
        let mut state = full_village();
        let choices = ballots(&[("seer", "wolf1"), ("doctor", "seer")]);
        assert_eq!(state.resolve_night(&choices, &RandomTieBreak), None);
        assert_eq!(state.alive.len(), 5);
    }

    #[test]
    fn by_day_the_village_kills_whom_it_elects() {
        let mut state = full_village();
        state.next();
        let choices = ballots(&[
            ("wolf1", "seer"),
            ("wolf2", "seer"),
            ("seer", "wolf1"),
            ("doctor", "wolf1"),
            ("villager", "wolf1"),
        ]);
        assert_eq!(
            state.resolve_day(&choices, &NoTieBreak),
            Some("wolf1".to_string())
        );
        assert!(!state.alive.contains("wolf1"));
    }

    #[test]
    fn a_day_that_ties_kills_nobody() {
        let mut state = full_village();
        state.next();
        let choices = ballots(&[("wolf1", "seer"), ("seer", "wolf1")]);
        assert_eq!(state.resolve_day(&choices, &NoTieBreak), None);
        assert_eq!(state.alive.len(), 5);
    }

    #[test]
    fn a_role_is_named_in_prose() {
        assert_eq!(Role::Werewolf.to_string(), "werewolf");
        assert_eq!(Role::Villager.to_string(), "villager");
        assert_eq!(Role::Doctor.to_string(), "doctor");
        assert_eq!(Role::Seer.to_string(), "seer");
    }

    #[test]
    fn a_team_is_named_in_prose() {
        assert_eq!(Team::Werewolves.to_string(), "werewolves");
        assert_eq!(Team::Villagers.to_string(), "villagers");
    }

    #[test]
    fn werewolves_win_at_parity_and_villagers_when_the_wolves_are_gone() {
        let mut state = State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]));
        assert_eq!(state.winner(), None);
        state.alive.remove("ann");
        assert_eq!(state.winner(), Some(Team::Werewolves));

        let mut state = village();
        state.alive.remove("wolf1");
        state.alive.remove("wolf2");
        assert_eq!(state.winner(), Some(Team::Villagers));
    }
}
