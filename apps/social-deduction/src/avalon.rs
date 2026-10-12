//! Avalon: the rules, the types they are written in, the one kind of
//! actor every game holds, and the function that builds every game.
//!
//! A game is an episode of [free agents](free_agent): the rules in one
//! actor, the environment, and a player in each of the others. The
//! environment alone holds the state of the game: the quest and the
//! proposal, the phase, who leads, every player's [`Role`], the team on
//! the table, and the record of every quest. A player sees the game only
//! through the [`View`] the environment sends it, which carries all of
//! that but the roles: a player is shown its own role and what it
//! [sees](Seen) of some others. Holding no state, a player can only talk.
//!
//! Each quest is led by a leader, who proposes a team; every player votes
//! on it; if it is approved, its members go on the quest, and each one
//! supports or sabotages it. Play goes on until one [`Team`] has won. The
//! log is written by every actor, an [`Entry`] at a time, in the language
//! of reinforcement learning: each message an actor sends to others is an
//! action of its sender, which the sender logs whole, once, and an
//! observation of each receiver, which the receiver logs as a receipt
//! naming the message by its sender and number. The environment logs the
//! deal, its announcements, the choices it receives, how each phase came
//! out, and the result; every player logs what it receives and the
//! choices it sends.
//!
//! The environment is [`announced`]'s, and each of an episode's players
//! follows the policy of the strategy it is dealt with its role: for now,
//! the [`random`] policy. The [`config`] says which strategies are dealt.
//! An episode holds one kind of actor, [`Actor`], which is the environment
//! or a player, and [`game`] builds every episode, with each seat's player
//! built for it.

pub mod announced;
pub mod config;
pub mod random;
pub mod report;

use crate::report::in_order;
use crate::seat::{ENVIRONMENT, PlayerId, to_everyone_but, to_the_environment, to_the_players_but};
use anyhow::ensure;
use async_trait::async_trait;
use clap::Args;
use free_agent::{ActorId, ActorInit, Behavior, Builder, Context, Episode, Logger, ThinkBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZero;
use std::time::Duration;
use tokio::sync::oneshot;

/// How a seat is built: its player's behavior, and its think loop if it
/// has one.
pub type Seat = (Builder<Actor>, Option<ThinkBuilder<Message, Entry>>);

/// What a seat is dealt: a role, and the strategy its player plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dealt {
    /// The role.
    pub role: Role,
    /// The name of the strategy.
    pub strategy: String,
}

/// An episode of a game of `deal` under `rules`: the announcing
/// environment, which may reach and stop every player, logs
/// `configuration` right after the deal, and sends the winning team on
/// `winner` when the game ends; and in each seat the player `player`
/// builds for it. Everyone holds `logger`. A player may send to the
/// environment and to every other player, and stops nobody.
pub fn game(
    deal: HashMap<PlayerId, Dealt>,
    rules: Rules,
    configuration: config::Configuration,
    winner: oneshot::Sender<Team>,
    mut player: impl FnMut(&PlayerId) -> Seat,
    logger: Logger<Entry>,
) -> Episode<Actor> {
    let players: Vec<PlayerId> = deal.keys().cloned().collect();
    let configuration = Entry::Configuration(Box::new(configuration));
    let environment = announced::init(deal, rules, configuration, winner, Actor::Environment);
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

/// What an actor in a game of Avalon does: run it, or play in it. An
/// episode holds one kind of actor, so every side meets here and each
/// method goes to whichever side this is.
pub enum Actor {
    /// The side that holds the game.
    Environment(announced::Environment),
    /// A player who chooses at random.
    Random(random::Player),
    /// A player who says whatever its script says.
    #[cfg(test)]
    Puppet(tests::Puppet),
}

#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Random(player) => player.context(),
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.context(),
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Random(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.receive(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Random(player) => player.start().await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.start().await,
        }
    }
}

/// What a player chooses in a phase: the leader a team, every player a
/// ballot, each member of the team a deed, and the Assassin a target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Choice {
    /// The team the leader proposes.
    Team(Vec<PlayerId>),
    /// A player's vote on the team.
    Ballot(Ballot),
    /// A member's deed on the quest.
    Deed(Deed),
    /// Whom the Assassin names as Merlin.
    Target(PlayerId),
}

impl Choice {
    /// The team, if this is one.
    fn team(&self) -> Option<&Vec<PlayerId>> {
        match self {
            Choice::Team(team) => Some(team),
            _ => None,
        }
    }

    /// The ballot, if this is one.
    fn ballot(&self) -> Option<Ballot> {
        match self {
            Choice::Ballot(ballot) => Some(*ballot),
            _ => None,
        }
    }

    /// The deed, if this is one.
    fn deed(&self) -> Option<Deed> {
        match self {
            Choice::Deed(deed) => Some(*deed),
            _ => None,
        }
    }

    /// The target, if this is one.
    fn target(&self) -> Option<&PlayerId> {
        match self {
            Choice::Target(target) => Some(target),
            _ => None,
        }
    }

    /// This choice as the message `from` sends in the phase numbered
    /// `seq`, its `n`th.
    pub fn message(self, seq: u64, from: PlayerId, n: u64) -> Message {
        match self {
            Choice::Team(team) => Message::Propose { seq, from, n, team },
            Choice::Ballot(ballot) => Message::Vote {
                seq,
                from,
                n,
                ballot,
            },
            Choice::Deed(deed) => Message::Perform { seq, from, n, deed },
            Choice::Target(target) => Message::Select {
                seq,
                from,
                n,
                target,
            },
        }
    }
}

/// What the awaited players chose in a phase: each player's choice.
type Choices = HashMap<PlayerId, Choice>;

/// The two sides. A game ends when one of them has won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// Merlin, Percival and the loyal servants of Arthur: win with three
    /// successful quests, unless the Assassin then names Merlin.
    Good,
    /// The Assassin, Morgana, Mordred, Oberon and the minions of Mordred:
    /// win with three failed quests, five failed proposals in a row, or
    /// Merlin named.
    Evil,
}

impl fmt::Display for Team {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Team::Good => "good",
            Team::Evil => "evil",
        })
    }
}

/// What a player is. The roles are ordered as they are dealt, good before
/// evil.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Role {
    /// Good, and sees every evil player but Mordred.
    Merlin,
    /// Good, and sees Merlin and Morgana without telling them apart.
    Percival,
    /// A loyal servant of Arthur: good, and sees nobody.
    Servant,
    /// Evil, sees the other evil players but Oberon, and names Merlin
    /// after the third successful quest.
    Assassin,
    /// Evil, sees the other evil players but Oberon, and looks like Merlin
    /// to Percival.
    Morgana,
    /// Evil, sees the other evil players but Oberon, and is hidden from
    /// Merlin.
    Mordred,
    /// Evil, and neither sees the other evil players nor is seen by them.
    Oberon,
    /// A minion of Mordred: evil, and sees the other evil players but
    /// Oberon.
    Minion,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Merlin => "Merlin",
            Role::Percival => "Percival",
            Role::Servant => "Servant",
            Role::Assassin => "Assassin",
            Role::Morgana => "Morgana",
            Role::Mordred => "Mordred",
            Role::Oberon => "Oberon",
            Role::Minion => "Minion",
        })
    }
}

impl Role {
    /// The word a count of the role goes by on the command line: the
    /// plural of a role a table may hold several of, `servants` and
    /// `minions`, and the name of any other.
    pub fn counted(self) -> &'static str {
        match self {
            Role::Merlin => "merlin",
            Role::Percival => "percival",
            Role::Servant => "servants",
            Role::Assassin => "assassin",
            Role::Morgana => "morgana",
            Role::Mordred => "mordred",
            Role::Oberon => "oberon",
            Role::Minion => "minions",
        }
    }

    /// Which side this role wins with.
    pub fn team(self) -> Team {
        match self {
            Role::Merlin | Role::Percival | Role::Servant => Team::Good,
            Role::Assassin | Role::Morgana | Role::Mordred | Role::Oberon | Role::Minion => {
                Team::Evil
            }
        }
    }

    /// Whether a table holds at most one of this role: every role but
    /// Servant and Minion.
    pub fn unique(self) -> bool {
        !matches!(self, Role::Servant | Role::Minion)
    }

    /// What a player of this role sees of a player of role `other`, if
    /// anything. Merlin sees every evil player but Mordred; Percival sees
    /// Merlin and Morgana, without telling them apart; every evil player
    /// but Oberon sees every other evil player but Oberon; and Oberon and
    /// the Servants see nobody.
    pub fn sees(self, other: Role) -> Option<Seen> {
        match (self, other) {
            (Role::Merlin, Role::Mordred) => None,
            (Role::Merlin, other) if other.team() == Team::Evil => Some(Seen::Evil),
            (Role::Percival, Role::Merlin | Role::Morgana) => Some(Seen::MerlinOrMorgana),
            (Role::Oberon, _) | (_, Role::Oberon) => None,
            (me, other) if me.team() == Team::Evil && other.team() == Team::Evil => {
                Some(Seen::Evil)
            }
            _ => None,
        }
    }
}

/// What a player is shown of another: knowledge in Avalon is of teams,
/// not roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Seen {
    /// The other player is evil.
    Evil,
    /// The other player is Merlin or Morgana.
    MerlinOrMorgana,
}

/// How many of each role sit at the table, for a game to flatten into its
/// command line arguments. A count left unset has no `clap` default, so
/// that the game can tell it apart from a count that was given, and fill
/// it in from the configuration file or [`Table::default`].
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct RoleCounts {
    /// How many Merlins.
    #[arg(long, value_name = "N", help = how_many("Merlins", Table::default().merlin))]
    pub merlin: Option<usize>,
    /// How many Percivals.
    #[arg(long, value_name = "N", help = how_many("Percivals", Table::default().percival))]
    pub percival: Option<usize>,
    /// How many loyal servants of Arthur.
    #[arg(long, value_name = "N", help = how_many("Servants", Table::default().servants))]
    pub servants: Option<usize>,
    /// How many Assassins.
    #[arg(long, value_name = "N", help = how_many("Assassins", Table::default().assassin))]
    pub assassin: Option<usize>,
    /// How many Morganas.
    #[arg(long, value_name = "N", help = how_many("Morganas", Table::default().morgana))]
    pub morgana: Option<usize>,
    /// How many Mordreds.
    #[arg(long, value_name = "N", help = how_many("Mordreds", Table::default().mordred))]
    pub mordred: Option<usize>,
    /// How many Oberons.
    #[arg(long, value_name = "N", help = how_many("Oberons", Table::default().oberon))]
    pub oberon: Option<usize>,
    /// How many minions of Mordred.
    #[arg(long, value_name = "N", help = how_many("Minions", Table::default().minions))]
    pub minions: Option<usize>,
}

impl RoleCounts {
    /// How many of `role` were given, if any.
    pub fn get(&self, role: Role) -> Option<usize> {
        match role {
            Role::Merlin => self.merlin,
            Role::Percival => self.percival,
            Role::Servant => self.servants,
            Role::Assassin => self.assassin,
            Role::Morgana => self.morgana,
            Role::Mordred => self.mordred,
            Role::Oberon => self.oberon,
            Role::Minion => self.minions,
        }
    }

    /// The flag that gives how many of `role`, such as `--servants`.
    pub fn flag(role: Role) -> String {
        format!("--{}", role.counted())
    }
}

/// The help for a count of `what`, naming the `default` the code fills in
/// when the count is left unset.
fn how_many(what: &str, default: usize) -> String {
    format!("How many {what} [default: {default}]")
}

/// How long each phase waits for the players, for a game to flatten into
/// its command line arguments. A limit left unset has no `clap` default, so
/// that the game can tell it apart from a limit that was given, and fill it
/// in from the configuration file or [`Rules::default`].
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct PhaseLimits {
    /// How long a proposal waits for its leader.
    #[arg(long, value_name = "D", value_parser = humantime::parse_duration, help = how_long("a proposal waits", Rules::default().propose_limit))]
    pub propose_limit: Option<Duration>,
    /// How long a vote waits for the players.
    #[arg(long, value_name = "D", value_parser = humantime::parse_duration, help = how_long("a vote waits", Rules::default().vote_limit))]
    pub vote_limit: Option<Duration>,
    /// How long a quest waits for its team.
    #[arg(long, value_name = "D", value_parser = humantime::parse_duration, help = how_long("a quest waits", Rules::default().quest_limit))]
    pub quest_limit: Option<Duration>,
    /// How long the assassination waits for the Assassin.
    #[arg(long, value_name = "D", value_parser = humantime::parse_duration, help = how_long("the assassination waits", Rules::default().assassinate_limit))]
    pub assassinate_limit: Option<Duration>,
}

/// The help for a limit, saying what `waits`, and naming the `default`
/// the code fills in when the limit is left unset.
fn how_long(waits: &str, default: Duration) -> String {
    format!(
        "How long {waits} [default: {}]",
        humantime::format_duration(default)
    )
}

/// How many of each role sit at the table.
#[derive(Debug, PartialEq, Eq)]
pub struct Table {
    /// How many Merlins: 0 or 1.
    pub merlin: usize,
    /// How many Percivals: 0 or 1.
    pub percival: usize,
    /// How many loyal servants of Arthur.
    pub servants: usize,
    /// How many Assassins: 0 or 1.
    pub assassin: usize,
    /// How many Morganas: 0 or 1.
    pub morgana: usize,
    /// How many Mordreds: 0 or 1.
    pub mordred: usize,
    /// How many Oberons: 0 or 1.
    pub oberon: usize,
    /// How many minions of Mordred.
    pub minions: usize,
}

impl Default for Table {
    /// Merlin, the Assassin, two Servants and one Minion: five players.
    fn default() -> Self {
        Table {
            merlin: 1,
            percival: 0,
            servants: 2,
            assassin: 1,
            morgana: 0,
            mordred: 0,
            oberon: 0,
            minions: 1,
        }
    }
}

/// The fewest players a game is played by.
pub const FEWEST: usize = 5;

/// The most players a game is played by.
pub const MOST: usize = 10;

impl Table {
    /// Each role and how many of it, in the order the roles are dealt.
    pub fn counts(&self) -> [(Role, usize); 8] {
        [
            (Role::Merlin, self.merlin),
            (Role::Percival, self.percival),
            (Role::Servant, self.servants),
            (Role::Assassin, self.assassin),
            (Role::Morgana, self.morgana),
            (Role::Mordred, self.mordred),
            (Role::Oberon, self.oberon),
            (Role::Minion, self.minions),
        ]
    }

    /// How many players sit at the table.
    pub fn players(&self) -> usize {
        self.counts().iter().map(|(_, count)| count).sum()
    }

    /// Whether the table is one a game is played at: 5 to 10 players, at
    /// most one of each role but Servant and Minion, and a Merlin exactly
    /// when there is an Assassin. The split between the teams is not
    /// checked.
    ///
    /// # Errors
    ///
    /// What is wrong with the table, the first thing found.
    pub fn check(&self) -> anyhow::Result<()> {
        let players = self.players();
        ensure!(
            (FEWEST..=MOST).contains(&players),
            "Avalon is played by {FEWEST} to {MOST} players, and this table seats {players}"
        );
        for (role, count) in self.counts() {
            ensure!(
                !role.unique() || count <= 1,
                "a table has at most one {role}, and this one has {count}"
            );
        }
        ensure!(
            self.merlin == self.assassin,
            "a table has Merlin exactly when it has an Assassin, and this one has {} Merlin and {} Assassin",
            self.merlin,
            self.assassin
        );
        Ok(())
    }

    /// The seats, named `player1` onward, one per player. The names say
    /// nothing about the roles, since every player sees every name.
    pub fn seats(&self) -> impl Iterator<Item = PlayerId> {
        (1..=self.players()).map(|seat| format!("player{seat}"))
    }
}

/// The phases of a quest. A game begins with the first quest's first
/// proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// The leader proposes a team.
    Propose,
    /// Every player votes on the team.
    Vote,
    /// Each member of the team supports or sabotages the quest.
    Quest,
    /// The Assassin names Merlin, after the third successful quest.
    Assassinate,
}

impl Phase {
    /// Whether this phase waits on `player`, of `role`, when `leader`
    /// leads and `team` is on the table: a proposal waits on the leader, a
    /// vote on everyone, a quest on its team, and the assassination on the
    /// Assassin. The one rule of whom a phase waits on, so that the
    /// environment and the players cannot disagree about it.
    fn awaits(
        self,
        player: &PlayerId,
        role: Role,
        leader: &PlayerId,
        team: Option<&Vec<PlayerId>>,
    ) -> bool {
        match self {
            Phase::Propose => player == leader,
            Phase::Vote => true,
            Phase::Quest => team.is_some_and(|team| team.contains(player)),
            Phase::Assassinate => role == Role::Assassin,
        }
    }
}

/// A player's vote on a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ballot {
    /// The team should go.
    Approve,
    /// The team should not go.
    Reject,
}

/// What a member of the team does on the quest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Deed {
    /// Help the quest succeed.
    Support,
    /// Help the quest fail. Only an evil player may.
    Sabotage,
}

/// How a vote came out: each player's ballot, and whether the team goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// Each player's ballot. A player who did not vote is absent.
    pub ballots: HashMap<PlayerId, Ballot>,
    /// Whether the team goes: the approvals are a strict majority of all
    /// the players.
    pub approved: bool,
}

impl Tally {
    /// The tally of `ballots` at a table of `players`: approved when twice
    /// the approvals is more than the number of players, so that a player
    /// who did not vote did not approve.
    pub fn of(ballots: HashMap<PlayerId, Ballot>, players: usize) -> Self {
        let approvals = ballots
            .values()
            .filter(|ballot| **ballot == Ballot::Approve)
            .count();
        Self {
            ballots,
            approved: 2 * approvals > players,
        }
    }
}

/// How a quest came out: who went, how many sabotaged it, and whether it
/// succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestResult {
    /// Who went on the quest.
    pub team: Vec<PlayerId>,
    /// How many sabotaged it. Who did is never told.
    pub sabotages: usize,
    /// Whether it succeeded: the sabotages fell short of the number that
    /// fails it.
    pub succeeded: bool,
}

/// How many players go on each quest, by the number of players: a row for
/// 5, 6, 7, and 8 to 10 players.
const TEAM_SIZES: [[usize; 5]; 4] = [
    [2, 3, 2, 3, 3],
    [2, 3, 4, 3, 4],
    [2, 3, 3, 4, 4],
    [3, 4, 4, 5, 5],
];

/// How many players go on the quest numbered `quest` at a table of
/// `players`.
pub fn team_size(players: usize, quest: NonZero<u8>) -> usize {
    let row = players.clamp(FEWEST, 8) - FEWEST;
    TEAM_SIZES[row][usize::from(quest.get()) - 1]
}

/// How many sabotages fail the quest numbered `quest` at a table of
/// `players`: two on the fourth quest with seven or more players, and one
/// otherwise.
pub fn sabotages_needed(players: usize, quest: NonZero<u8>) -> usize {
    if quest.get() == 4 && players >= 7 {
        2
    } else {
        1
    }
}

/// How many proposals in a row may fail before evil wins.
pub const PROPOSALS: u8 = 5;

/// How many quests one side must win.
const TO_WIN: usize = 3;

/// The state of a game, held by the environment alone.
struct State {
    /// Every player's role.
    roles: HashMap<PlayerId, Role>,
    /// The players in seat order, the order the lead passes in.
    seats: Vec<PlayerId>,
    /// Quests count from one.
    quest: NonZero<u8>,
    /// How many proposals have failed in a row since the last quest.
    failed: u8,
    /// The phase underway.
    phase: Phase,
    /// Where the leader sits in [`State::seats`].
    lead: usize,
    /// The team proposed, in the vote and the quest.
    team: Option<Vec<PlayerId>>,
    /// The vote that just ended, in the phase right after it.
    tally: Option<Tally>,
    /// Every quest so far.
    record: Vec<QuestResult>,
    /// Whether the Assassin named Merlin, once it has named anyone or run
    /// out of time.
    named: Option<bool>,
}

impl State {
    /// A game of `roles`, at the first quest's first proposal, led by the
    /// first seat.
    fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let seats = in_order(roles.keys()).into_iter().cloned().collect();
        Self {
            roles,
            seats,
            quest: NonZero::new(1).unwrap(),
            failed: 0,
            phase: Phase::Propose,
            lead: 0,
            team: None,
            tally: None,
            record: vec![],
            named: None,
        }
    }

    /// Who leads.
    fn leader(&self) -> &PlayerId {
        &self.seats[self.lead]
    }

    /// How many players the quest underway needs.
    fn size(&self) -> usize {
        team_size(self.seats.len(), self.quest)
    }

    /// What `player_id` sees of the others, as its role has it.
    fn sees(&self, player_id: &PlayerId) -> HashMap<PlayerId, Seen> {
        let mine = self.roles[player_id];
        self.roles
            .iter()
            .filter(|(other, _)| *other != player_id)
            .filter_map(|(other, role)| mine.sees(*role).map(|seen| (other.clone(), seen)))
            .collect()
    }

    /// What `player_id` is allowed to see: everything but the roles, of
    /// which it sees its own and what its role shows it of the others.
    fn view(&self, player_id: &PlayerId) -> View {
        View {
            quest: self.quest,
            proposal: NonZero::new(self.failed + 1).unwrap(),
            phase: self.phase,
            leader: self.leader().clone(),
            size: self.size(),
            sabotages_needed: sabotages_needed(self.seats.len(), self.quest),
            team: self.team.clone(),
            tally: self.tally.clone(),
            record: self.record.clone(),
            role: self.roles[player_id],
            sees: self.sees(player_id),
            players: self.seats.clone(),
        }
    }

    /// The players the current phase waits on, as [`Phase::awaits`] has
    /// it.
    fn awaited(&self) -> HashSet<PlayerId> {
        self.seats
            .iter()
            .filter(|player| {
                self.phase.awaits(
                    player,
                    self.roles[*player],
                    self.leader(),
                    self.team.as_ref(),
                )
            })
            .cloned()
            .collect()
    }

    /// Whether `from` may make `choice` in the current phase: it is of the
    /// kind the phase takes; a team is the size the quest needs, of
    /// distinct players at the table; a sabotage comes from an evil
    /// player; and the Assassin's target is one of its [`candidates`].
    /// Whether the phase waits on `from` is the environment's to say.
    fn legal(&self, from: &PlayerId, choice: &Choice) -> bool {
        match self.phase {
            Phase::Propose => choice.team().is_some_and(|team| {
                let distinct: HashSet<&PlayerId> = team.iter().collect();
                team.len() == self.size()
                    && distinct.len() == team.len()
                    && team.iter().all(|player| self.roles.contains_key(player))
            }),
            Phase::Vote => choice.ballot().is_some(),
            Phase::Quest => choice
                .deed()
                .is_some_and(|deed| deed == Deed::Support || self.roles[from].team() == Team::Evil),
            Phase::Assassinate => choice
                .target()
                .is_some_and(|target| candidates(from, &self.view(from)).contains(target)),
        }
    }

    /// Resolve the current phase from what the awaited players chose. A
    /// proposal puts the leader's team on the table, or fails without
    /// one; a vote is tallied, and fails the proposal unless it approves
    /// the team; a quest is recorded, a missing deed as support, and starts
    /// the count of failed proposals over; and the assassination names
    /// Merlin or not. Returns what the phase came to, as the entry that
    /// records it in the phase numbered `seq`.
    fn resolve(&mut self, choices: &Choices, seq: u64) -> Entry {
        match self.phase {
            Phase::Propose => {
                let team = choices.values().find_map(Choice::team).cloned();
                if team.is_none() {
                    self.failed += 1;
                }
                self.team.clone_from(&team);
                Entry::Proposed { seq, team }
            }
            Phase::Vote => {
                let ballots = choices
                    .iter()
                    .filter_map(|(player, choice)| Some((player.clone(), choice.ballot()?)))
                    .collect();
                let tally = Tally::of(ballots, self.seats.len());
                if !tally.approved {
                    self.failed += 1;
                }
                self.tally = Some(tally.clone());
                Entry::Tallied { seq, tally }
            }
            Phase::Quest => {
                let team = self.team.clone().unwrap_or_default();
                let sabotages = choices
                    .values()
                    .filter(|choice| choice.deed() == Some(Deed::Sabotage))
                    .count();
                let result = QuestResult {
                    team,
                    sabotages,
                    succeeded: sabotages < sabotages_needed(self.seats.len(), self.quest),
                };
                self.record.push(result.clone());
                self.failed = 0;
                Entry::Quested { seq, result }
            }
            Phase::Assassinate => {
                let target = choices.values().find_map(Choice::target).cloned();
                let merlin = target
                    .as_ref()
                    .is_some_and(|target| self.roles[target] == Role::Merlin);
                self.named = Some(merlin);
                Entry::Assassinated {
                    seq,
                    target,
                    merlin,
                }
            }
        }
    }

    /// How many quests have come out `succeeded`.
    fn quests(&self, succeeded: bool) -> usize {
        self.record
            .iter()
            .filter(|quest| quest.succeeded == succeeded)
            .count()
    }

    /// Whether a Merlin sits at the table.
    fn has_merlin(&self) -> bool {
        self.roles.values().any(|role| *role == Role::Merlin)
    }

    /// Move on from the phase just resolved: a proposed team to the vote,
    /// an approved one to its quest, the third successful quest to the
    /// assassination, and anything else to the next proposal, led by the
    /// next seat, wrapping around. The tally is shown only in the phase
    /// right after the vote, and the team only in the vote and the quest.
    fn next(&mut self) {
        let after = self.phase;
        self.phase = match after {
            Phase::Propose if self.team.is_some() => Phase::Vote,
            Phase::Vote if self.tally.as_ref().is_some_and(|tally| tally.approved) => Phase::Quest,
            Phase::Quest if self.quests(true) == TO_WIN => Phase::Assassinate,
            _ => {
                if after == Phase::Quest {
                    self.quest = self.quest.checked_add(1).expect("five quests at most");
                }
                self.lead = (self.lead + 1) % self.seats.len();
                self.team = None;
                Phase::Propose
            }
        };
        if after != Phase::Vote {
            self.tally = None;
        }
    }

    /// Who has won, if anyone, and how: evil with five failed proposals in
    /// a row or three failed quests; good with three successful quests
    /// unless the Assassin then names Merlin, and evil if it does. A game
    /// without Merlin has no assassination.
    fn winner(&self) -> Option<(Team, Victory)> {
        if self.failed == PROPOSALS {
            Some((Team::Evil, Victory::Rejections))
        } else if self.quests(false) == TO_WIN {
            Some((Team::Evil, Victory::Sabotage))
        } else if self.quests(true) < TO_WIN {
            None
        } else {
            match (self.named, self.has_merlin()) {
                (Some(true), _) => Some((Team::Evil, Victory::Assassination)),
                (Some(false), _) | (None, false) => Some((Team::Good, Victory::Quests)),
                (None, true) => None,
            }
        }
    }
}

/// How a game is played: how long each phase waits for the players. A
/// choice that comes later is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// How long a proposal waits for its leader.
    pub propose_limit: Duration,
    /// How long a vote waits for the players.
    pub vote_limit: Duration,
    /// How long a quest waits for its team.
    pub quest_limit: Duration,
    /// How long the assassination waits for the Assassin.
    pub assassinate_limit: Duration,
}

impl Rules {
    /// How long `phase` waits for the players.
    pub fn limit(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Propose => self.propose_limit,
            Phase::Vote => self.vote_limit,
            Phase::Quest => self.quest_limit,
            Phase::Assassinate => self.assassinate_limit,
        }
    }

    /// The entry that opens a game of `deal` under these rules: every
    /// player's role and strategy, and the limits.
    pub fn start(&self, deal: &HashMap<PlayerId, Dealt>) -> Entry {
        let (roles, strategies) = deal
            .iter()
            .map(|(id, dealt)| {
                (
                    (id.clone(), dealt.role),
                    (id.clone(), dealt.strategy.clone()),
                )
            })
            .unzip();
        Entry::Start {
            roles,
            strategies,
            propose_limit: self.propose_limit,
            vote_limit: self.vote_limit,
            quest_limit: self.quest_limit,
            assassinate_limit: self.assassinate_limit,
        }
    }
}

impl Default for Rules {
    /// Each phase waits a minute.
    fn default() -> Self {
        let minute = Duration::from_secs(60);
        Self {
            propose_limit: minute,
            vote_limit: minute,
            quest_limit: minute,
            assassinate_limit: minute,
        }
    }
}

/// What the environment and the players say to one another. Every message
/// an actor sends to others is numbered by its sender, from 1, so that the
/// pair of sender and number names a message in an episode: the
/// environment numbers its announcements, each player its choices and
/// utterances together. Each announcement is a message of its own, since
/// each player is shown a different view. A reminder an actor sends itself
/// is not numbered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// The environment opens a phase for one player, awaited in it or not.
    /// Phases are numbered from 1 for the first proposal, one up for each
    /// phase.
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
    /// The leader's team for the proposal numbered `seq`, to the
    /// environment alone.
    Propose {
        /// The number of the phase.
        seq: u64,
        /// Who proposed, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// The team.
        team: Vec<PlayerId>,
    },
    /// A player's secret vote in the phase numbered `seq`, to the
    /// environment alone.
    Vote {
        /// The number of the phase.
        seq: u64,
        /// Who voted, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// The vote.
        ballot: Ballot,
    },
    /// A member's secret deed on the quest numbered `seq`, to the
    /// environment alone.
    Perform {
        /// The number of the phase.
        seq: u64,
        /// Who did it, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// The deed.
        deed: Deed,
    },
    /// The Assassin's naming of Merlin in the phase numbered `seq`, to the
    /// environment alone.
    Select {
        /// The number of the phase.
        seq: u64,
        /// Who named, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// Whom they named.
        target: PlayerId,
    },
    /// A player's utterance in the phase numbered `seq`, as the speaker
    /// knew it: the latest phase the speaker was announced. It goes to
    /// every other player, and never to the environment, which does
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
            | Message::Propose { n, .. }
            | Message::Vote { n, .. }
            | Message::Perform { n, .. }
            | Message::Select { n, .. }
            | Message::Say { n, .. } => Some(*n),
            Message::EndPhase { .. } | Message::Wake | Message::Remind { .. } => None,
        }
    }

    /// The phase this message is tagged with: for a message to others, the
    /// phase its sender was in; for a reminder, the phase it is for. A
    /// wake-up names none.
    pub fn seq(&self) -> Option<u64> {
        match self {
            Message::Announce { seq, .. }
            | Message::Propose { seq, .. }
            | Message::Vote { seq, .. }
            | Message::Perform { seq, .. }
            | Message::Select { seq, .. }
            | Message::Say { seq, .. }
            | Message::EndPhase { seq }
            | Message::Remind { seq, .. } => Some(*seq),
            Message::Wake => None,
        }
    }

    /// Who sent this message: the environment, unless it is a player's
    /// choice or utterance, which carries its sender.
    pub fn sender(&self) -> ActorId {
        match self {
            Message::Propose { from, .. }
            | Message::Vote { from, .. }
            | Message::Perform { from, .. }
            | Message::Select { from, .. }
            | Message::Say { from, .. } => from.clone(),
            _ => ENVIRONMENT.to_string(),
        }
    }

    /// The choice this message makes, with the phase it is for and its
    /// sender's number for it, if it is a choice.
    pub fn choice(&self) -> Option<(u64, u64, Choice)> {
        match self {
            Message::Propose { seq, n, team, .. } => Some((*seq, *n, Choice::Team(team.clone()))),
            Message::Vote { seq, n, ballot, .. } => Some((*seq, *n, Choice::Ballot(*ballot))),
            Message::Perform { seq, n, deed, .. } => Some((*seq, *n, Choice::Deed(*deed))),
            Message::Select { seq, n, target, .. } => {
                Some((*seq, *n, Choice::Target(target.clone())))
            }
            _ => None,
        }
    }
}

/// Whom `me` sends `message` to, by the rules of the game: an utterance
/// to every other player, and never to the environment, which does
/// nothing with speech; anything else, a choice above all, to the
/// environment alone, since every choice is secret.
pub fn receivers(message: &Message, me: &PlayerId, view: &View) -> HashSet<ActorId> {
    match message {
        Message::Say { .. } => to_the_players_but(me, &view.players),
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

/// What the log holds: the deal, the configuration, every message sent as
/// the action of its sender and the observation of its receiver, each
/// message a model player withheld because its phase had ended, each
/// failure of a model player to act, how each phase came out, and the
/// result. Every actor logs its own actions and observations: the
/// environment logs the deal, the configuration, how each phase came out,
/// and the result besides, and a player what it withholds and its
/// failures. Every phase ends with exactly one of
/// [`Entry::Proposed`], [`Entry::Tallied`], [`Entry::Quested`] and
/// [`Entry::Assassinated`]. Every entry made during a phase names the
/// phase in a field, so nothing about phases depends on the order entries
/// reach the log or on their times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    /// The game begins: who plays what, with which strategy, and how long
    /// each phase waits for the players. The limits are written as
    /// `humantime` writes them, such as `"60s"`.
    Start {
        /// Every player's role.
        roles: HashMap<PlayerId, Role>,
        /// The name of every player's strategy.
        strategies: HashMap<PlayerId, String>,
        /// How long a proposal waits for its leader.
        #[serde(with = "humantime_serde")]
        propose_limit: Duration,
        /// How long a vote waits for the players.
        #[serde(with = "humantime_serde")]
        vote_limit: Duration,
        /// How long a quest waits for its team.
        #[serde(with = "humantime_serde")]
        quest_limit: Duration,
        /// How long the assassination waits for the Assassin.
        #[serde(with = "humantime_serde")]
        assassinate_limit: Duration,
    },
    /// The configuration the game ran under, logged right after the deal.
    Configuration(Box<config::Configuration>),
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
        /// model made: the environment's, or a random player's.
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
    /// `from` did not send `message`, which a model made from `seen`
    /// history entries, having forgotten `forgot` lines of talk, because
    /// the phase it was made in had ended before the model answered. It was
    /// never sent, so it has no number: its `n` is 0, and the player does
    /// not remember it.
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
    /// not reached, or answered with something that is not a well-formed
    /// response.
    Failed {
        /// Who failed.
        player: PlayerId,
        /// The phase it was in.
        seq: u64,
        /// What went wrong, as the error says it, causes included.
        error: String,
    },
    /// The proposal numbered `seq` ended: the leader's team, or none when
    /// the leader ran out of time.
    Proposed {
        /// The number of the phase.
        seq: u64,
        /// The team proposed, if any.
        team: Option<Vec<PlayerId>>,
    },
    /// The vote numbered `seq` ended.
    Tallied {
        /// The number of the phase.
        seq: u64,
        /// How it came out.
        tally: Tally,
    },
    /// The quest numbered `seq` ended.
    Quested {
        /// The number of the phase.
        seq: u64,
        /// How it came out.
        result: QuestResult,
    },
    /// The assassination numbered `seq` ended: whom the Assassin named, if
    /// anyone, and whether it was Merlin.
    Assassinated {
        /// The number of the phase.
        seq: u64,
        /// Whom the Assassin named, or nobody by the limit.
        target: Option<PlayerId>,
        /// Whether the Assassin named Merlin.
        merlin: bool,
    },
    /// The game is over.
    End {
        /// Who won.
        winner: Team,
        /// How.
        by: Victory,
        /// How long the whole game took, from the deal to the end, written
        /// as `humantime` writes it.
        #[serde(with = "humantime_serde")]
        took: Duration,
    },
}

/// How a game was won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Victory {
    /// Three quests succeeded, and the Assassin did not name Merlin.
    Quests,
    /// Three quests failed.
    Sabotage,
    /// Five proposals in a row failed.
    Rejections,
    /// The Assassin named Merlin.
    Assassination,
}

/// What a player is shown of the game: its partial view of the state,
/// everything in it that the player may know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Quests count from one, up to five.
    quest: NonZero<u8>,
    /// Which proposal of the quest this is, from one to five: the failed
    /// proposals in a row, plus one.
    proposal: NonZero<u8>,
    /// The phase underway.
    phase: Phase,
    /// Who leads.
    leader: PlayerId,
    /// How many players the quest needs.
    size: usize,
    /// How many sabotages fail the quest.
    sabotages_needed: usize,
    /// The team proposed, in the vote and the quest.
    team: Option<Vec<PlayerId>>,
    /// The vote that just ended, in the phase right after it.
    tally: Option<Tally>,
    /// Every quest so far.
    record: Vec<QuestResult>,
    /// The player's own role.
    role: Role,
    /// What the player sees of the others.
    sees: HashMap<PlayerId, Seen>,
    /// Everyone at the table, in seat order.
    players: Vec<PlayerId>,
}

/// The players `me` may name as Merlin from what it sees in `view`: every
/// other player it does not see as evil, in seat order.
pub fn candidates(me: &PlayerId, view: &View) -> Vec<PlayerId> {
    view.players
        .iter()
        .filter(|player| *player != me && view.sees.get(*player) != Some(&Seen::Evil))
        .cloned()
        .collect()
}

/// Whether the phase `view` shows waits on `me`: a proposal on the leader,
/// a vote on everyone, a quest on its team, and the assassination on the
/// Assassin. The rule is the one the environment goes by, read from the
/// role `view` shows `me` as its own, so the two cannot disagree.
pub fn awaited(me: &PlayerId, view: &View) -> bool {
    view.phase
        .awaits(me, view.role, &view.leader, view.team.as_ref())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::strategy::{RANDOM, Random, Strategy};
    use config::Configuration;
    use free_agent::Event;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio::time::Instant;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// The default table, seated `player1` to `player5`: Merlin in the
    /// first seat, the Assassin in the second, the Servants in the third
    /// and fourth, and the Minion in the fifth.
    pub(crate) fn five() -> HashMap<PlayerId, Role> {
        seated(&[
            Role::Merlin,
            Role::Assassin,
            Role::Servant,
            Role::Servant,
            Role::Minion,
        ])
    }

    /// `roles`, seated in order from `player1`.
    pub(crate) fn seated(roles: &[Role]) -> HashMap<PlayerId, Role> {
        roles
            .iter()
            .enumerate()
            .map(|(seat, role)| (format!("player{}", seat + 1), *role))
            .collect()
    }

    #[test]
    fn a_team_is_as_big_as_its_quest_and_table_say() {
        let sizes =
            |players| (1..=5).map(move |quest| team_size(players, NonZero::new(quest).unwrap()));
        assert!(sizes(5).eq([2, 3, 2, 3, 3]));
        assert!(sizes(6).eq([2, 3, 4, 3, 4]));
        assert!(sizes(7).eq([2, 3, 3, 4, 4]));
        for players in 8..=10 {
            assert!(sizes(players).eq([3, 4, 4, 5, 5]), "{players}");
        }
    }

    #[test]
    fn two_sabotages_fail_the_fourth_quest_with_seven_or_more_players_and_one_any_other() {
        for players in 5..=10 {
            for quest in 1..=5 {
                let needed = sabotages_needed(players, NonZero::new(quest).unwrap());
                let two = quest == 4 && players >= 7;
                assert_eq!(needed, if two { 2 } else { 1 }, "{players}, {quest}");
            }
        }
    }

    /// What the player in `seat` of a table of `roles` sees, in seat
    /// order.
    fn sight(roles: &[Role], seat: usize) -> Vec<(String, Seen)> {
        let state = State::new(seated(roles));
        let sees = state.sees(&format!("player{seat}"));
        in_order(sees.keys())
            .into_iter()
            .map(|player| (player.clone(), sees[player]))
            .collect()
    }

    /// The players named, as seen `seen`.
    fn as_seen(players: &[&str], seen: Seen) -> Vec<(String, Seen)> {
        players.iter().map(|player| (id(player), seen)).collect()
    }

    /// Every role at one table: Merlin, Percival, a Servant, the Assassin,
    /// Morgana, Mordred, Oberon and a Minion, from `player1` to `player8`.
    const EVERYONE: [Role; 8] = [
        Role::Merlin,
        Role::Percival,
        Role::Servant,
        Role::Assassin,
        Role::Morgana,
        Role::Mordred,
        Role::Oberon,
        Role::Minion,
    ];

    #[test]
    fn merlin_sees_every_evil_player_but_mordred() {
        let evil = as_seen(&["player4", "player5", "player7", "player8"], Seen::Evil);
        assert_eq!(sight(&EVERYONE, 1), evil);
    }

    #[test]
    fn percival_sees_merlin_and_morgana_alike() {
        let seen = as_seen(&["player1", "player5"], Seen::MerlinOrMorgana);
        assert_eq!(sight(&EVERYONE, 2), seen);
    }

    #[test]
    fn evil_but_oberon_sees_the_other_evil_players_but_oberon() {
        for seat in [4, 5, 6, 8] {
            let evil: Vec<String> = [4, 5, 6, 8]
                .into_iter()
                .filter(|other| *other != seat)
                .map(|other| format!("player{other}"))
                .collect();
            let evil: Vec<&str> = evil.iter().map(String::as_str).collect();
            assert_eq!(sight(&EVERYONE, seat), as_seen(&evil, Seen::Evil), "{seat}");
        }
    }

    #[test]
    fn oberon_and_the_servants_see_nobody() {
        assert!(sight(&EVERYONE, 3).is_empty());
        assert!(sight(&EVERYONE, 7).is_empty());
    }

    #[test]
    fn a_role_is_named_and_counted_and_belongs_to_a_team() {
        let named = EVERYONE.map(|role| role.to_string());
        assert_eq!(
            named,
            [
                "Merlin", "Percival", "Servant", "Assassin", "Morgana", "Mordred", "Oberon",
                "Minion"
            ]
        );
        let teams = EVERYONE.map(Role::team);
        assert_eq!(teams[..3], [Team::Good; 3]);
        assert_eq!(teams[3..], [Team::Evil; 5]);
        assert_eq!(Team::Good.to_string(), "good");
        assert_eq!(Team::Evil.to_string(), "evil");
        let unique: Vec<Role> = EVERYONE.into_iter().filter(|role| !role.unique()).collect();
        assert_eq!(unique, [Role::Servant, Role::Minion]);
    }

    #[test]
    fn role_counts_give_each_roles_count_under_the_flag_clap_derives() {
        let counts = RoleCounts {
            servants: Some(3),
            oberon: Some(1),
            ..RoleCounts::default()
        };
        let got = Table::default().counts().map(|(role, _)| counts.get(role));
        assert_eq!(got, [None, None, Some(3), None, None, None, Some(1), None]);
        let all = RoleCounts {
            merlin: Some(1),
            percival: Some(2),
            servants: Some(3),
            assassin: Some(4),
            morgana: Some(5),
            mordred: Some(6),
            oberon: Some(7),
            minions: Some(8),
        };
        let got = EVERYONE.map(|role| all.get(role).unwrap());
        assert_eq!(got, [1, 2, 3, 4, 5, 6, 7, 8]);
        let flags: Vec<String> = RoleCounts::augment_args(clap::Command::new("avalon"))
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(|long| format!("--{long}")))
            .collect();
        let named = EVERYONE.map(RoleCounts::flag);
        assert_eq!(flags, named);
        assert_eq!(named[2], "--servants");
    }

    #[test]
    fn a_table_seats_five_to_ten_at_most_one_of_each_role_but_servant_and_minion_and_merlin_with_an_assassin()
     {
        let error = |table: Table| table.check().unwrap_err().to_string();
        assert!(Table::default().check().is_ok());
        assert_eq!(
            error(Table {
                servants: 1,
                ..Table::default()
            }),
            "Avalon is played by 5 to 10 players, and this table seats 4"
        );
        assert_eq!(
            error(Table {
                servants: 8,
                ..Table::default()
            }),
            "Avalon is played by 5 to 10 players, and this table seats 11"
        );
        let ten = Table {
            servants: 7,
            ..Table::default()
        };
        assert!(ten.check().is_ok());
        assert_eq!(ten.players(), 10);
        assert_eq!(
            error(Table {
                morgana: 2,
                ..Table::default()
            }),
            "a table has at most one Morgana, and this one has 2"
        );
        assert!(
            Table {
                servants: 4,
                minions: 3,
                ..Table::default()
            }
            .check()
            .is_ok()
        );
        assert_eq!(
            error(Table {
                merlin: 0,
                servants: 3,
                ..Table::default()
            }),
            "a table has Merlin exactly when it has an Assassin, and this one has 0 Merlin and 1 Assassin"
        );
        // Neither is fine, and so is any split between the teams.
        assert!(
            Table {
                merlin: 0,
                assassin: 0,
                servants: 0,
                minions: 5,
                ..Table::default()
            }
            .check()
            .is_ok()
        );
    }

    #[test]
    fn the_start_carries_the_limits_as_humantime_writes_them_and_survives_a_trip_through_json() {
        let rules = Rules {
            propose_limit: Duration::from_secs(90),
            ..Rules::default()
        };
        let start = rules.start(&dealt(&five(), RANDOM));
        let json = serde_json::to_value(&start).unwrap();
        assert_eq!(json["Start"]["roles"]["player1"], "Merlin");
        assert_eq!(json["Start"]["strategies"]["player1"], "random");
        assert_eq!(json["Start"]["propose_limit"], "1m 30s");
        assert_eq!(json["Start"]["vote_limit"], "1m");
        assert_eq!(json["Start"]["quest_limit"], "1m");
        assert_eq!(json["Start"]["assassinate_limit"], "1m");
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, start);
    }

    #[test]
    fn an_entry_survives_a_trip_through_json() {
        let state = State::new(five());
        let entries = [
            Entry::Action {
                from: ENVIRONMENT.to_string(),
                n: 2,
                to: HashSet::from([id("player2")]),
                message: announcement(1, state.view(&id("player2"))),
                seen: None,
                forgot: None,
            },
            Entry::Action {
                from: id("player1"),
                n: 1,
                to: to_the_environment(),
                message: Choice::Team(vec![id("player1"), id("player3")]).message(
                    1,
                    id("player1"),
                    1,
                ),
                seen: None,
                forgot: Some(2),
            },
            Entry::Observation {
                by: ENVIRONMENT.to_string(),
                from: id("player1"),
                n: 1,
                seq: 1,
            },
            Entry::Withheld {
                from: id("player2"),
                message: utterance(2, "player2", 0, "Too late."),
                seen: Some(7),
                forgot: None,
            },
            Entry::Failed {
                player: id("player3"),
                seq: 3,
                error: "connection refused".to_string(),
            },
            Entry::Proposed { seq: 1, team: None },
            Entry::Proposed {
                seq: 2,
                team: Some(vec![id("player2"), id("player3")]),
            },
            Entry::Tallied {
                seq: 3,
                tally: Tally::of(
                    HashMap::from([
                        (id("player1"), Ballot::Approve),
                        (id("player2"), Ballot::Reject),
                    ]),
                    5,
                ),
            },
            Entry::Quested {
                seq: 4,
                result: QuestResult {
                    team: vec![id("player2"), id("player3")],
                    sabotages: 1,
                    succeeded: false,
                },
            },
            Entry::Assassinated {
                seq: 20,
                target: Some(id("player1")),
                merlin: true,
            },
            Entry::Assassinated {
                seq: 20,
                target: None,
                merlin: false,
            },
            Entry::End {
                winner: Team::Evil,
                by: Victory::Assassination,
                took: Duration::from_secs(65),
            },
        ];
        for entry in entries {
            let json = serde_json::to_string(&entry).unwrap();
            let back: Entry = serde_json::from_str(&json).unwrap();
            assert_eq!(back, entry);
        }
        let json = serde_json::to_value(Entry::End {
            winner: Team::Good,
            by: Victory::Quests,
            took: Duration::from_secs(65),
        })
        .unwrap();
        assert_eq!(json["End"]["took"], "1m 5s");
    }

    #[test]
    fn an_announcement_carries_its_limit_as_humantime_writes_it() {
        let announce = Message::Announce {
            seq: 1,
            n: 1,
            view: State::new(five()).view(&id("player1")),
            limit: Duration::from_secs(90),
        };
        let json = serde_json::to_value(&announce).unwrap();
        assert_eq!(json["Announce"]["limit"], "1m 30s");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, announce);
    }

    /// One message of every kind a player sends to another, each the
    /// phase's number 3 and the sender's number 5.
    fn sent_by_ann() -> [Message; 5] {
        [
            Choice::Team(vec![id("bob")]).message(3, id("ann"), 5),
            Choice::Ballot(Ballot::Approve).message(3, id("ann"), 5),
            Choice::Deed(Deed::Sabotage).message(3, id("ann"), 5),
            Choice::Target(id("bob")).message(3, id("ann"), 5),
            utterance(3, "ann", 5, "Hello."),
        ]
    }

    #[test]
    fn a_message_to_others_is_numbered_and_names_its_phase_and_a_reminder_is_not() {
        let view = State::new(five()).view(&id("player1"));
        let announced = announcement(2, view);
        assert_eq!(announced.number(), Some(2));
        assert_eq!(announced.seq(), Some(2));
        assert_eq!(announced.sender(), ENVIRONMENT);
        for message in sent_by_ann() {
            assert_eq!(message.number(), Some(5), "{message:?}");
            assert_eq!(message.seq(), Some(3), "{message:?}");
            assert_eq!(message.sender(), "ann", "{message:?}");
        }
        assert_eq!(Message::EndPhase { seq: 1 }.number(), None);
        assert_eq!(Message::EndPhase { seq: 1 }.seq(), Some(1));
        assert_eq!(Message::Wake.number(), None);
        assert_eq!(Message::Wake.seq(), None);
        assert_eq!(Message::Remind { seq: 4, count: 1 }.number(), None);
        assert_eq!(Message::Remind { seq: 4, count: 1 }.seq(), Some(4));
        assert_eq!(Message::Remind { seq: 4, count: 1 }.sender(), ENVIRONMENT);
    }

    #[test]
    fn a_choice_is_the_message_that_makes_it_and_nothing_else_is_one() {
        let choices = [
            Choice::Team(vec![id("bob")]),
            Choice::Ballot(Ballot::Reject),
            Choice::Deed(Deed::Support),
            Choice::Target(id("bob")),
        ];
        for choice in choices {
            let message = choice.clone().message(3, id("ann"), 5);
            assert_eq!(message.choice(), Some((3, 5, choice)));
        }
        assert_eq!(utterance(3, "ann", 5, "Hello.").choice(), None);
        assert_eq!(Message::Wake.choice(), None);
    }

    #[test]
    fn talk_goes_to_the_other_players_and_every_choice_to_the_environment_alone() {
        let view = State::new(five()).view(&id("player1"));
        let me = id("player1");
        let [propose, vote, perform, select, say] = sent_by_ann();
        let others: HashSet<PlayerId> = ["player2", "player3", "player4", "player5"].map(id).into();
        assert_eq!(receivers(&say, &me, &view), others);
        for choice in [propose, vote, perform, select] {
            assert_eq!(receivers(&choice, &me, &view), to_the_environment());
        }
    }

    #[test]
    fn the_first_seat_leads_the_first_proposal_of_the_first_quest() {
        let state = State::new(five());
        let view = state.view(&id("player3"));
        assert_eq!(view.quest.get(), 1);
        assert_eq!(view.proposal.get(), 1);
        assert_eq!(view.phase, Phase::Propose);
        assert_eq!(view.leader, "player1");
        assert_eq!(view.size, 2);
        assert_eq!(view.sabotages_needed, 1);
        assert_eq!(view.team, None);
        assert_eq!(view.tally, None);
        assert!(view.record.is_empty());
        assert_eq!(view.role, Role::Servant);
        assert!(view.sees.is_empty());
        assert_eq!(
            view.players,
            ["player1", "player2", "player3", "player4", "player5"]
        );
        assert_eq!(sorted(state.awaited()), ["player1"]);
    }

    /// `choices`, each player beside its choice.
    fn choices(choices: &[(&str, Choice)]) -> Choices {
        choices
            .iter()
            .map(|(player, choice)| (id(player), choice.clone()))
            .collect()
    }

    /// The team of the players named.
    fn team(players: &[&str]) -> Vec<PlayerId> {
        players.iter().map(|player| id(player)).collect()
    }

    /// Ballots: each player named in `approve` approves, and each in
    /// `reject` rejects.
    fn ballots(approve: &[&str], reject: &[&str]) -> Choices {
        let approve = approve
            .iter()
            .map(|p| (*p, Choice::Ballot(Ballot::Approve)));
        let reject = reject.iter().map(|p| (*p, Choice::Ballot(Ballot::Reject)));
        choices(&approve.chain(reject).collect::<Vec<_>>())
    }

    /// Resolve the phase underway in `state` from `chosen`, and move on.
    fn resolve(state: &mut State, chosen: &Choices) -> Entry {
        let entry = state.resolve(chosen, 1);
        if state.winner().is_none() {
            state.next();
        }
        entry
    }

    /// Put `players` on the table in `state` and send them on the quest
    /// with everyone approving, where `sabotage` sabotage.
    fn go_on(state: &mut State, players: &[&str], sabotage: &[&str]) -> Entry {
        let everyone: Vec<String> = state.seats.clone();
        let everyone: Vec<&str> = everyone.iter().map(String::as_str).collect();
        resolve(
            state,
            &choices(&[(state.leader().clone().as_str(), Choice::Team(team(players)))]),
        );
        resolve(state, &ballots(&everyone, &[]));
        let deeds: Vec<(&str, Choice)> = sabotage
            .iter()
            .map(|player| (*player, Choice::Deed(Deed::Sabotage)))
            .collect();
        resolve(state, &choices(&deeds))
    }

    #[test]
    fn a_proposal_puts_the_team_on_the_table_for_the_vote() {
        let mut state = State::new(five());
        let proposed = resolve(
            &mut state,
            &choices(&[("player1", Choice::Team(team(&["player1", "player3"])))]),
        );
        assert_eq!(
            proposed,
            Entry::Proposed {
                seq: 1,
                team: Some(team(&["player1", "player3"]))
            }
        );
        assert_eq!(state.phase, Phase::Vote);
        let view = state.view(&id("player4"));
        assert_eq!(view.team, Some(team(&["player1", "player3"])));
        assert_eq!(view.proposal.get(), 1);
        assert_eq!(sorted(state.awaited()).len(), 5);
    }

    #[test]
    fn a_lapsed_proposal_fails_and_passes_the_lead() {
        let mut state = State::new(five());
        assert_eq!(
            resolve(&mut state, &Choices::new()),
            Entry::Proposed { seq: 1, team: None }
        );
        assert_eq!(state.phase, Phase::Propose);
        let view = state.view(&id("player4"));
        assert_eq!(view.leader, "player2");
        assert_eq!(view.proposal.get(), 2);
        assert_eq!(view.quest.get(), 1);
        assert_eq!(view.tally, None);
        assert_eq!(sorted(state.awaited()), ["player2"]);
    }

    #[test]
    fn the_lead_passes_in_seat_order_and_wraps_around() {
        // Ten seats, so that player10 comes after player9 and before
        // player1, and lapses that leave the count short of five.
        let mut roles = seated(&[Role::Servant; 10]);
        roles.insert(id("player1"), Role::Merlin);
        roles.insert(id("player2"), Role::Assassin);
        let mut state = State::new(roles);
        let mut leaders = vec![];
        for _ in 0..3 {
            for _ in 0..4 {
                leaders.push(state.leader().clone());
                resolve(&mut state, &Choices::new());
            }
            // A quest starts the count of failed proposals over.
            leaders.push(state.leader().clone());
            go_on(&mut state, &["player1", "player2", "player3"], &[]);
            assert_eq!(state.failed, 0);
        }
        let expected: Vec<String> = (0..15).map(|i| format!("player{}", i % 10 + 1)).collect();
        assert_eq!(leaders, expected);
    }

    #[test]
    fn five_failed_proposals_in_a_row_win_for_evil() {
        let mut state = State::new(five());
        for _ in 0..2 {
            resolve(&mut state, &Choices::new());
        }
        for _ in 0..2 {
            let leader = state.leader().clone();
            resolve(
                &mut state,
                &choices(&[(leader.as_str(), Choice::Team(team(&["player1", "player2"])))]),
            );
            resolve(&mut state, &ballots(&["player1", "player2"], &["player3"]));
            assert_eq!(state.winner(), None);
        }
        assert_eq!(state.view(&id("player1")).proposal.get(), 5);
        resolve(&mut state, &Choices::new());
        assert_eq!(state.winner(), Some((Team::Evil, Victory::Rejections)));
    }

    #[test]
    fn a_team_goes_on_a_strict_majority_of_all_the_players_and_a_missing_vote_does_not_approve() {
        // Two of five approve, two reject, and one does not vote.
        let mut state = State::new(five());
        resolve(
            &mut state,
            &choices(&[("player1", Choice::Team(team(&["player1", "player2"])))]),
        );
        let tallied = resolve(
            &mut state,
            &ballots(&["player1", "player2"], &["player3", "player4"]),
        );
        let tally = Tally {
            ballots: HashMap::from([
                (id("player1"), Ballot::Approve),
                (id("player2"), Ballot::Approve),
                (id("player3"), Ballot::Reject),
                (id("player4"), Ballot::Reject),
            ]),
            approved: false,
        };
        assert_eq!(
            tallied,
            Entry::Tallied {
                seq: 1,
                tally: tally.clone()
            }
        );
        // The tally is shown in the next proposal, and no team.
        assert_eq!(state.phase, Phase::Propose);
        let view = state.view(&id("player5"));
        assert_eq!(view.tally, Some(tally));
        assert_eq!(view.team, None);
        assert_eq!(view.leader, "player2");
        assert_eq!(view.proposal.get(), 2);
        // Three of five approve: the team goes, and the tally is shown on
        // the quest, with the team.
        resolve(
            &mut state,
            &choices(&[("player2", Choice::Team(team(&["player2", "player3"])))]),
        );
        assert_eq!(state.view(&id("player5")).tally, None);
        resolve(
            &mut state,
            &ballots(&["player1", "player2", "player3"], &[]),
        );
        assert_eq!(state.phase, Phase::Quest);
        let view = state.view(&id("player5"));
        assert!(view.tally.as_ref().unwrap().approved);
        assert_eq!(view.team, Some(team(&["player2", "player3"])));
        assert_eq!(sorted(state.awaited()), ["player2", "player3"]);
        // And with six players, three is not enough.
        let six = Tally::of(
            ballots(&["a", "b", "c"], &[])
                .into_iter()
                .map(|(player, choice)| (player, choice.ballot().unwrap()))
                .collect(),
            6,
        );
        assert!(!six.approved);
    }

    #[test]
    fn a_quest_fails_on_its_sabotages_and_a_missing_deed_supports() {
        let mut state = State::new(five());
        let quested = go_on(&mut state, &["player2", "player3"], &["player2"]);
        assert_eq!(
            quested,
            Entry::Quested {
                seq: 1,
                result: QuestResult {
                    team: team(&["player2", "player3"]),
                    sabotages: 1,
                    succeeded: false,
                }
            }
        );
        // Nobody acts: the quest succeeds.
        let quested = go_on(&mut state, &["player2", "player3", "player4"], &[]);
        assert_eq!(
            quested,
            Entry::Quested {
                seq: 1,
                result: QuestResult {
                    team: team(&["player2", "player3", "player4"]),
                    sabotages: 0,
                    succeeded: true,
                }
            }
        );
        // The next proposal is the third quest's, the record shows both,
        // and nobody sees who sabotaged.
        let view = state.view(&id("player3"));
        assert_eq!(view.quest.get(), 3);
        assert_eq!(view.phase, Phase::Propose);
        assert_eq!(view.record.len(), 2);
        assert!(!view.record[0].succeeded);
        assert_eq!(view.tally, None);
        assert_eq!(view.leader, "player3");
    }

    #[test]
    fn the_fourth_quest_with_seven_players_needs_two_sabotages_to_fail() {
        let roles = seated(&[
            Role::Merlin,
            Role::Assassin,
            Role::Minion,
            Role::Servant,
            Role::Servant,
            Role::Servant,
            Role::Minion,
        ]);
        let mut state = State::new(roles.clone());
        go_on(&mut state, &["player1", "player4"], &[]);
        go_on(&mut state, &["player2", "player4", "player5"], &["player2"]);
        go_on(&mut state, &["player1", "player4", "player5"], &[]);
        assert_eq!(state.view(&id("player1")).sabotages_needed, 2);
        let one = go_on(
            &mut state,
            &["player2", "player4", "player5", "player6"],
            &["player2"],
        );
        assert!(matches!(
            one,
            Entry::Quested {
                result: QuestResult {
                    succeeded: true,
                    sabotages: 1,
                    ..
                },
                ..
            }
        ));
        assert_eq!(state.phase, Phase::Assassinate);
        // Two would have failed it.
        let mut state = State::new(roles);
        state.quest = NonZero::new(4).unwrap();
        let two = go_on(
            &mut state,
            &["player2", "player3", "player4", "player5"],
            &["player2", "player3"],
        );
        assert!(matches!(
            two,
            Entry::Quested {
                result: QuestResult {
                    succeeded: false,
                    sabotages: 2,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn three_failed_quests_win_for_evil() {
        let mut state = State::new(five());
        for quest in 1..=3 {
            assert_eq!(state.winner(), None, "{quest}");
            go_on(&mut state, &["player2", "player3"], &["player2"]);
        }
        assert_eq!(state.winner(), Some((Team::Evil, Victory::Sabotage)));
    }

    #[test]
    fn three_successful_quests_go_to_the_assassination_where_naming_merlin_wins_for_evil() {
        let mut state = State::new(five());
        go_on(&mut state, &["player1", "player3"], &[]);
        go_on(&mut state, &["player2", "player3", "player4"], &["player2"]);
        go_on(&mut state, &["player1", "player3"], &[]);
        go_on(&mut state, &["player1", "player3", "player4"], &[]);
        assert_eq!(state.winner(), None);
        assert_eq!(state.phase, Phase::Assassinate);
        let view = state.view(&id("player2"));
        assert_eq!(view.quest.get(), 4);
        assert_eq!(view.record.len(), 4);
        assert_eq!(sorted(state.awaited()), ["player2"]);
        let named = resolve(
            &mut state,
            &choices(&[("player2", Choice::Target(id("player1")))]),
        );
        assert_eq!(
            named,
            Entry::Assassinated {
                seq: 1,
                target: Some(id("player1")),
                merlin: true
            }
        );
        assert_eq!(state.winner(), Some((Team::Evil, Victory::Assassination)));
    }

    /// A game of the default table at its assassination.
    fn at_the_assassination() -> State {
        let mut state = State::new(five());
        for _ in 0..3 {
            go_on(&mut state, &["player1", "player3"], &[]);
        }
        assert_eq!(state.phase, Phase::Assassinate);
        state
    }

    #[test]
    fn naming_anyone_else_or_nobody_wins_for_good() {
        let mut state = at_the_assassination();
        let named = resolve(
            &mut state,
            &choices(&[("player2", Choice::Target(id("player3")))]),
        );
        assert!(matches!(named, Entry::Assassinated { merlin: false, .. }));
        assert_eq!(state.winner(), Some((Team::Good, Victory::Quests)));
        let mut state = at_the_assassination();
        let named = resolve(&mut state, &Choices::new());
        assert_eq!(
            named,
            Entry::Assassinated {
                seq: 1,
                target: None,
                merlin: false
            }
        );
        assert_eq!(state.winner(), Some((Team::Good, Victory::Quests)));
    }

    #[test]
    fn without_merlin_three_successful_quests_win_for_good() {
        let mut state = State::new(seated(&[
            Role::Servant,
            Role::Servant,
            Role::Servant,
            Role::Minion,
            Role::Minion,
        ]));
        for _ in 0..3 {
            go_on(&mut state, &["player1", "player2"], &[]);
        }
        assert_eq!(state.winner(), Some((Team::Good, Victory::Quests)));
    }

    #[test]
    fn a_choice_is_legal_only_if_it_is_of_the_phases_kind_and_the_rules_allow_it() {
        let mut state = State::new(five());
        let me = id("player1");
        let legal = |state: &State, from: &str, choice: Choice| state.legal(&id(from), &choice);
        // A proposal takes a team of the size the quest needs, of distinct
        // players at the table.
        assert!(legal(
            &state,
            "player1",
            Choice::Team(team(&["player1", "player2"]))
        ));
        assert!(!legal(&state, "player1", Choice::Team(team(&["player1"]))));
        assert!(!legal(
            &state,
            "player1",
            Choice::Team(team(&["player1", "player2", "player3"]))
        ));
        assert!(!legal(
            &state,
            "player1",
            Choice::Team(team(&["player1", "player1"]))
        ));
        assert!(!legal(
            &state,
            "player1",
            Choice::Team(team(&["player1", "nobody"]))
        ));
        assert!(!legal(&state, "player1", Choice::Ballot(Ballot::Approve)));
        resolve(
            &mut state,
            &choices(&[("player1", Choice::Team(team(&["player1", "player2"])))]),
        );
        // A vote takes a ballot.
        assert!(legal(&state, "player3", Choice::Ballot(Ballot::Reject)));
        assert!(!legal(&state, "player3", Choice::Deed(Deed::Support)));
        resolve(
            &mut state,
            &ballots(&["player1", "player2", "player3"], &[]),
        );
        // A quest takes a deed, and only an evil player may sabotage.
        assert!(legal(&state, "player1", Choice::Deed(Deed::Support)));
        assert!(!legal(&state, "player1", Choice::Deed(Deed::Sabotage)));
        assert!(legal(&state, "player2", Choice::Deed(Deed::Sabotage)));
        assert!(!legal(&state, "player2", Choice::Target(me.clone())));
        // The assassination takes another player at the table that the
        // Assassin does not see as evil.
        let state = at_the_assassination();
        assert!(legal(&state, "player2", Choice::Target(id("player1"))));
        assert!(legal(&state, "player2", Choice::Target(id("player3"))));
        assert!(!legal(&state, "player2", Choice::Target(id("player2"))));
        assert!(!legal(&state, "player2", Choice::Target(id("player5"))));
        assert!(!legal(&state, "player2", Choice::Target(id("nobody"))));
        assert!(!legal(
            &state,
            "player2",
            Choice::Team(team(&["player1", "player3"]))
        ));
    }

    #[test]
    fn a_phase_awaits_the_leader_everyone_the_team_or_the_assassin_as_each_player_sees_it() {
        // The environment and each player read the one rule, so whoever
        // the environment awaits sees itself awaited, and nobody else does.
        let mut state = State::new(five());
        let check = |state: &State| {
            let awaited = state.awaited();
            for player in &state.seats {
                let view = state.view(player);
                assert_eq!(
                    super::awaited(player, &view),
                    awaited.contains(player),
                    "{player} in {:?}",
                    state.phase
                );
            }
        };
        check(&state);
        resolve(
            &mut state,
            &choices(&[("player1", Choice::Team(team(&["player1", "player2"])))]),
        );
        check(&state);
        resolve(
            &mut state,
            &ballots(&["player1", "player2", "player3"], &[]),
        );
        check(&state);
        check(&at_the_assassination());
    }

    #[test]
    fn the_assassins_candidates_are_the_others_it_does_not_see_as_evil_in_seat_order() {
        let state = at_the_assassination();
        let me = id("player2");
        let view = state.view(&me);
        assert_eq!(candidates(&me, &view), ["player1", "player3", "player4"]);
    }

    /// `players` in name order.
    pub(crate) fn sorted(players: HashSet<PlayerId>) -> Vec<PlayerId> {
        let mut players: Vec<_> = players.into_iter().collect();
        players.sort();
        players
    }

    /// `roles`, each dealt with the strategy `strategy`.
    pub(crate) fn dealt(
        roles: &HashMap<PlayerId, Role>,
        strategy: &str,
    ) -> HashMap<PlayerId, Dealt> {
        roles
            .iter()
            .map(|(id, role)| {
                let dealt = Dealt {
                    role: *role,
                    strategy: strategy.to_string(),
                };
                (id.clone(), dealt)
            })
            .collect()
    }

    /// The configuration of a game of `roles` under `rules` with every
    /// player random.
    pub(crate) fn configured(roles: &HashMap<PlayerId, Role>, rules: &Rules) -> Configuration {
        let count = |role| roles.values().filter(|r| **r == role).count();
        Configuration {
            role_counts: EVERYONE.map(|role| (role, count(role))).into(),
            propose_limit: rules.propose_limit,
            vote_limit: rules.vote_limit,
            quest_limit: rules.quest_limit,
            assassinate_limit: rules.assassinate_limit,
            strategies: BTreeMap::from([(RANDOM.to_string(), Strategy::Random(Random {}))]),
            allotments: EVERYONE
                .map(|role| (role, BTreeMap::from([(RANDOM.to_string(), count(role))])))
                .into(),
            models: BTreeMap::new(),
            prompts: BTreeMap::new(),
            text: BTreeMap::new(),
        }
    }

    /// What a game reports: the winner, everything logged, and how long
    /// the game took.
    pub(crate) type Played = (Team, Vec<Entry>, Duration);

    /// Run `episode`, whose winner comes on `won` and whose log on `log`.
    pub(crate) async fn run(
        episode: Episode<Actor>,
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

    /// What a puppet says on being announced a phase, given its own name,
    /// the phase's number, and what it may see, awaited in the phase or
    /// not. Each message carries the number the script gives it.
    pub(crate) type Script = Box<dyn Fn(&PlayerId, u64, &View) -> Vec<Message> + Send>;

    /// The latest phase a puppet was announced: its number.
    type Announced = Arc<Mutex<u64>>;

    /// A player who says whatever its script says, at once, for a game
    /// with a player who is late, illegal, repeats itself, or talks. Its
    /// script is consulted for every phase it is announced. A message goes
    /// where the game's rules send it from a player. It logs each message
    /// it sends as its action and each message it receives as its
    /// observation, in the latest phase announced, and does nothing else
    /// with what it hears.
    pub struct Puppet {
        context: Context<Message, Entry>,
        script: Script,
        announced: Announced,
    }

    #[async_trait]
    impl Behavior for Puppet {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.context
        }

        async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
            let Some(n) = message.number() else {
                return Ok(());
            };
            let Message::Announce { seq, view, .. } = message else {
                let seq = *self.announced.lock().unwrap();
                observe(&self.context, message.sender(), n, seq);
                return Ok(());
            };
            *self.announced.lock().unwrap() = *seq;
            observe(&self.context, ENVIRONMENT.to_string(), n, *seq);
            let me = &self.context.id;
            for said in (self.script)(me, *seq, view) {
                let to = receivers(&said, me, view);
                act(&self.context, said, to, None, None)?;
            }
            Ok(())
        }
    }

    impl Actor {
        /// A puppet following `script`.
        pub(crate) fn puppet(script: Script) -> Seat {
            let behavior: Builder<Actor> = Box::new(move |context| {
                Actor::Puppet(Puppet {
                    context,
                    script,
                    announced: Announced::default(),
                })
            });
            (behavior, None)
        }
    }

    /// A player who never chooses.
    pub(crate) fn mute() -> Seat {
        Actor::puppet(Box::new(|_, _, _| vec![]))
    }

    /// Run a game of `roles` under `rules`, each player choosing at random
    /// except those in `puppets`, who follow their own scripts.
    pub(crate) async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        mut puppets: HashMap<&str, Seat>,
    ) -> anyhow::Result<Played> {
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let player = |id: &PlayerId| {
            puppets
                .remove(id.as_str())
                .unwrap_or_else(random::Player::init)
        };
        let configuration = configured(&roles, &rules);
        let episode = game(
            dealt(&roles, RANDOM),
            rules,
            configuration,
            winner,
            player,
            logger,
        );
        run(episode, won, log).await
    }

    /// Short limits, each its own, so a phase that waits one out is told
    /// apart from one that does not, and by which.
    pub(crate) fn quick() -> Rules {
        Rules {
            propose_limit: Duration::from_secs(10),
            vote_limit: Duration::from_secs(20),
            quest_limit: Duration::from_secs(30),
            assassinate_limit: Duration::from_secs(40),
        }
    }

    /// `payload` as an event logged when the game began, in no episode in
    /// particular.
    pub(crate) fn logged<L>(payload: L) -> Event<L> {
        Event {
            time: std::time::UNIX_EPOCH,
            episode: Default::default(),
            payload,
        }
    }

    /// The environment's announcement of the phase numbered `seq`, showing
    /// `view`, lasting a minute. The number is the phase's.
    pub(crate) fn announcement(seq: u64, view: View) -> Message {
        Message::Announce {
            seq,
            n: seq,
            view,
            limit: Duration::from_secs(60),
        }
    }

    /// `from`'s utterance of `text` in the phase numbered `seq`, the `n`th
    /// message `from` sent.
    pub(crate) fn utterance(seq: u64, from: &str, n: u64, text: &str) -> Message {
        Message::Say {
            seq,
            from: from.to_string(),
            n,
            text: text.to_string(),
        }
    }

    /// What the player in `seat` of the default table sees at the start of
    /// the game, moved to `phase` with `team` on the table.
    pub(crate) fn view_of(seat: &str, phase: Phase, team: &[&str]) -> View {
        let mut state = State::new(five());
        state.phase = phase;
        state.team = (!team.is_empty()).then(|| team.iter().map(|p| id(p)).collect());
        state.view(&id(seat))
    }

    /// The messages `by` logged as its observations, each as who sent it,
    /// the sender's number for it, and the phase `by` was in when it
    /// arrived, in the log's order.
    pub(crate) fn observed_by<'a>(logged: &'a [Entry], by: &str) -> Vec<(&'a str, u64, u64)> {
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

    /// The outcome entries in the log, in order.
    pub(crate) fn outcomes(logged: &[Entry]) -> Vec<&Entry> {
        logged
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    Entry::Proposed { .. }
                        | Entry::Tallied { .. }
                        | Entry::Quested { .. }
                        | Entry::Assassinated { .. }
                )
            })
            .collect()
    }

    /// How the log says the game ended: who won and how.
    pub(crate) fn ending(logged: &[Entry]) -> (Team, Victory) {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::End { winner, by, .. } => Some((*winner, *by)),
                _ => None,
            })
            .unwrap()
    }
}
