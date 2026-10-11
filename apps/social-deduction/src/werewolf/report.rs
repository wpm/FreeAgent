//! A legible account of a game, told from its log as the log is written,
//! either of everything or of what one player saw.

use super::{Entry, Message, Observation, Phase, PlayerId, Role};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::num::NonZero;

/// Whose account a narrator gives.
#[derive(Debug, Default)]
enum Voice {
    /// Everyone's: the deal, every deed, every death.
    #[default]
    Omniscient,
    /// One player's: what it was shown and what it did, told to it.
    Player(PlayerId),
}

/// Turns the log's entries, one at a time, into lines of prose. The
/// omniscient narrator tells the table, then each night and day with what
/// every player did and how it ended, then who won. A player's narrator
/// tells the same nights and days from what that player was shown, in the
/// second person, with no table. Each names a player with its role in
/// parentheses whenever it knows the role, except in reporting its death,
/// and lists players in seat order. Both learn who died from who is shown as alive next, so a
/// phase's result is told as the next one begins, and the omniscient
/// narrator holds the phase's deeds until then too, to tell them in seat
/// order; a player's narrator tells the player's own deed as it is done. A
/// phase announced is told as one requested, and a selection as a reply,
/// unless a later phase has been announced since: then it is told as too
/// late to count, for the phase it was for.
#[derive(Debug, Default)]
pub struct Narrator {
    /// Whose account this is.
    voice: Voice,
    /// Every role known so far: from the deal, and from what players are
    /// shown.
    roles: HashMap<PlayerId, Role>,
    /// Who was alive when the current phase began.
    alive: HashSet<PlayerId>,
    /// The phase being told, once one has begun.
    phase: Option<(NonZero<u8>, Phase)>,
    /// The deeds of the phase being told, each with whose it is, held in
    /// the omniscient account until the phase ends.
    deeds: Vec<(PlayerId, String)>,
    /// Every phase announced so far, by its number.
    announced: BTreeMap<u64, (NonZero<u8>, Phase)>,
}

impl Narrator {
    /// The account `me` would give of its own game. Feed it the entries
    /// about `me`: each observation `me` was sent, and each selection it
    /// made. Entries about anyone else add nothing, so the whole log tells
    /// it the same account as `me`'s own history does. It opens with who
    /// `me` is, names the roles `me` knows and nobody else's, and tells
    /// `me`'s deeds as "You ...".
    pub fn for_player(me: PlayerId) -> Self {
        Self {
            voice: Voice::Player(me),
            ..Self::default()
        }
    }

    /// The lines `entry` adds to the account, often none.
    pub fn narrate(&mut self, entry: &Entry) -> Vec<String> {
        match entry {
            Entry::Start { variation, roles } if matches!(self.voice, Voice::Omniscient) => {
                self.roles = roles.clone();
                self.alive = roles.keys().cloned().collect();
                vec![self.table(variation)]
            }
            Entry::Sent {
                to,
                message: Message::Observation(observation),
            } if self.concerns(to) => self.show(observation),
            Entry::Sent {
                to,
                message: Message::Announce { seq, observation },
            } if self.concerns(to) => {
                self.announced
                    .insert(*seq, (observation.round, observation.phase.clone()));
                self.show(observation)
            }
            Entry::Replied {
                from,
                message: Message::Action(chosen),
            } if self.concerns(from) => self.did(from, chosen),
            Entry::Received {
                from,
                message:
                    Message::Select {
                        seq,
                        target: chosen,
                        ..
                    },
            } if self.concerns(from) => match self.late(*seq).cloned() {
                Some(phase) => vec![self.lateness(from, chosen, &phase)],
                None => self.did(from, chosen),
            },
            Entry::End {
                winner,
                days,
                survivors,
            } => {
                let mut lines = self.outcome(survivors, false);
                let seats: Vec<String> = in_order(survivors)
                    .into_iter()
                    .map(|player| self.seat(player).to_string())
                    .collect();
                lines.push(format!(
                    "The {winner} win after {days} {}. Survivors: {}.",
                    if *days == 1 { "day" } else { "days" },
                    seats.join(", ")
                ));
                lines
            }
            _ => vec![],
        }
    }

    /// Whether an entry about `player` belongs in this account: every
    /// player's does in the omniscient one, only `me`'s in a player's.
    fn concerns(&self, player: &PlayerId) -> bool {
        match &self.voice {
            Voice::Omniscient => true,
            Voice::Player(me) => me == player,
        }
    }

    /// The deal, with every player's role.
    fn table(&self, variation: &str) -> String {
        let seats: Vec<String> = in_order(self.roles.keys())
            .into_iter()
            .map(|player| self.seat(player).to_string())
            .collect();
        format!(
            "{variation} with {} players: {}.",
            self.roles.len(),
            seats.join(", ")
        )
    }

    /// `player` as this account names it: with its role when known.
    fn seat<'a>(&self, player: &'a PlayerId) -> Seat<'a> {
        Seat {
            player,
            role: self.roles.get(player).copied(),
        }
    }

    /// Takes in the roles an observation shows that were not known. In a
    /// player's account, its first observation says who it is, and a role
    /// it is shown later is one it learned.
    fn learn(&mut self, roles: &HashMap<PlayerId, Role>) -> Vec<String> {
        let new: Vec<(&PlayerId, Role)> = in_order(roles.keys())
            .into_iter()
            .filter(|player| !self.roles.contains_key(*player))
            .map(|player| (player, roles[player]))
            .collect();
        let lines = match &self.voice {
            Voice::Omniscient => vec![],
            Voice::Player(me) if self.phase.is_none() => vec![identity(me, roles)],
            Voice::Player(_) => new
                .iter()
                .map(|(player, role)| format!("You learn {player} is a {role}."))
                .collect(),
        };
        self.roles
            .extend(new.into_iter().map(|(player, role)| (player.clone(), role)));
        lines
    }

    /// What a player being shown `observation` adds to the account: the
    /// roles it learns, and the phase it begins.
    fn show(&mut self, observation: &Observation) -> Vec<String> {
        let mut lines = self.learn(&observation.roles);
        lines.extend(self.begin(observation));
        lines
    }

    /// A phase begins when a player is first shown it. The phase before
    /// it ends then, with whoever is no longer alive.
    fn begin(&mut self, observation: &Observation) -> Vec<String> {
        let phase = (observation.round, observation.phase.clone());
        if self.phase.as_ref() == Some(&phase) {
            return vec![];
        }
        let slept = self
            .phase
            .as_ref()
            .is_some_and(|told| ordinal(&phase) != ordinal(told) + 1);
        let mut lines = self.outcome(&observation.alive, slept);
        self.alive = observation.alive.clone();
        self.phase = Some(phase);
        lines.push(match observation.phase {
            Phase::Night => format!("Night {}.", observation.round),
            Phase::Day => format!("Day {}.", observation.round),
        });
        lines
    }

    /// How the phase being told ended, given who is alive after it: the
    /// deeds held for it, then who died, by name alone. A player that
    /// `slept` through phases since is told who died without being told
    /// how. Several dead take a plural verb.
    fn outcome(&mut self, alive_after: &HashSet<PlayerId>, slept: bool) -> Vec<String> {
        let mut lines = self.done();
        let Some((_, phase)) = &self.phase else {
            return lines;
        };
        let dead = in_order(self.alive.difference(alive_after));
        let listed = listed(dead.iter().map(|player| Seat::named(player)));
        let line = match (phase, slept, dead.len()) {
            (_, true, 0) => "Nobody died.".to_string(),
            (_, true, _) => format!("{listed} died."),
            (Phase::Night, false, 0) => "Nobody dies.".to_string(),
            (Phase::Night, false, 1) => format!("{listed} dies."),
            (Phase::Night, false, _) => format!("{listed} die."),
            (Phase::Day, false, 0) => "Nobody is voted out.".to_string(),
            (Phase::Day, false, 1) => format!("{listed} is voted out."),
            (Phase::Day, false, _) => format!("{listed} are voted out."),
        };
        self.alive = alive_after.clone();
        lines.push(line);
        lines
    }

    /// `player`'s deed of choosing `chosen`: told at once in the player's
    /// own account, held for the end of the phase in the omniscient one.
    fn did(&mut self, player: &PlayerId, chosen: &PlayerId) -> Vec<String> {
        let line = self.deed(player, chosen);
        match self.voice {
            Voice::Omniscient => {
                self.deeds.push((player.clone(), line));
                vec![]
            }
            Voice::Player(_) => vec![line],
        }
    }

    /// The deeds held for the phase being told, in seat order.
    fn done(&mut self) -> Vec<String> {
        let mut deeds = std::mem::take(&mut self.deeds);
        deeds.sort_by(|a, b| seat_order(&a.0, &b.0));
        deeds.into_iter().map(|(_, line)| line).collect()
    }

    /// What `player` did by choosing `chosen`, which depends on the phase
    /// and, by night, on the player's role. In a player's account it is
    /// always the player's own deed, told to it.
    fn deed(&self, player: &PlayerId, chosen: &PlayerId) -> String {
        let role = self.roles.get(player).copied();
        let phase = self.phase.as_ref().map(|(_, phase)| phase);
        let (player, chosen) = (self.seat(player), self.seat(chosen));
        match (&self.voice, phase, role) {
            (Voice::Player(_), Some(Phase::Night), Some(Role::Werewolf)) => {
                format!("You vote to kill {chosen}.")
            }
            (Voice::Player(_), Some(Phase::Night), Some(Role::Doctor)) => {
                format!("You protect {chosen}.")
            }
            (Voice::Player(_), Some(Phase::Night), Some(Role::Seer)) => {
                format!("You ask about {chosen}.")
            }
            (Voice::Player(_), _, _) => format!("You vote against {chosen}."),
            (Voice::Omniscient, Some(Phase::Night), Some(Role::Werewolf)) => {
                format!("{player} votes to kill {chosen}.")
            }
            (Voice::Omniscient, Some(Phase::Night), Some(Role::Doctor)) => {
                format!("{player} protects {chosen}.")
            }
            (Voice::Omniscient, Some(Phase::Night), Some(Role::Seer)) => match chosen.role {
                Some(seen) => format!("{player} learns {} is a {seen}.", chosen.player),
                None => format!("{player} asks about {chosen}."),
            },
            (Voice::Omniscient, _, _) => format!("{player} votes against {chosen}."),
        }
    }

    /// The phase a selection numbered `seq` was for, if a later phase has
    /// been announced since. The environment drops such a selection, so it
    /// is no deed of the phase being told.
    fn late(&self, seq: u64) -> Option<&(NonZero<u8>, Phase)> {
        let later = self.announced.range(seq + 1..).next().is_some();
        later.then(|| self.announced.get(&seq)).flatten()
    }

    /// What `player`'s choice of `chosen` for `phase` came to once a later
    /// phase had begun: nothing.
    fn lateness(
        &self,
        player: &PlayerId,
        chosen: &PlayerId,
        (round, phase): &(NonZero<u8>, Phase),
    ) -> String {
        let when = match phase {
            Phase::Night => format!("night {round}"),
            Phase::Day => format!("day {round}"),
        };
        let chosen = self.seat(chosen);
        match &self.voice {
            Voice::Player(_) => format!("You choose {chosen} for {when} too late to count."),
            Voice::Omniscient => {
                format!(
                    "{} chooses {chosen} for {when} too late to count.",
                    self.seat(player)
                )
            }
        }
    }
}

/// A player as an account names it: `player (role)` when the role is
/// known, the name alone when it is not.
#[derive(Debug, Clone, Copy)]
struct Seat<'a> {
    player: &'a PlayerId,
    role: Option<Role>,
}

impl<'a> Seat<'a> {
    /// `player` by name alone.
    fn named(player: &'a PlayerId) -> Self {
        Seat { player, role: None }
    }
}

impl fmt::Display for Seat<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.role {
            Some(role) => write!(f, "{} ({role})", self.player),
            None => f.write_str(self.player),
        }
    }
}

/// Who `me` is, from the roles its first observation shows: its own, and
/// for a werewolf the whole pack.
fn identity(me: &PlayerId, roles: &HashMap<PlayerId, Role>) -> String {
    match roles.get(me) {
        Some(Role::Werewolf) => {
            let pack = roles
                .iter()
                .filter(|(_, role)| **role == Role::Werewolf)
                .map(|(player, _)| player);
            match in_order(pack).as_slice() {
                [_] => format!("You are {me}, the only werewolf."),
                pack => format!(
                    "You are {me}, a werewolf. The werewolves are {}.",
                    listed(pack.iter().map(|player| Seat::named(player)))
                ),
            }
        }
        Some(Role::Villager) => format!("You are {me}, a villager."),
        Some(role) => format!("You are {me}, the {role}."),
        None => format!("You are {me}."),
    }
}

/// Where a phase falls in the game, so that consecutive phases are one
/// apart.
fn ordinal((round, phase): &(NonZero<u8>, Phase)) -> u16 {
    let half = match phase {
        Phase::Night => 0,
        Phase::Day => 1,
    };
    u16::from(round.get()) * 2 + half
}

/// `players` in seat order.
fn in_order<'a>(players: impl IntoIterator<Item = &'a PlayerId>) -> Vec<&'a PlayerId> {
    let mut players: Vec<&PlayerId> = players.into_iter().collect();
    players.sort_unstable_by(|a, b| seat_order(a, b));
    players
}

/// Seat order: by the number a name ends in, so that `player2` comes
/// before `player10`, and by the name where that does not tell.
fn seat_order(a: &PlayerId, b: &PlayerId) -> Ordering {
    numbered(a).cmp(&numbered(b)).then_with(|| a.cmp(b))
}

/// `name` as what comes before its number and the number, when it ends in
/// one.
fn numbered(name: &str) -> (&str, Option<u64>) {
    let prefix = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let number = name[prefix.len()..].parse().ok();
    (prefix, number)
}

/// Seats as a list in prose, in the order given.
fn listed<'a>(seats: impl IntoIterator<Item = Seat<'a>>) -> String {
    let seats: Vec<String> = seats.into_iter().map(|seat| seat.to_string()).collect();
    seats.join(" and ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::Team;
    use crate::werewolf::llm::Configuration;
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    fn roles() -> HashMap<PlayerId, Role> {
        HashMap::from([
            (id("wolf"), Role::Werewolf),
            (id("seer"), Role::Seer),
            (id("doctor"), Role::Doctor),
            (id("ann"), Role::Villager),
        ])
    }

    /// Round `round`'s `phase`, with `alive` alive and `known` roles shown.
    fn observed(round: u8, phase: Phase, alive: &[&str], known: &[(&str, Role)]) -> Observation {
        Observation {
            round: NonZero::new(round).unwrap(),
            phase,
            roles: known.iter().map(|(name, role)| (id(name), *role)).collect(),
            alive: alive.iter().map(|name| id(name)).collect(),
        }
    }

    fn seen(round: u8, phase: Phase, alive: &[&str]) -> Observation {
        observed(round, phase, alive, &[])
    }

    fn observation(round: u8, phase: Phase, alive: &[&str]) -> Message {
        Message::Observation(seen(round, phase, alive))
    }

    fn shown(to: &str, round: u8, phase: Phase, alive: &[&str]) -> Entry {
        told(to, round, phase, alive, &[])
    }

    /// `to` was shown round `round`'s `phase` with `alive` alive and `known`
    /// roles, as a player of a requesting environment is.
    fn told(to: &str, round: u8, phase: Phase, alive: &[&str], known: &[(&str, Role)]) -> Entry {
        Entry::Sent {
            to: id(to),
            message: Message::Observation(observed(round, phase, alive, known)),
        }
    }

    fn chose(from: &str, chosen: &str) -> Entry {
        Entry::Replied {
            from: id(from),
            message: Message::Action(id(chosen)),
        }
    }

    /// The environment announced the phase to `to`.
    fn announced(to: &str, seq: u64, round: u8, phase: Phase, alive: &[&str]) -> Entry {
        Entry::Sent {
            to: id(to),
            message: Message::Announce {
                seq,
                observation: seen(round, phase, alive),
            },
        }
    }

    /// `from` selected `target` in the phase numbered `seq`.
    fn selected(from: &str, seq: u64, target: &str) -> Entry {
        Entry::Received {
            from: id(from),
            message: Message::Select {
                seq,
                from: id(from),
                target: id(target),
            },
        }
    }

    fn ended(winner: Team, days: u8, survivors: &[&str]) -> Entry {
        Entry::End {
            winner,
            days,
            survivors: survivors.iter().map(|name| id(name)).collect(),
        }
    }

    fn start() -> Entry {
        Entry::Start {
            variation: "Uniform Random".to_string(),
            roles: roles(),
        }
    }

    #[test]
    fn the_table_is_told_first() {
        let mut narrator = Narrator::default();
        assert_eq!(
            narrator.narrate(&start()),
            [
                "Uniform Random with 4 players: ann (villager), doctor (doctor), seer (seer), wolf (werewolf)."
            ]
        );
    }

    #[test]
    fn a_phase_begins_when_a_player_is_first_shown_it() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone)),
            ["Night 1."]
        );
        assert!(
            narrator
                .narrate(&shown("seer", 1, Phase::Night, &everyone))
                .is_empty()
        );
    }

    #[test]
    fn by_night_each_role_does_its_own_thing_told_in_seat_order_as_the_day_begins() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert!(narrator.narrate(&chose("wolf", "ann")).is_empty());
        assert!(narrator.narrate(&chose("doctor", "seer")).is_empty());
        assert!(narrator.narrate(&chose("seer", "wolf")).is_empty());
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"])),
            [
                "doctor (doctor) protects seer (seer).",
                "seer (seer) learns wolf is a werewolf.",
                "wolf (werewolf) votes to kill ann (villager).",
                "ann dies.",
                "Day 1."
            ]
        );
    }

    #[test]
    fn players_come_in_seat_order_with_player2_before_player10() {
        let mut narrator = Narrator::default();
        let roles = HashMap::from([
            (id("player10"), Role::Villager),
            (id("player2"), Role::Werewolf),
            (id("player1"), Role::Seer),
        ]);
        let start = Entry::Start {
            variation: "Uniform Random".to_string(),
            roles,
        };
        assert_eq!(
            narrator.narrate(&start),
            [
                "Uniform Random with 3 players: player1 (seer), player2 (werewolf), player10 (villager)."
            ]
        );
        let everyone = ["player1", "player2", "player10"];
        narrator.narrate(&shown("player1", 1, Phase::Day, &everyone));
        narrator.narrate(&chose("player10", "player2"));
        narrator.narrate(&chose("player2", "player10"));
        narrator.narrate(&chose("player1", "player2"));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 1, &["player2"])),
            [
                "player1 (seer) votes against player2 (werewolf).",
                "player2 (werewolf) votes against player10 (villager).",
                "player10 (villager) votes against player2 (werewolf).",
                "player1 and player10 are voted out.",
                "The werewolves win after 1 day. Survivors: player2 (werewolf)."
            ]
        );
    }

    #[test]
    fn a_nights_result_is_told_as_the_day_begins() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"])),
            ["ann dies.", "Day 1."]
        );
    }

    #[test]
    fn a_night_in_which_nobody_dies_says_so() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone)),
            ["Nobody dies.", "Day 1."]
        );
    }

    #[test]
    fn by_day_everyone_votes_told_in_seat_order_as_the_night_begins() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        assert!(narrator.narrate(&chose("wolf", "seer")).is_empty());
        assert!(narrator.narrate(&chose("seer", "wolf")).is_empty());
        assert_eq!(
            narrator.narrate(&shown("wolf", 2, Phase::Night, &["wolf", "seer", "doctor"])),
            [
                "seer (seer) votes against wolf (werewolf).",
                "wolf (werewolf) votes against seer (seer).",
                "Nobody is voted out.",
                "Night 2."
            ]
        );
    }

    #[test]
    fn a_days_result_is_told_as_the_night_begins() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 2, Phase::Night, &["wolf", "seer", "doctor"])),
            ["Nobody is voted out.", "Night 2."]
        );
        narrator.narrate(&shown("wolf", 2, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 3, Phase::Night, &["seer", "doctor"])),
            ["wolf is voted out.", "Night 3."]
        );
    }

    #[test]
    fn the_end_tells_the_last_result_then_the_winner() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["seer", "doctor"])),
            [
                "wolf is voted out.",
                "The villagers win after 1 day. Survivors: doctor (doctor), seer (seer)."
            ]
        );
    }

    #[test]
    fn an_announced_game_is_told_as_a_requested_one_is() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone)),
            ["Night 1."]
        );
        assert!(
            narrator
                .narrate(&announced("seer", 1, 1, Phase::Night, &everyone))
                .is_empty()
        );
        assert!(narrator.narrate(&selected("wolf", 1, "ann")).is_empty());
        assert_eq!(
            narrator.narrate(&announced(
                "wolf",
                2,
                1,
                Phase::Day,
                &["wolf", "seer", "doctor"]
            )),
            [
                "wolf (werewolf) votes to kill ann (villager).",
                "ann dies.",
                "Day 1."
            ]
        );
        assert!(narrator.narrate(&selected("seer", 2, "wolf")).is_empty());
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["seer", "doctor"])),
            [
                "seer (seer) votes against wolf (werewolf).",
                "wolf is voted out.",
                "The villagers win after 1 day. Survivors: doctor (doctor), seer (seer)."
            ]
        );
    }

    #[test]
    fn a_selection_for_a_phase_that_has_ended_is_told_as_too_late() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone));
        narrator.narrate(&announced("seer", 1, 1, Phase::Night, &everyone));
        narrator.narrate(&selected("wolf", 1, "ann"));
        narrator.narrate(&announced(
            "wolf",
            2,
            1,
            Phase::Day,
            &["wolf", "seer", "doctor"],
        ));
        assert_eq!(
            narrator.narrate(&selected("seer", 1, "wolf")),
            ["seer (seer) chooses wolf (werewolf) for night 1 too late to count."]
        );
        // A selection in time is held for the end of its phase.
        assert!(narrator.narrate(&selected("seer", 2, "wolf")).is_empty());
    }

    #[test]
    fn a_player_is_told_its_own_selection_came_too_late() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("doctor", 1, 1, Phase::Night, &everyone));
        narrator.narrate(&announced("doctor", 2, 1, Phase::Day, &everyone));
        narrator.narrate(&announced("doctor", 3, 2, Phase::Night, &everyone));
        assert_eq!(
            narrator.narrate(&selected("doctor", 2, "ann")),
            ["You choose ann for day 1 too late to count."]
        );
    }

    #[test]
    fn a_statement_that_is_not_a_selection_is_passed_over() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let received = Entry::Received {
            from: id("wolf"),
            message: Message::Action(id("ann")),
        };
        assert!(narrator.narrate(&received).is_empty());
    }

    #[test]
    fn a_message_going_the_other_way_is_passed_over() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let backwards = Entry::Sent {
            to: id("wolf"),
            message: Message::Action(id("ann")),
        };
        assert!(narrator.narrate(&backwards).is_empty());
        let backwards = Entry::Replied {
            from: id("wolf"),
            message: observation(1, Phase::Night, &["wolf"]),
        };
        assert!(narrator.narrate(&backwards).is_empty());
    }

    #[test]
    fn a_seer_asking_about_nobody_at_the_table_is_told_as_asking() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "seer",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        narrator.narrate(&chose("seer", "stranger"));
        assert_eq!(
            narrator.narrate(&shown(
                "seer",
                1,
                Phase::Day,
                &["wolf", "seer", "doctor", "ann"]
            )),
            ["seer (seer) asks about stranger.", "Nobody dies.", "Day 1."]
        );
    }

    #[test]
    fn a_survivor_the_deal_never_named_is_named_alone() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["stranger"])),
            ["The villagers win after 1 day. Survivors: stranger."]
        );
    }

    #[test]
    fn days_are_counted_in_the_plural() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 0, &["wolf", "ann"])),
            [
                "doctor and seer die.",
                "The werewolves win after 0 days. Survivors: ann (villager), wolf (werewolf)."
            ]
        );
    }

    #[test]
    fn several_voted_out_take_a_plural_verb() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Day,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 1, &["wolf", "ann"])),
            [
                "doctor and seer are voted out.",
                "The werewolves win after 1 day. Survivors: ann (villager), wolf (werewolf)."
            ]
        );
    }

    #[test]
    fn a_player_is_shown_no_table() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert!(narrator.narrate(&start()).is_empty());
    }

    #[test]
    fn a_werewolf_opens_with_its_pack_and_kills_in_the_second_person() {
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "bob", "seer", "doctor", "ann"];
        let pack = [("wolf", Role::Werewolf), ("bob", Role::Werewolf)];
        assert_eq!(
            narrator.narrate(&told("wolf", 1, Phase::Night, &everyone, &pack)),
            [
                "You are wolf, a werewolf. The werewolves are bob and wolf.",
                "Night 1."
            ]
        );
        assert_eq!(
            narrator.narrate(&chose("wolf", "ann")),
            ["You vote to kill ann."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "wolf",
                1,
                Phase::Day,
                &["wolf", "bob", "seer", "doctor"],
                &pack
            )),
            ["ann dies.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("wolf", "seer")),
            ["You vote against seer."]
        );
    }

    #[test]
    fn a_werewolf_without_a_pack_is_told_it_is_alone() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert_eq!(
            narrator.narrate(&told(
                "wolf",
                1,
                Phase::Night,
                &["wolf", "ann"],
                &[("wolf", Role::Werewolf)]
            )),
            ["You are wolf, the only werewolf.", "Night 1."]
        );
    }

    #[test]
    fn a_player_shown_no_role_is_named_alone() {
        let mut narrator = Narrator::for_player(id("ann"));
        assert_eq!(
            narrator.narrate(&shown("ann", 1, Phase::Day, &["wolf", "ann"])),
            ["You are ann.", "Day 1."]
        );
    }

    #[test]
    fn the_seer_asks_by_night_and_learns_by_morning() {
        let mut narrator = Narrator::for_player(id("seer"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&told(
                "seer",
                1,
                Phase::Night,
                &everyone,
                &[("seer", Role::Seer)]
            )),
            ["You are seer, the seer.", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("seer", "wolf")),
            ["You ask about wolf."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "seer",
                1,
                Phase::Day,
                &["wolf", "seer", "doctor"],
                &[("seer", Role::Seer), ("wolf", Role::Werewolf)]
            )),
            ["You learn wolf is a werewolf.", "ann dies.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("seer", "wolf")),
            ["You vote against wolf (werewolf)."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "seer",
                2,
                Phase::Night,
                &["wolf", "seer", "doctor"],
                &[("seer", Role::Seer), ("wolf", Role::Werewolf)]
            )),
            ["Nobody is voted out.", "Night 2."]
        );
    }

    #[test]
    fn the_doctor_protects_by_night_and_votes_by_day() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        assert_eq!(
            narrator.narrate(&told("doctor", 1, Phase::Night, &everyone, &known)),
            ["You are doctor, the doctor.", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("doctor", "seer")),
            ["You protect seer."]
        );
        assert_eq!(
            narrator.narrate(&told("doctor", 1, Phase::Day, &everyone, &known)),
            ["Nobody dies.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("doctor", "wolf")),
            ["You vote against wolf."]
        );
    }

    #[test]
    fn a_villager_sleeps_through_the_nights() {
        let mut narrator = Narrator::for_player(id("ann"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                1,
                Phase::Day,
                &everyone,
                &[("ann", Role::Villager)]
            )),
            ["You are ann, a villager.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("ann", "wolf")),
            ["You vote against wolf."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                2,
                Phase::Day,
                &["seer", "doctor", "ann"],
                &[("ann", Role::Villager)]
            )),
            ["wolf died.", "Day 2."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                3,
                Phase::Day,
                &["seer", "doctor", "ann"],
                &[("ann", Role::Villager)]
            )),
            ["Nobody died.", "Day 3."]
        );
    }

    #[test]
    fn entries_about_other_players_are_not_in_a_players_account() {
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert!(
            narrator
                .narrate(&told(
                    "seer",
                    1,
                    Phase::Night,
                    &everyone,
                    &[("seer", Role::Seer)]
                ))
                .is_empty()
        );
        assert!(narrator.narrate(&chose("seer", "wolf")).is_empty());
        assert!(
            narrator
                .narrate(&announced("seer", 1, 1, Phase::Night, &everyone))
                .is_empty()
        );
        assert!(narrator.narrate(&selected("seer", 1, "wolf")).is_empty());
    }

    #[test]
    fn an_announced_game_is_told_to_its_player_as_a_requested_one_is() {
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let announcement = Entry::Sent {
            to: id("wolf"),
            message: Message::Announce {
                seq: 1,
                observation: observed(1, Phase::Night, &everyone, &[("wolf", Role::Werewolf)]),
            },
        };
        assert_eq!(
            narrator.narrate(&announcement),
            ["You are wolf, the only werewolf.", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&selected("wolf", 1, "ann")),
            ["You vote to kill ann."]
        );
    }

    #[test]
    fn a_player_names_the_survivors_it_knows_the_roles_of() {
        let mut narrator = Narrator::for_player(id("seer"));
        narrator.narrate(&told(
            "seer",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
            &[("seer", Role::Seer), ("wolf", Role::Werewolf)],
        ));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 0, &["wolf", "seer", "doctor"])),
            [
                "ann dies.",
                "The werewolves win after 0 days. Survivors: doctor, seer (seer), wolf (werewolf)."
            ]
        );
    }

    #[test]
    fn the_configuration_adds_nothing_to_either_account() {
        let configuration = Entry::Configuration(Box::new(Configuration {
            role_counts: BTreeMap::from([(Role::Werewolf, 1), (Role::Villager, 2)]),
            night_limit: Duration::from_secs(30),
            day_limit: Duration::from_secs(60),
            request_timeout: Duration::from_secs(10),
            base_url: "http://localhost:1234/v1".to_string(),
            model: "qwen2.5-7b-instruct".to_string(),
            api_key_env: None,
            personas: BTreeMap::new(),
            prompts: BTreeMap::from([(id("wolf"), "You are wolf.".to_string())]),
            default_template: Some("You are {{ name }}.".to_string()),
            role_templates: BTreeMap::new(),
            text: BTreeMap::new(),
        }));
        assert!(Narrator::default().narrate(&configuration).is_empty());
        assert!(
            Narrator::for_player(id("wolf"))
                .narrate(&configuration)
                .is_empty()
        );
    }
}
