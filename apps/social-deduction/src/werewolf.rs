//! What every variant of Werewolf shares: the rules and the types they are
//! written in.
//!
//! A game is an episode of [free agents](free_agent): the rules in one
//! actor, the environment, and a player in each of the others. The
//! environment alone holds the state of the game: the round and whether
//! it is night or day, every player's [`Role`], who is alive, and what the
//! seer has learned. A player sees the game only through the
//! [`View`] the environment sends it, which carries the round, the
//! phase, who is alive, and the roles that player may know. A werewolf knows
//! every werewolf, the seer knows itself and whoever it has discovered, and
//! everyone else knows only itself. Holding no state, a player can only
//! talk.
//!
//! Play alternates between night and day, starting with night, until one
//! [`Team`] has won. The log is written by every actor, an [`Entry`] at a
//! time, in the language of reinforcement learning: each message an actor
//! sends to others is an action of its sender, which the sender logs
//! whole, once, and an observation of each receiver, which the receiver
//! logs as a receipt naming the message by its sender and number. The
//! environment logs the deal, its announcements, the selections it
//! receives, each night's hunt, and the result; every player logs what it
//! receives and the selections and utterances it sends.
//!
//! Each variant's module holds what is particular to it: its environment,
//! its players, the actor enum its episode holds, and the function that
//! builds its episode.

pub mod announced;
pub mod llm;
pub mod report;
pub mod uniform_random;

use clap::Args;
use free_agent::{ActorId, ActorInit, Behavior, Builder, Context, Episode, Logger, ThinkBuilder};
use rand::seq::{IndexedRandom, SliceRandom};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZero;
use std::time::Duration;

/// A player is an actor, named as the episode names it.
pub type PlayerId = ActorId;

/// The environment's name in the episode.
pub const ENVIRONMENT: &str = "environment";

/// An episode of a game: `environment`, which runs it, and a player for
/// each of `players` built by `player`. Everyone holds `logger`. A player
/// may send to the environment alone, stops nobody, and does not think.
pub fn episode<A>(
    environment: ActorInit<A>,
    players: impl IntoIterator<Item = PlayerId>,
    mut player: impl FnMut(&PlayerId) -> Builder<A>,
    logger: Logger<Entry>,
) -> Episode<A>
where
    A: Behavior<Message = Message, Log = Entry>,
{
    thinking_episode(environment, players, |id| (player(id), None), logger)
}

/// The episode of [`episode`], whose players may think: `player` builds
/// each player's behavior and its think loop, if it has one. Everyone
/// holds `logger`. A player may send to the environment and to every other
/// player, and stops nobody.
pub fn thinking_episode<A>(
    environment: ActorInit<A>,
    players: impl IntoIterator<Item = PlayerId>,
    mut player: impl FnMut(&PlayerId) -> (Builder<A>, Option<ThinkBuilder<Message, Entry>>),
    logger: Logger<Entry>,
) -> Episode<A>
where
    A: Behavior<Message = Message, Log = Entry>,
{
    let players: Vec<PlayerId> = players.into_iter().collect();
    let mut init = HashMap::from([(ENVIRONMENT.to_string(), environment)]);
    for id in &players {
        let (behavior, think) = player(id);
        init.insert(
            id.clone(),
            ActorInit {
                behavior,
                think,
                can_send_to: to_everyone_but(id, &players),
                can_shut_down: HashSet::new(),
                has_logger: true,
            },
        );
    }
    Episode::new(init, logger)
}

/// What the awake players chose in a phase: each player's choice of another.
type Choices = HashMap<PlayerId, PlayerId>;

/// How a night's hunt came out: the werewolves elected `victim`, who died
/// unless a doctor protected them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunt {
    /// Whom the werewolves elected.
    pub victim: PlayerId,
    /// The doctors who chose to protect the victim.
    pub protectors: HashSet<PlayerId>,
}

impl Hunt {
    /// Who died: the victim, unless protected.
    pub fn dead(&self) -> Option<&PlayerId> {
        self.protectors.is_empty().then_some(&self.victim)
    }
}

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

/// What a player is. The roles are ordered as they are dealt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash)]
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

/// How many of each role sit at the table, for a variant to flatten into its
/// command line arguments. A count left unset has no `clap` default, so
/// that a variant can tell it apart from a count that was given, and fill
/// it in from [`Table::default`] or from somewhere else.
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct RoleCounts {
    /// How many werewolves.
    #[arg(long, help = how_many("werewolves", Table::default().werewolves))]
    pub werewolves: Option<usize>,
    /// How many plain villagers.
    #[arg(long, help = how_many("plain villagers", Table::default().villagers))]
    pub villagers: Option<usize>,
    /// How many doctors.
    #[arg(long, help = how_many("doctors", Table::default().doctors))]
    pub doctors: Option<usize>,
    /// How many seers.
    #[arg(long, help = how_many("seers", Table::default().seers))]
    pub seers: Option<usize>,
}

/// The help for a count of `what`, naming the `default` the code fills in
/// when the count is left unset.
fn how_many(what: &str, default: usize) -> String {
    format!("How many {what} [default: {default}]")
}

/// How long each phase waits for the players, for a variant to flatten into
/// its command line arguments. A limit left unset has no `clap` default, so
/// that a variant can tell it apart from a limit that was given, and fill it
/// in from [`Rules::default`] or from somewhere else.
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct PhaseLimits {
    /// How long the night waits for a player.
    #[arg(long, value_parser = humantime::parse_duration, help = how_long("night", Rules::default().night_limit))]
    pub night_limit: Option<Duration>,
    /// How long the day waits for a player.
    #[arg(long, value_parser = humantime::parse_duration, help = how_long("day", Rules::default().day_limit))]
    pub day_limit: Option<Duration>,
}

/// The help for the limit of `phase`, naming the `default` the code fills
/// in when the limit is left unset.
fn how_long(phase: &str, default: Duration) -> String {
    format!(
        "How long the {phase} waits for a player [default: {}]",
        humantime::format_duration(default)
    )
}

impl PhaseLimits {
    /// `rules`, with each limit given here in place of its own.
    pub fn or(&self, rules: Rules) -> Rules {
        Rules {
            night_limit: self.night_limit.unwrap_or(rules.night_limit),
            day_limit: self.day_limit.unwrap_or(rules.day_limit),
            ..rules
        }
    }
}

impl RoleCounts {
    /// The table these counts seat, with any count left unset taken from
    /// `default`.
    pub fn or(&self, default: Table) -> Table {
        Table {
            werewolves: self.werewolves.unwrap_or(default.werewolves),
            villagers: self.villagers.unwrap_or(default.villagers),
            doctors: self.doctors.unwrap_or(default.doctors),
            seers: self.seers.unwrap_or(default.seers),
        }
    }
}

/// How many of each role sit at the table.
#[derive(Debug, PartialEq, Eq)]
pub struct Table {
    /// How many werewolves.
    pub werewolves: usize,
    /// How many plain villagers.
    pub villagers: usize,
    /// How many doctors.
    pub doctors: usize,
    /// How many seers.
    pub seers: usize,
}

impl Default for Table {
    /// Two werewolves, three villagers, a doctor and a seer: seven players.
    fn default() -> Self {
        Table {
            werewolves: 2,
            villagers: 3,
            doctors: 1,
            seers: 1,
        }
    }
}

impl Table {
    /// Each role and how many of it, in the order the roles are dealt.
    pub fn counts(&self) -> [(Role, usize); 4] {
        [
            (Role::Werewolf, self.werewolves),
            (Role::Villager, self.villagers),
            (Role::Doctor, self.doctors),
            (Role::Seer, self.seers),
        ]
    }

    /// The seats, named `player1` onward, one per player. The names say
    /// nothing about the roles, since every player sees every name.
    pub fn seats(&self) -> impl Iterator<Item = PlayerId> {
        let players: usize = self.counts().iter().map(|(_, count)| count).sum();
        (1..=players).map(|seat| format!("player{seat}"))
    }

    /// Deal the roles at random to the seats.
    pub fn deal(&self) -> HashMap<PlayerId, Role> {
        let mut roles = Vec::new();
        for (role, count) in self.counts() {
            roles.extend(std::iter::repeat_n(role, count));
        }
        roles.shuffle(&mut rand::rng());
        self.seats().zip(roles).collect()
    }
}

impl fmt::Display for Table {
    /// The counts in the order the roles are dealt, each in its number:
    /// `2 werewolves, 3 villagers, 1 doctor, 1 seer`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let counted = self.counts().map(|(role, count)| match (count, role) {
            (1, _) => format!("1 {role}"),
            (_, Role::Werewolf) => format!("{count} werewolves"),
            _ => format!("{count} {role}s"),
        });
        f.write_str(&counted.join(", "))
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

impl Phase {
    /// Whether a player of `role`, if known, acts in this phase: by night
    /// the werewolves, the doctor and the seer do, and by day everyone. The
    /// one rule of who is awake, so that the environment and the players
    /// cannot disagree about who sleeps.
    fn wakes(&self, role: Option<Role>) -> bool {
        match self {
            Phase::Night => matches!(role, Some(Role::Werewolf | Role::Doctor | Role::Seer)),
            Phase::Day => true,
        }
    }
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
    fn view(&self, player_id: PlayerId) -> anyhow::Result<View> {
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
        Ok(View {
            round: self.round,
            phase: self.phase.clone(),
            roles,
            alive: self.alive.clone(),
        })
    }

    /// Resolve the night from what the awake players chose. The werewolves'
    /// choices are ballots under `vote`, and whoever they elect dies unless
    /// a doctor chose to protect them. Each seer learns the role of whom it
    /// chose. Returns the hunt, if the werewolves elected anyone.
    fn resolve_night(&mut self, choices: &Choices, vote: &dyn Vote) -> Option<Hunt> {
        for (seer, seen) in self.chosen_by(choices, Role::Seer) {
            self.seers_discovered.entry(seer).or_default().insert(seen);
        }
        let victim = vote.elect(&self.chosen_by(choices, Role::Werewolf))?;
        let protectors = self
            .chosen_by(choices, Role::Doctor)
            .into_iter()
            .filter(|(_, protected)| *protected == victim)
            .map(|(doctor, _)| doctor)
            .collect();
        let hunt = Hunt { victim, protectors };
        if let Some(dead) = hunt.dead().cloned() {
            self.kill(&dead);
        }
        Some(hunt)
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

    /// The living players who act in the current phase, as
    /// [`Phase::wakes`] has it: by night the werewolves, the doctor, and the
    /// seer; by day everyone alive.
    fn awake(&self) -> HashSet<PlayerId> {
        self.alive
            .iter()
            .filter(|player_id| self.phase.wakes(Some(self.roles[*player_id])))
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

/// How a game is played: the vote each phase is decided by, how long each
/// phase waits for the players, and whether the day's vote is ratified.
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
    /// How long the day's votes must stand for the player they select to
    /// be voted out. With a ratify limit, a player may change its vote
    /// while the day is open, its latest vote counts, and the day ends one
    /// ratify limit after the votes stop moving, or at the day limit with
    /// nobody voted out. Without one, the first vote counts and the day
    /// ends when everyone has voted or at its limit.
    pub ratify_limit: Option<Duration>,
}

impl Rules {
    /// The entry that opens a game of `roles` under these rules: the
    /// variation, the deal, and the limits.
    pub fn start(&self, roles: HashMap<PlayerId, Role>) -> Entry {
        Entry::Start {
            variation: self.variation.clone(),
            roles,
            night_limit: self.night_limit,
            day_limit: self.day_limit,
            ratify_limit: self.ratify_limit,
        }
    }
}

impl Default for Rules {
    /// Uniform Random: the werewolves break ties at random, the village
    /// does not, each phase waits a minute, and the day's vote is not
    /// ratified.
    fn default() -> Self {
        Self {
            variation: "Uniform Random".to_string(),
            night_vote: Box::new(RandomTieBreak),
            day_vote: Box::new(NoTieBreak),
            night_limit: Duration::from_secs(60),
            day_limit: Duration::from_secs(60),
            ratify_limit: None,
        }
    }
}

/// What the environment and the players say to one another. Every message
/// an actor sends to others is numbered by its sender, from 1, so that the
/// pair of sender and number names a message in an episode: the
/// environment numbers its announcements and tentative decisions, each
/// player its selections and utterances together. A message to several
/// actors has one number, but each announcement is a message of its own,
/// since each player is shown a different view. A reminder an actor sends
/// itself is not numbered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// The environment opens a phase for one living player, awake in it
    /// or not. Phases are numbered from 1 for the first night, one up for
    /// each phase.
    Announce {
        /// The number of the phase.
        seq: u64,
        /// The environment's number for this message.
        n: u64,
        /// What the player may see of it.
        view: View,
        /// How long the phase lasts, written as `humantime` writes it. A
        /// player holds no state but its history, and can see when its
        /// messages arrived, so the limit is all it needs to know the time.
        #[serde(with = "humantime_serde")]
        limit: Duration,
    },
    /// A player's selection for the phase numbered `seq`. By night it goes
    /// to the environment alone; by day, when the vote is ratified, to every
    /// other living player as well.
    Select {
        /// The number of the phase.
        seq: u64,
        /// Who selected, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// Whom they selected.
        target: PlayerId,
    },
    /// A player's retraction of its vote in the day numbered `seq`: it is
    /// undecided again, as it was before it first voted. By day it goes to
    /// the environment and every other living player.
    Retract {
        /// The number of the phase.
        seq: u64,
        /// Who retracted, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
    },
    /// The environment's tentative decision for the day numbered `seq`,
    /// sent to every living player whenever it changes: the player the
    /// votes select, or nobody, and how long the votes must stand for it to
    /// be ratified.
    Tentative {
        /// The number of the day.
        seq: u64,
        /// The environment's number for this message.
        n: u64,
        /// The player the votes select, if they select anyone.
        target: Option<PlayerId>,
        /// How long the votes must stand, written as `humantime` writes it.
        #[serde(with = "humantime_serde")]
        ratify: Duration,
    },
    /// A player's utterance in the phase numbered `seq`, as the speaker knew
    /// it: the latest phase the speaker was announced. By day it goes to
    /// every other living player, and never to the environment, which does
    /// nothing with speech.
    Say {
        /// The number of the phase.
        seq: u64,
        /// Who spoke, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// What they said.
        text: String,
    },
    /// The environment's own message to itself that the phase numbered
    /// `seq` is over.
    EndPhase {
        /// The number of the phase.
        seq: u64,
    },
    /// The environment's own message to itself that the ratify timer set at
    /// the `change`th change of the tentative decision of the day numbered
    /// `seq` has run out. A timer cannot be canceled, so one whose `change`
    /// is not the day's latest is ignored.
    Ratify {
        /// The number of the day.
        seq: u64,
        /// Which change of the day's tentative decision set the timer.
        change: u64,
    },
    /// A model player's own message to its think loop to call the model.
    Wake,
    /// A model player's own reminder, the `count`th it set, to be woken in
    /// the phase numbered `seq`. A timer cannot be canceled, so one whose
    /// `count` is not the latest set, or whose phase has ended, is ignored.
    Remind {
        /// The number of the phase.
        seq: u64,
        /// Which reminder of the player's this is.
        count: u64,
    },
}
impl free_agent::Message for Message {}

impl Message {
    /// The sender's number for this message, which a reminder to itself
    /// does not have.
    pub fn number(&self) -> Option<u64> {
        match self {
            Message::Announce { n, .. }
            | Message::Select { n, .. }
            | Message::Retract { n, .. }
            | Message::Tentative { n, .. }
            | Message::Say { n, .. } => Some(*n),
            Message::EndPhase { .. }
            | Message::Ratify { .. }
            | Message::Wake
            | Message::Remind { .. } => None,
        }
    }

    /// The phase this message is tagged with: for a message to others, the
    /// phase its sender was in; for a reminder, the phase it is for. A
    /// wake-up names none.
    pub fn seq(&self) -> Option<u64> {
        match self {
            Message::Announce { seq, .. }
            | Message::Select { seq, .. }
            | Message::Retract { seq, .. }
            | Message::Tentative { seq, .. }
            | Message::Say { seq, .. }
            | Message::EndPhase { seq }
            | Message::Ratify { seq, .. }
            | Message::Remind { seq, .. } => Some(*seq),
            Message::Wake => None,
        }
    }

    /// Who sent this message: the environment, unless it is a player's
    /// selection, retraction or utterance, which carries its sender.
    pub fn sender(&self) -> ActorId {
        match self {
            Message::Select { from, .. }
            | Message::Retract { from, .. }
            | Message::Say { from, .. } => from.clone(),
            _ => ENVIRONMENT.to_string(),
        }
    }
}

/// The environment alone, as the receivers of a message: whom a player
/// sends to by night.
pub fn to_the_environment() -> HashSet<ActorId> {
    HashSet::from([ENVIRONMENT.to_string()])
}

/// Everyone in `players` but `me`, as the receivers of a message: whom a
/// player speaks to, since the environment does not hear speech.
pub fn to_the_players_but<'a>(
    me: &PlayerId,
    players: impl IntoIterator<Item = &'a PlayerId>,
) -> HashSet<ActorId> {
    players
        .into_iter()
        .filter(|player| *player != me)
        .cloned()
        .collect()
}

/// The environment and everyone in `players` but `me`, as the receivers of
/// a message: whom a player sends to when what it says is public.
pub fn to_everyone_but<'a>(
    me: &PlayerId,
    players: impl IntoIterator<Item = &'a PlayerId>,
) -> HashSet<ActorId> {
    let mut to = to_the_players_but(me, players);
    to.insert(ENVIRONMENT.to_string());
    to
}

/// Whom `me` sends `message` to in the phase `view` shows, by the rules of
/// the game: an utterance to every other living player, and never to the
/// environment, which does nothing with speech; a selection or retraction
/// to the environment, and by day to every other living player as well,
/// since the votes are public; anything else to the environment.
pub fn receivers(message: &Message, me: &PlayerId, view: &View) -> HashSet<ActorId> {
    match (message, &view.phase) {
        (Message::Say { .. }, _) => to_the_players_but(me, &view.alive),
        (Message::Select { .. } | Message::Retract { .. }, Phase::Day) => {
            to_everyone_but(me, &view.alive)
        }
        _ => to_the_environment(),
    }
}

/// Send `message` from the actor of `context` to everyone in `to`, logging
/// it first as the actor's action if it is numbered. `seen` is how many
/// history entries a model was shown when it made the call that produced
/// the message, and `forgot` how many lines of talk the call left out, each
/// `None` for a message no model made.
pub fn act(
    context: &Context<Message, Entry>,
    message: Message,
    to: HashSet<ActorId>,
    seen: Option<usize>,
    forgot: Option<usize>,
) -> anyhow::Result<()> {
    if let Some(n) = message.number() {
        context.log(Entry::Action {
            from: context.id.clone(),
            n,
            to: to.clone(),
            message: message.clone(),
            seen,
            forgot,
        });
    }
    context.send(message, to)
}

/// Log that the actor of `context`, in the phase numbered `seq`, received
/// the message `from` numbered `n`: its observation.
pub fn observe(context: &Context<Message, Entry>, from: ActorId, n: u64, seq: u64) {
    context.log(Entry::Observation {
        by: context.id.clone(),
        from,
        n,
        seq,
    });
}

/// What the log holds: the deal, the configuration of a model-played game,
/// every message sent as the action of its sender and the observation of
/// its receiver, each message a model player withheld because its phase had
/// ended, each failure of a model player to act, each night's hunt, each
/// day's vote, and the result. Every actor logs its own actions and
/// observations: the environment logs the deal, the configuration, the
/// hunts, the votes and the result besides, and a player what it withholds
/// and its failures. Only the result summarizes anything; the rest is kept
/// whole for whatever reads the log later. Every entry made during a phase
/// names the phase in a field, so nothing about phases depends on the order
/// entries reach the log or on their times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    /// The game begins: who plays what, under which variation, how long
    /// each phase waits for the players, and how long a day's votes must
    /// stand, if they are ratified. The limits are written as `humantime`
    /// writes them, such as `"60s"`.
    Start {
        /// The name of the variation played, such as "Uniform Random".
        variation: String,
        /// Every player's role.
        roles: HashMap<PlayerId, Role>,
        /// How long a night waits for the players.
        #[serde(with = "humantime_serde")]
        night_limit: Duration,
        /// How long a day waits for the players.
        #[serde(with = "humantime_serde")]
        day_limit: Duration,
        /// How long a day's votes must stand to be ratified, if they are.
        /// Left out when they are not.
        #[serde(
            default,
            with = "humantime_serde",
            skip_serializing_if = "Option::is_none"
        )]
        ratify_limit: Option<Duration>,
    },
    /// The configuration a model-played game ran under, logged right after
    /// the deal.
    Configuration(Box<llm::Configuration>),
    /// `from` sent `message`, which it numbered `n`, to everyone in `to`.
    /// Logged by the sender.
    Action {
        /// Who sent it.
        from: ActorId,
        /// The sender's number for it.
        n: u64,
        /// Everyone it was sent to.
        to: HashSet<ActorId>,
        /// What was sent.
        message: Message,
        /// How many history entries the model was shown when it made the
        /// call that produced the message, and `None` for a message no
        /// model made: the environment's, or a uniform-random player's.
        seen: Option<usize>,
        /// How many lines of talk the call that produced the message left
        /// out to fit the context window, `None` when it left out none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forgot: Option<usize>,
    },
    /// `by` received the message `from` numbered `n` while in the phase
    /// numbered `seq`. Logged by the receiver, and naming the message
    /// without repeating it: the sender's `Action` holds it.
    Observation {
        /// Who received it.
        by: ActorId,
        /// Who sent it.
        from: ActorId,
        /// The sender's number for it.
        n: u64,
        /// The phase the receiver was in when it arrived: for the
        /// environment, the phase it is in; for a player, the latest phase
        /// it was announced. Set beside the `seq` of the message named, it
        /// shows a message that arrived in a later phase than it was sent
        /// in.
        seq: u64,
    },
    /// `from` did not send `message`, which a model made from `seen` history
    /// entries, having forgotten `forgot` lines of talk, because the phase it
    /// was made in had ended before the model answered. It was never sent, so it has no number: its `n` is 0, and
    /// the player does not remember it.
    Withheld {
        /// Who would have sent it.
        from: PlayerId,
        /// What it would have sent.
        message: Message,
        /// How many history entries the model was shown when it made the
        /// call that produced the message.
        seen: Option<usize>,
        /// How many lines of talk the call that produced the message left
        /// out to fit the context window, `None` when it left out none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forgot: Option<usize>,
    },
    /// `player` failed to act in the phase numbered `seq`: the model was
    /// not reached, or answered with something that is not a selection or
    /// a well-formed day's response.
    Failed {
        /// Who failed.
        player: PlayerId,
        /// The phase it was selecting in.
        seq: u64,
        /// What went wrong, as the error says it, causes included.
        error: String,
    },
    /// The night numbered `seq` was resolved: whom the werewolves elected,
    /// and which doctors protected them. A night in which the werewolves
    /// elected nobody logs nothing.
    Hunted {
        /// The number of the night.
        seq: u64,
        /// How the hunt came out.
        hunt: Hunt,
    },
    /// The day numbered `seq` ended: who was voted out, if anyone, and how
    /// it ended. Logged for every day, so that a day ended by its limit is
    /// told apart from one that was ratified.
    Voted {
        /// The number of the day.
        seq: u64,
        /// Who was voted out, if anyone.
        out: Option<PlayerId>,
        /// How the day ended.
        by: Ending,
    },
    /// The game is over.
    End {
        /// Who won.
        winner: Team,
        /// How many days were played through.
        days: u8,
        /// Who was still alive.
        survivors: HashSet<PlayerId>,
        /// How long the whole game took, from the deal to the end, written
        /// as `humantime` writes it.
        #[serde(with = "humantime_serde")]
        took: Duration,
    },
}

/// How a day ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ending {
    /// A player stayed selected, the decision unchanged, for the ratify
    /// limit.
    Ratified,
    /// The day limit came first.
    DayLimit,
    /// Everyone awake had selected, in a day without a ratify limit.
    AllSelected,
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
fn most_votes(ballots: &HashMap<PlayerId, PlayerId>) -> Vec<PlayerId> {
    let mut votes: HashMap<&PlayerId, usize> = HashMap::new();
    for chosen in ballots.values() {
        *votes.entry(chosen).or_default() += 1;
    }
    let Some(most) = votes.values().copied().max() else {
        return vec![];
    };
    let mut most_votes: Vec<PlayerId> = votes
        .into_iter()
        .filter(|(_, count)| *count == most)
        .map(|(player, _)| player.clone())
        .collect();
    most_votes.sort();
    most_votes
}

/// A plurality wins, and a tie is broken at random. How the werewolves
/// vote.
pub struct RandomTieBreak;

impl Vote for RandomTieBreak {
    fn elect(&self, ballots: &HashMap<PlayerId, PlayerId>) -> Option<PlayerId> {
        most_votes(ballots).choose(&mut rand::rng()).cloned()
    }
}

/// A plurality wins, and a tie elects nobody. How the village votes.
pub struct NoTieBreak;

impl Vote for NoTieBreak {
    fn elect(&self, ballots: &HashMap<PlayerId, PlayerId>) -> Option<PlayerId> {
        match most_votes(ballots).as_slice() {
            [player] => Some(player.clone()),
            _ => None,
        }
    }
}

/// What a player is shown of the game: its partial view of the state,
/// everything in it that the player may know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Rounds count from one. A round is a night and then a day.
    round: NonZero<u8>,
    /// Which half of the round it is.
    phase: Phase,
    /// The roles known to whoever holds this state.
    roles: HashMap<PlayerId, Role>,
    /// Everyone still in the game.
    alive: HashSet<PlayerId>,
}

/// The players `me` may choose from what it sees in `view`. By
/// night, the living other than `me` whose roles `me` does not know: a
/// werewolf does not kill a werewolf, the seer does not ask about anyone
/// twice, and the doctor does not protect itself. By day, everyone living
/// other than `me`, since the vote is about who the werewolves are, and a
/// player who knows is the one with the most reason to vote. Sorted by
/// name, so that a random choice among them depends on the random number
/// alone.
pub fn candidates(me: &PlayerId, view: &View) -> Vec<PlayerId> {
    let eligible = |player: &PlayerId| view.phase == Phase::Day || !view.roles.contains_key(player);
    let mut candidates: Vec<PlayerId> = view
        .alive
        .iter()
        .filter(|player| *player != me && eligible(player))
        .cloned()
        .collect();
    candidates.sort();
    candidates
}

/// Whether `me` acts in the phase `view` shows: by day everyone alive does,
/// and by night the werewolves, the doctor and the seer. The rule is the
/// one the environment goes by in waking its players, read from the role
/// `view` shows `me` as its own, so the two cannot disagree about who
/// sleeps.
pub fn awake(me: &PlayerId, view: &View) -> bool {
    view.phase.wakes(view.roles.get(me).copied())
}

#[cfg(test)]
mod tests {
    use super::llm::Memory;
    use super::*;
    use free_agent::Event;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::sync::oneshot;
    use tokio::time::Instant;

    #[test]
    fn a_limit_given_replaces_the_rules_own_and_one_left_unset_keeps_it() {
        let limits = PhaseLimits {
            night_limit: Some(Duration::from_secs(5)),
            day_limit: None,
        };
        let rules = limits.or(Rules::default());
        assert_eq!(rules.night_limit, Duration::from_secs(5));
        assert_eq!(rules.day_limit, Rules::default().day_limit);
        assert_eq!(rules.variation, "Uniform Random");
    }

    #[test]
    fn unset_counts_are_filled_from_the_defaults() {
        let roles = RoleCounts {
            werewolves: Some(1),
            ..RoleCounts::default()
        };
        assert_eq!(
            roles.or(Table::default()),
            Table {
                werewolves: 1,
                ..Table::default()
            }
        );
        assert_eq!(RoleCounts::default().or(Table::default()), Table::default());
    }

    #[test]
    fn the_deal_seats_every_role_as_many_times_as_asked() {
        let roles = Table::default().deal();
        let count = |role| roles.values().filter(|r| **r == role).count();
        assert_eq!(roles.len(), 7);
        assert_eq!(count(Role::Werewolf), 2);
        assert_eq!(count(Role::Villager), 3);
        assert_eq!(count(Role::Doctor), 1);
        assert_eq!(count(Role::Seer), 1);
    }

    #[test]
    fn the_seats_are_numbered_from_one_up_to_the_number_of_players() {
        let table = Table {
            werewolves: 1,
            villagers: 2,
            doctors: 0,
            seers: 1,
        };
        let seats: Vec<_> = table.seats().collect();
        assert_eq!(seats, ["player1", "player2", "player3", "player4"]);
    }

    #[test]
    fn the_deal_names_players_without_giving_away_their_roles() {
        let table = Table {
            werewolves: 1,
            villagers: 2,
            doctors: 0,
            seers: 0,
        };
        let mut names: Vec<_> = table.deal().into_keys().collect();
        names.sort();
        assert_eq!(names, ["player1", "player2", "player3"]);
    }

    #[test]
    fn the_deal_is_shuffled() {
        let table = Table {
            werewolves: 1,
            villagers: 1,
            doctors: 0,
            seers: 0,
        };
        let wolves: HashSet<PlayerId> = (0..50)
            .flat_map(|_| {
                table
                    .deal()
                    .into_iter()
                    .filter(|(_, role)| *role == Role::Werewolf)
                    .map(|(name, _)| name)
            })
            .collect();
        assert_eq!(wolves.len(), 2, "{wolves:?}");
    }

    #[test]
    fn a_table_is_told_role_by_role_in_the_singular_or_the_plural() {
        assert_eq!(
            Table::default().to_string(),
            "2 werewolves, 3 villagers, 1 doctor, 1 seer"
        );
        let table = Table {
            werewolves: 1,
            villagers: 0,
            doctors: 2,
            seers: 2,
        };
        assert_eq!(
            table.to_string(),
            "1 werewolf, 0 villagers, 2 doctors, 2 seers"
        );
    }

    #[test]
    fn the_start_carries_the_limits_as_humantime_writes_them_and_survives_a_trip_through_json() {
        let rules = Rules {
            night_limit: Duration::from_secs(90),
            day_limit: Duration::from_secs(40),
            ..Rules::default()
        };
        let start = rules.start(village().roles);
        assert!(
            matches!(&start, Entry::Start { variation, roles, .. }
                if variation == "Uniform Random" && roles.len() == 4),
            "{start:?}"
        );
        let json = serde_json::to_value(&start).unwrap();
        assert_eq!(json["Start"]["night_limit"], "1m 30s");
        assert_eq!(json["Start"]["day_limit"], "40s");
        // Without a ratify limit, the start is written as it always was.
        assert!(json["Start"].get("ratify_limit").is_none(), "{json}");
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, start);
    }

    #[test]
    fn the_start_carries_the_ratify_limit_when_there_is_one() {
        let rules = Rules {
            ratify_limit: Some(Duration::from_secs(20)),
            ..Rules::default()
        };
        let start = rules.start(village().roles);
        let json = serde_json::to_value(&start).unwrap();
        assert_eq!(json["Start"]["ratify_limit"], "20s");
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, start);
    }

    #[test]
    fn an_entry_survives_a_trip_through_json() {
        let state = village();
        let entries = [
            Entry::Action {
                from: ENVIRONMENT.to_string(),
                n: 2,
                to: HashSet::from(["seer".to_string()]),
                message: Message::Announce {
                    seq: 1,
                    n: 2,
                    view: state.view("seer".to_string()).unwrap(),
                    limit: Duration::from_secs(90),
                },
                seen: None,
                forgot: None,
            },
            Entry::Action {
                from: "seer".to_string(),
                n: 1,
                to: to_the_environment(),
                message: selection(1, "seer", 1, "wolf1"),
                seen: Some(3),
                forgot: None,
            },
            Entry::Observation {
                by: ENVIRONMENT.to_string(),
                from: "seer".to_string(),
                n: 1,
                seq: 2,
            },
            Entry::Hunted {
                seq: 1,
                hunt: Hunt {
                    victim: "villager".to_string(),
                    protectors: HashSet::new(),
                },
            },
            Entry::Voted {
                seq: 2,
                out: Some("wolf1".to_string()),
                by: Ending::Ratified,
            },
            Entry::Voted {
                seq: 4,
                out: None,
                by: Ending::DayLimit,
            },
            Entry::Voted {
                seq: 6,
                out: None,
                by: Ending::AllSelected,
            },
            Entry::Action {
                from: ENVIRONMENT.to_string(),
                n: 5,
                to: HashSet::from(["seer".to_string(), "villager".to_string()]),
                message: tentative(2, 5, Some("wolf1")),
                seen: None,
                forgot: None,
            },
            Entry::Failed {
                player: "seer".to_string(),
                seq: 3,
                error: "POST http://localhost:1234/v1/chat/completions: connection refused"
                    .to_string(),
            },
            Entry::Action {
                from: "seer".to_string(),
                n: 2,
                to: HashSet::from(["villager".to_string()]),
                message: utterance(2, "seer", 2, "I trust villager."),
                seen: Some(5),
                forgot: None,
            },
            Entry::Withheld {
                from: "seer".to_string(),
                message: utterance(2, "seer", 0, "Too late."),
                seen: Some(7),
                forgot: None,
            },
            Entry::Action {
                from: "seer".to_string(),
                n: 3,
                to: HashSet::from([ENVIRONMENT.to_string(), "villager".to_string()]),
                message: retraction(2, "seer", 3),
                seen: Some(9),
                forgot: None,
            },
            Entry::Withheld {
                from: "seer".to_string(),
                message: selection(2, "seer", 0, "wolf1"),
                seen: None,
                forgot: None,
            },
        ];
        for entry in entries {
            let json = serde_json::to_string(&entry).unwrap();
            let back: Entry = serde_json::from_str(&json).unwrap();
            assert_eq!(back, entry);
        }
    }

    #[test]
    fn how_much_talk_a_request_forgot_is_logged_unless_it_forgot_none() {
        let action: fn(Option<usize>) -> Entry = |forgot| Entry::Action {
            from: "seer".to_string(),
            n: 2,
            to: HashSet::from(["villager".to_string()]),
            message: utterance(2, "seer", 2, "I trust villager."),
            seen: Some(5),
            forgot,
        };
        let withheld: fn(Option<usize>) -> Entry = |forgot| Entry::Withheld {
            from: "seer".to_string(),
            message: utterance(2, "seer", 0, "Too late."),
            seen: Some(7),
            forgot,
        };
        // The fields of the entry `logged` makes, in JSON.
        let fields = |logged: Entry| {
            let json = serde_json::to_value(logged).unwrap();
            json.as_object().unwrap().values().next().unwrap().clone()
        };
        for entry in [action, withheld] {
            assert_eq!(fields(entry(Some(3)))["forgot"], 3);
            assert!(fields(entry(None)).get("forgot").is_none());
            // Read back, each entry is the one written.
            for forgot in [None, Some(3)] {
                let json = serde_json::to_string(&entry(forgot)).unwrap();
                assert_eq!(serde_json::from_str::<Entry>(&json).unwrap(), entry(forgot));
            }
        }
    }

    #[test]
    fn an_announcement_carries_its_limit_as_humantime_writes_it() {
        let announce = Message::Announce {
            seq: 1,
            n: 1,
            view: seen(&["wolf"], &[("wolf", Role::Werewolf)]),
            limit: Duration::from_secs(90),
        };
        let json = serde_json::to_value(&announce).unwrap();
        assert_eq!(json["Announce"]["limit"], "1m 30s");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, announce);
    }

    #[test]
    fn a_tentative_decision_carries_its_ratify_limit_as_humantime_writes_it_and_may_name_nobody() {
        let message = tentative(2, 5, Some("wolf"));
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["Tentative"]["ratify"], "20s");
        assert_eq!(json["Tentative"]["target"], "wolf");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, message);
        let nobody = tentative(2, 6, None);
        let json = serde_json::to_value(&nobody).unwrap();
        assert_eq!(json["Tentative"]["target"], serde_json::Value::Null);
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, nobody);
    }

    #[test]
    fn a_message_to_others_is_numbered_and_a_reminder_is_not() {
        let view = seen(&["wolf"], &[("wolf", Role::Werewolf)]);
        assert_eq!(announcement(2, view).number(), Some(2));
        assert_eq!(selection(1, "wolf", 3, "ann").number(), Some(3));
        assert_eq!(tentative(2, 4, None).number(), Some(4));
        assert_eq!(utterance(2, "wolf", 5, "Hello.").number(), Some(5));
        assert_eq!(retraction(2, "wolf", 6).number(), Some(6));
        assert_eq!(Message::EndPhase { seq: 1 }.number(), None);
        assert_eq!(Message::Ratify { seq: 2, change: 3 }.number(), None);
        assert_eq!(Message::Wake.number(), None);
        assert_eq!(Message::Remind { seq: 2, count: 1 }.number(), None);
    }

    #[test]
    fn every_message_but_a_wake_up_names_its_phase() {
        let view = seen(&["wolf"], &[("wolf", Role::Werewolf)]);
        assert_eq!(announcement(2, view).seq(), Some(2));
        assert_eq!(selection(1, "wolf", 3, "ann").seq(), Some(1));
        assert_eq!(tentative(2, 4, None).seq(), Some(2));
        assert_eq!(utterance(3, "wolf", 5, "Hello.").seq(), Some(3));
        assert_eq!(retraction(4, "wolf", 6).seq(), Some(4));
        assert_eq!(Message::EndPhase { seq: 1 }.seq(), Some(1));
        assert_eq!(Message::Ratify { seq: 2, change: 3 }.seq(), Some(2));
        assert_eq!(Message::Remind { seq: 4, count: 1 }.seq(), Some(4));
        assert_eq!(Message::Wake.seq(), None);
    }

    #[test]
    fn talk_goes_to_the_players_and_a_selection_to_the_environment_and_by_day_to_the_players_too() {
        let everyone = ["wolf", "ann", "bob"];
        let me = "wolf".to_string();
        let night = seen(&everyone, &[]);
        let day = seen_by_day(&everyone, &[]);
        let others = HashSet::from(["ann".to_string(), "bob".to_string()]);
        let said = utterance(2, "wolf", 1, "Hello.");
        assert_eq!(receivers(&said, &me, &day), others);
        assert_eq!(receivers(&said, &me, &night), others);
        let chose = selection(1, "wolf", 2, "ann");
        assert_eq!(receivers(&chose, &me, &night), to_the_environment());
        assert_eq!(
            receivers(&chose, &me, &day),
            to_everyone_but(&me, &day.alive)
        );
        // A retraction goes where a selection goes.
        let retracted = retraction(2, "wolf", 3);
        assert_eq!(receivers(&retracted, &me, &night), to_the_environment());
        assert_eq!(
            receivers(&retracted, &me, &day),
            to_everyone_but(&me, &day.alive)
        );
        let reminder = Message::EndPhase { seq: 1 };
        assert_eq!(receivers(&reminder, &me, &day), to_the_environment());
    }

    #[test]
    fn a_retraction_carries_its_phase_sender_and_number() {
        let message = retraction(2, "ann", 3);
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"Retract": {"seq": 2, "from": "ann", "n": 3}})
        );
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn a_selection_retraction_or_utterance_is_sent_by_its_player_and_everything_else_by_the_environment()
     {
        let view = seen(&["wolf"], &[("wolf", Role::Werewolf)]);
        assert_eq!(selection(1, "wolf", 3, "ann").sender(), "wolf");
        assert_eq!(retraction(2, "bob", 2).sender(), "bob");
        assert_eq!(utterance(2, "ann", 1, "Hello.").sender(), "ann");
        assert_eq!(announcement(2, view).sender(), ENVIRONMENT);
        assert_eq!(tentative(2, 4, None).sender(), ENVIRONMENT);
    }

    #[test]
    fn everyone_but_me_is_the_environment_and_the_other_players() {
        let players = ["ann".to_string(), "bob".to_string(), "cat".to_string()];
        assert_eq!(
            to_everyone_but(&"bob".to_string(), &players),
            HashSet::from([
                ENVIRONMENT.to_string(),
                "ann".to_string(),
                "cat".to_string()
            ])
        );
        assert_eq!(
            to_everyone_but(&"dan".to_string(), &[]),
            to_the_environment()
        );
        // The players alone, for speech, which the environment never hears.
        assert_eq!(
            to_the_players_but(&"bob".to_string(), &players),
            HashSet::from(["ann".to_string(), "cat".to_string()])
        );
        assert!(to_the_players_but(&"dan".to_string(), &[]).is_empty());
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

    pub(super) fn village() -> State {
        State::new(HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
            ("seer".to_string(), Role::Seer),
            ("villager".to_string(), Role::Villager),
        ]))
    }

    /// Everything `logged` between the deal and the end of the game,
    /// checking that the deal is of `roles` under `rules`, whose variation
    /// and limits it carries, and that the end names `winner` and survivors
    /// from among the players.
    pub(super) fn between_the_deal_and_the_end<'a>(
        logged: &'a [Entry],
        rules: &Rules,
        roles: &HashMap<PlayerId, Role>,
        winner: Team,
    ) -> &'a [Entry] {
        assert_eq!(logged.first(), Some(&rules.start(roles.clone())));
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
        &logged[1..logged.len() - 1]
    }

    /// What a game reports: the winner, everything the environment
    /// logged, and how long the game took.
    pub(super) type Played = (Team, Vec<Entry>, Duration);

    /// Run `episode`, whose winner comes on `won` and whose log on `log`.
    pub(super) async fn run<A: Behavior<Message = Message, Log = Entry> + 'static>(
        episode: Episode<A>,
        won: oneshot::Receiver<Team>,
        mut log: UnboundedReceiver<Event<Entry>>,
    ) -> anyhow::Result<Played> {
        let started = Instant::now();
        episode.run(Duration::from_secs(60 * 60)).await?;
        let took = started.elapsed();

        let mut logged = Vec::new();
        while let Some(event) = log.recv().await {
            logged.push(event.payload);
        }
        Ok((won.await?, logged, took))
    }

    /// `payload`, an entry or a memory, as an event logged or remembered
    /// when the game began, in no episode in particular.
    pub(super) fn logged<L>(payload: L) -> Event<L> {
        Event {
            time: std::time::UNIX_EPOCH,
            episode: Default::default(),
            payload,
        }
    }

    /// The environment's announcement of the phase numbered `seq`, showing
    /// `view`, lasting a minute. The accounts do not read the number, so
    /// it is the phase's.
    pub(super) fn announcement(seq: u64, view: View) -> Message {
        Message::Announce {
            seq,
            n: seq,
            view,
            limit: Duration::from_secs(60),
        }
    }

    /// The selections the players logged as their actions, each with
    /// everyone it was sent to, in the log's order.
    pub(super) fn selections_sent(logged: &[Entry]) -> Vec<(&Message, &HashSet<ActorId>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    message: message @ Message::Select { .. },
                    to,
                    ..
                } => Some((message, to)),
                _ => None,
            })
            .collect()
    }

    /// The selections the players logged as their actions, in the log's
    /// order.
    pub(super) fn selections(logged: &[Entry]) -> Vec<&Message> {
        selections_sent(logged)
            .into_iter()
            .map(|(message, _)| message)
            .collect()
    }

    /// The messages `by` logged as its observations, each as who sent it,
    /// the sender's number for it, and the phase `by` was in when it
    /// arrived, in the log's order.
    pub(super) fn observed_by<'a>(logged: &'a [Entry], by: &str) -> Vec<(&'a str, u64, u64)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Observation {
                    by: observer,
                    from,
                    n,
                    seq,
                } if observer == by => Some((from.as_str(), *n, *seq)),
                _ => None,
            })
            .collect()
    }

    /// The selections the environment logged as its observations, as
    /// [`observed_by`] has them.
    pub(super) fn observed_by_the_environment(logged: &[Entry]) -> Vec<(&str, u64, u64)> {
        observed_by(logged, ENVIRONMENT)
    }

    /// A player's memory of receiving `message`, in no episode in
    /// particular.
    pub(super) fn received(message: Message) -> Event<Memory> {
        logged(Memory::Observed { message })
    }

    /// A selection of `target` by `from` in the phase numbered `seq`, the
    /// `n`th message `from` sent.
    pub(super) fn selection(seq: u64, from: &str, n: u64, target: &str) -> Message {
        Message::Select {
            seq,
            from: from.to_string(),
            n,
            target: target.to_string(),
        }
    }

    /// `from`'s retraction of its vote in the phase numbered `seq`, the
    /// `n`th message `from` sent.
    pub(super) fn retraction(seq: u64, from: &str, n: u64) -> Message {
        Message::Retract {
            seq,
            from: from.to_string(),
            n,
        }
    }

    /// `from`'s utterance of `text` in the phase numbered `seq`, the `n`th
    /// message `from` sent.
    pub(super) fn utterance(seq: u64, from: &str, n: u64, text: &str) -> Message {
        Message::Say {
            seq,
            from: from.to_string(),
            n,
            text: text.to_string(),
        }
    }

    /// The environment's tentative decision for the day numbered `seq`,
    /// the `n`th message it sent: `target`, or nobody, to be ratified
    /// after twenty seconds.
    pub(super) fn tentative(seq: u64, n: u64, target: Option<&str>) -> Message {
        Message::Tentative {
            seq,
            n,
            target: target.map(str::to_string),
            ratify: Duration::from_secs(20),
        }
    }

    /// One werewolf against `villagers`.
    pub(super) fn one_wolf_against(villagers: &[&str]) -> HashMap<PlayerId, Role> {
        let mut roles = HashMap::from([("wolf".to_string(), Role::Werewolf)]);
        for villager in villagers {
            roles.insert(villager.to_string(), Role::Villager);
        }
        roles
    }

    /// What a player sees of a village on the first night, where everyone
    /// in `alive` lives and the player knows the roles in `known`.
    pub(super) fn seen(alive: &[&str], known: &[(&str, Role)]) -> View {
        View {
            round: NonZero::new(1).unwrap(),
            phase: Phase::Night,
            roles: known
                .iter()
                .map(|(name, role)| (name.to_string(), *role))
                .collect(),
            alive: alive.iter().map(|name| name.to_string()).collect(),
        }
    }

    /// What a player sees of a village on the first day: [`seen`], by day.
    pub(super) fn seen_by_day(alive: &[&str], known: &[(&str, Role)]) -> View {
        View {
            phase: Phase::Day,
            ..seen(alive, known)
        }
    }

    fn seen_by(observer: &str) -> Vec<String> {
        let view = village().view(observer.to_string()).unwrap();
        let mut seen: Vec<_> = view.roles.keys().cloned().collect();
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
        let view = state.view("seer".to_string()).unwrap();
        let mut seen: Vec<_> = view.roles.keys().cloned().collect();
        seen.sort();
        assert_eq!(seen, ["seer", "wolf1"]);
        assert_eq!(view.roles["wolf1"], Role::Werewolf);
    }

    #[test]
    fn the_view_keeps_the_rest_of_the_state() {
        let state = village();
        let view = state.view("villager".to_string()).unwrap();
        assert_eq!(view.round, state.round);
        assert_eq!(view.alive, state.alive);
    }

    /// `players` in name order.
    pub(super) fn sorted(players: HashSet<PlayerId>) -> Vec<PlayerId> {
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
            Some(Hunt {
                victim: "villager".to_string(),
                protectors: HashSet::new(),
            })
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
        assert_eq!(
            state.resolve_night(&choices, &RandomTieBreak),
            Some(Hunt {
                victim: "villager".to_string(),
                protectors: HashSet::from(["doctor".to_string()]),
            })
        );
        assert!(state.alive.contains("villager"));
    }

    #[test]
    fn a_hunt_kills_its_victim_unless_a_doctor_protected_them() {
        let hunt = Hunt {
            victim: "villager".to_string(),
            protectors: HashSet::new(),
        };
        assert_eq!(hunt.dead(), Some(&"villager".to_string()));
        let saved = Hunt {
            protectors: HashSet::from(["doctor".to_string()]),
            ..hunt
        };
        assert_eq!(saved.dead(), None);
    }

    #[test]
    fn the_seer_learns_the_role_of_whom_it_chooses() {
        let mut state = full_village();
        let choices = ballots(&[("wolf1", "villager"), ("seer", "wolf1")]);
        state.resolve_night(&choices, &RandomTieBreak);
        let view = state.view("seer".to_string()).unwrap();
        assert_eq!(view.roles["wolf1"], Role::Werewolf);
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

    #[test]
    fn by_night_the_candidates_are_the_living_whose_role_is_unknown_in_name_order() {
        let view = seen(
            &["wolf1", "wolf2", "bob", "ann"],
            &[("wolf1", Role::Werewolf), ("wolf2", Role::Werewolf)],
        );
        assert_eq!(
            candidates(&"wolf1".to_string(), &view),
            ["ann".to_string(), "bob".to_string()]
        );
    }

    #[test]
    fn a_player_is_never_its_own_candidate() {
        let view = seen(&["ann", "bob"], &[("ann", Role::Villager)]);
        assert_eq!(candidates(&"ann".to_string(), &view), ["bob".to_string()]);
    }

    #[test]
    fn a_player_who_knows_everyone_alive_has_no_candidates_by_night() {
        let view = seen(
            &["seer", "wolf"],
            &[("seer", Role::Seer), ("wolf", Role::Werewolf)],
        );
        assert!(candidates(&"seer".to_string(), &view).is_empty());
    }

    #[test]
    fn by_day_the_candidates_are_everyone_living_but_oneself_whatever_their_roles() {
        let view = seen_by_day(
            &["seer", "wolf", "ann"],
            &[("seer", Role::Seer), ("wolf", Role::Werewolf)],
        );
        assert_eq!(
            candidates(&"seer".to_string(), &view),
            ["ann".to_string(), "wolf".to_string()]
        );
    }

    #[test]
    fn the_dead_are_not_candidates() {
        let mut state = village();
        state.kill(&"villager".to_string());
        let view = state.view("seer".to_string()).unwrap();
        assert_eq!(
            candidates(&"seer".to_string(), &view),
            ["wolf1".to_string(), "wolf2".to_string()]
        );
    }

    #[test]
    fn a_player_is_awake_in_a_phase_as_its_view_shows_it_when_the_environment_says_it_is() {
        // The environment and each player read the one rule, so whoever the
        // environment has awake sees itself awake, and nobody else does.
        let mut state = full_village();
        state.alive.remove("wolf2");
        for _ in 0..2 {
            let awake_by_the_rules = state.awake();
            for player in &state.alive {
                let view = state.view(player.clone()).unwrap();
                assert_eq!(
                    awake(player, &view),
                    awake_by_the_rules.contains(player),
                    "{player} in {:?}",
                    state.phase
                );
            }
            state.next();
        }
        // By night a villager sleeps, and so does a player shown no role;
        // by day both are awake.
        let villager = "villager".to_string();
        let known = [("villager", Role::Villager)];
        assert!(!awake(&villager, &seen(&["wolf1", "villager"], &known)));
        assert!(awake(
            &villager,
            &seen_by_day(&["wolf1", "villager"], &known)
        ));
        assert!(!awake(&villager, &seen(&["wolf1", "villager"], &[])));
        assert!(awake(&villager, &seen_by_day(&["wolf1", "villager"], &[])));
    }
}
