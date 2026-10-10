//! What every variant of Werewolf shares: the rules and the types they are
//! written in.
//!
//! A game is an episode of [free agents](free_agent): the rules in one
//! actor, the environment, and a player in each of the others. The
//! environment alone holds the state of the game: the round and whether
//! it is night or day, every player's [`Role`], who is alive, and what the
//! seer has learned. A player sees the game only through the
//! [`Observation`] the environment sends it, which carries the round, the
//! phase, who is alive, and the roles that player may know. A werewolf knows
//! every werewolf, the seer knows itself and whoever it has discovered, and
//! everyone else knows only itself. Holding no state, a player can only
//! talk.
//!
//! Play alternates between night and day, starting with night, until one
//! [`Team`] has won. The environment logs the deal, every message to or
//! from a player as it was, and the result, as an [`Entry`] each.
//!
//! Each variant's module holds what is particular to it: its environment,
//! its players, the actor enum its episode holds, and the function that
//! builds its episode.

pub mod llm;
pub mod report;
pub mod uniform_random;

use clap::Args;
use free_agent::ActorId;
use rand::seq::{IndexedRandom, SliceRandom};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZero;
use std::time::Duration;

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

/// How many of each role sit at the table, for a variant to flatten into its
/// command line arguments. A count left unset has no `clap` default, so
/// that a variant can tell it apart from a count that was given, and fill
/// it in from [`Table::default`] or from somewhere else.
#[derive(Args, Debug, Default)]
pub struct RoleCounts {
    /// How many werewolves [default: 2]
    #[arg(long)]
    pub werewolves: Option<usize>,
    /// How many plain villagers [default: 3]
    #[arg(long)]
    pub villagers: Option<usize>,
    /// How many doctors [default: 1]
    #[arg(long)]
    pub doctors: Option<usize>,
    /// How many seers [default: 1]
    #[arg(long)]
    pub seers: Option<usize>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Deal the roles at random to players named `player1` onward. The
    /// names say nothing about the roles, since every player sees every
    /// name.
    pub fn deal(&self) -> HashMap<PlayerId, Role> {
        let mut roles = Vec::new();
        for (role, count) in [
            (Role::Werewolf, self.werewolves),
            (Role::Villager, self.villagers),
            (Role::Doctor, self.doctors),
            (Role::Seer, self.seers),
        ] {
            roles.extend(std::iter::repeat_n(role, count));
        }
        roles.shuffle(&mut rand::rng());
        roles
            .into_iter()
            .enumerate()
            .map(|(seat, role)| (format!("player{}", seat + 1), role))
            .collect()
    }
}

impl fmt::Display for Table {
    /// The counts in the order the roles are dealt, each in its number:
    /// `2 werewolves, 3 villagers, 1 doctor, 1 seer`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let counted = [
            (self.werewolves, "werewolf", "werewolves"),
            (self.villagers, "villager", "villagers"),
            (self.doctors, "doctor", "doctors"),
            (self.seers, "seer", "seers"),
        ]
        .map(|(count, one, many)| format!("{count} {}", if count == 1 { one } else { many }));
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

    pub(super) fn village() -> State {
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
