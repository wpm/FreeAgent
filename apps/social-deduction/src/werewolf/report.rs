//! A legible account of a game, told from its log as the log is written,
//! either of everything or of what one player saw.

use super::{Entry, Hunt, Message, Observation, Phase, PlayerId, Role};
use free_agent::Event;
use humantime::format_duration;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::num::NonZero;
use std::time::{Duration, SystemTime};

/// Whose account a narrator gives.
#[derive(Debug, Default)]
enum Voice {
    /// Everyone's: the deal, every deed, every death.
    #[default]
    Omniscient,
    /// One player's: what it was shown and what it did, told to it.
    Player(PlayerId),
}

/// Turns the log's events, one at a time, into lines of prose. The
/// omniscient narrator tells the table, a player to a line, and how long
/// a night and a day wait, then each night and day with what every player
/// did, how it ended, and how long it took, then who won and the
/// survivors, a player to a line. A player's narrator tells the same
/// nights and days from what that player was shown, in the second person,
/// with no table, no limits, and no times. Each names a player with its
/// role in parentheses whenever it knows the role, except in reporting its
/// death, and lists players in seat order. A blank line sets each phase
/// off from what came before it, and another the winner from the last.
/// Both learn who died from who is shown as alive next, so a phase's
/// result is told as the next one begins, and the omniscient narrator
/// holds the phase's deeds until then too, to tell them in seat order,
/// then whom the werewolves chose and who protected them, which no player
/// is shown; a player's narrator tells the player's own deed as it is
/// done. A phase begins at its first announcement and ends at the next
/// phase's, or at the end of the game, and the omniscient narrator tells
/// the time between, to the nearest second, after who died. A phase
/// announced is told as one requested, and a selection as a reply, unless
/// a later phase has been announced since: then it is told as too late to
/// count, for the phase it was for.
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
    /// When the phase being told began: the time of its first
    /// announcement.
    began: Option<SystemTime>,
    /// The deeds of the phase being told, each with whose it is, held in
    /// the omniscient account until the phase ends.
    deeds: Vec<(PlayerId, String)>,
    /// What came of the night's hunt, held in the omniscient account until
    /// the night ends.
    hunted: Option<String>,
    /// Every phase announced so far, by its number.
    announced: BTreeMap<u64, (NonZero<u8>, Phase)>,
}

impl Narrator {
    /// The account `me` would give of its own game. Feed it the events
    /// about `me`: each observation `me` was sent, and each selection it
    /// made. Events about anyone else add nothing, so the whole log tells
    /// it the same account as `me`'s own history does. It opens with who
    /// `me` is, names the roles `me` knows and nobody else's, and tells
    /// `me`'s deeds as "You ...".
    pub fn for_player(me: PlayerId) -> Self {
        Self {
            voice: Voice::Player(me),
            ..Self::default()
        }
    }

    /// The lines `event` adds to the account, often none.
    pub fn narrate(&mut self, event: &Event<Entry>) -> Vec<String> {
        let time = event.time;
        match &event.payload {
            Entry::Start {
                variation,
                roles,
                night_limit,
                day_limit,
            } if matches!(self.voice, Voice::Omniscient) => {
                self.roles = roles.clone();
                self.alive = roles.keys().cloned().collect();
                let mut lines = self.table(variation);
                lines.push(format!(
                    "A night waits {} and a day {}.",
                    format_duration(*night_limit),
                    format_duration(*day_limit)
                ));
                lines
            }
            Entry::Sent {
                to,
                message: Message::Observation(observation),
            } if self.concerns(to) => self.show(observation, time),
            Entry::Sent {
                to,
                message: Message::Announce { seq, observation },
            } if self.concerns(to) => {
                self.announced
                    .insert(*seq, (observation.round, observation.phase.clone()));
                self.show(observation, time)
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
            Entry::Hunted(hunt) if matches!(self.voice, Voice::Omniscient) => {
                self.hunted = Some(self.hunt(hunt));
                vec![]
            }
            Entry::End {
                winner,
                days,
                survivors,
            } => {
                let mut lines = self.outcome(survivors, false, time);
                lines.push(String::new());
                lines.push(format!(
                    "The {winner} win after {days} {}.",
                    if *days == 1 { "day" } else { "days" }
                ));
                lines.push("Survivors:".to_string());
                lines.extend(
                    in_order(survivors)
                        .into_iter()
                        .map(|player| self.seat(player).to_string()),
                );
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

    /// The deal: how many players, then each with its role on a line of
    /// its own.
    fn table(&self, variation: &str) -> Vec<String> {
        let heading = format!("{variation} with {} players:", self.roles.len());
        let seats = in_order(self.roles.keys())
            .into_iter()
            .map(|player| self.seat(player).to_string());
        std::iter::once(heading).chain(seats).collect()
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

    /// What a player being shown `observation` at `time` adds to the
    /// account: the roles it learns, and the phase it begins.
    fn show(&mut self, observation: &Observation, time: SystemTime) -> Vec<String> {
        let mut lines = self.learn(&observation.roles);
        lines.extend(self.begin(observation, time));
        lines
    }

    /// A phase begins when a player is first shown it, at `time`. The
    /// phase before it ends then, with whoever is no longer alive, and a
    /// blank line sets the new phase off from what came before it.
    fn begin(&mut self, observation: &Observation, time: SystemTime) -> Vec<String> {
        let phase = (observation.round, observation.phase.clone());
        if self.phase.as_ref() == Some(&phase) {
            return vec![];
        }
        let slept = self
            .phase
            .as_ref()
            .is_some_and(|told| ordinal(&phase) != ordinal(told) + 1);
        let mut lines = self.outcome(&observation.alive, slept, time);
        self.alive = observation.alive.clone();
        lines.push(String::new());
        lines.push(format!("{}.", title(&phase)));
        self.phase = Some(phase);
        self.began = Some(time);
        lines
    }

    /// How the phase being told ended at `time`, given who is alive after
    /// it: the deeds held for it, the hunt if there was one, then who died,
    /// by name alone and in the same words by night and by day, then in
    /// the omniscient account how long the phase took. A player that
    /// `slept` through phases since is told who died in the past tense.
    /// Several dead take a plural verb.
    fn outcome(
        &mut self,
        alive_after: &HashSet<PlayerId>,
        slept: bool,
        time: SystemTime,
    ) -> Vec<String> {
        let mut lines = self.done();
        lines.extend(self.hunted.take());
        let Some(told) = &self.phase else {
            return lines;
        };
        let dead = in_order(self.alive.difference(alive_after));
        let listed = listed(dead.iter().map(|player| Seat::named(player)));
        let line = match (slept, dead.len()) {
            (true, 0) => "Nobody died.".to_string(),
            (true, _) => format!("{listed} died."),
            (false, 0) => "Nobody dies.".to_string(),
            (false, 1) => format!("{listed} dies."),
            (false, _) => format!("{listed} die."),
        };
        self.alive = alive_after.clone();
        lines.push(line);
        if let (Voice::Omniscient, Some(began)) = (&self.voice, self.began) {
            let took = time.duration_since(began).unwrap_or_default();
            lines.push(format!("{} took {}.", title(told), whole_seconds(took)));
        }
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

    /// What came of the werewolves' `hunt`: whom they chose, and who
    /// protected them if anyone did.
    fn hunt(&self, hunt: &Hunt) -> String {
        let victim = self.seat(&hunt.victim);
        let protectors = in_order(&hunt.protectors);
        let listed = listed(protectors.iter().map(|doctor| self.seat(doctor)));
        match protectors.len() {
            0 => format!("The werewolves choose {victim}."),
            1 => format!("The werewolves choose {victim}, whom {listed} protects."),
            _ => format!("The werewolves choose {victim}, whom {listed} protect."),
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

/// A phase by its name: `Night 1`, `Day 1`.
fn title((round, phase): &(NonZero<u8>, Phase)) -> String {
    match phase {
        Phase::Night => format!("Night {round}"),
        Phase::Day => format!("Day {round}"),
    }
}

/// `elapsed` to the nearest whole second, as `humantime` writes it: `40s`,
/// `1m 5s`.
fn whole_seconds(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64().round() as u64;
    format_duration(Duration::from_secs(seconds)).to_string()
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
    (numbered(a), a).cmp(&(numbered(b), b))
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
    use std::time::{Duration, UNIX_EPOCH};

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// `entry` as logged when the game began.
    fn logged(entry: Entry) -> Event<Entry> {
        at(0.0, Event::now(Default::default(), entry))
    }

    /// `event`, logged `seconds` into the game.
    fn at(seconds: f64, event: Event<Entry>) -> Event<Entry> {
        Event {
            time: UNIX_EPOCH + Duration::from_secs_f64(seconds),
            ..event
        }
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

    fn shown(to: &str, round: u8, phase: Phase, alive: &[&str]) -> Event<Entry> {
        told(to, round, phase, alive, &[])
    }

    /// `to` was shown round `round`'s `phase` with `alive` alive and `known`
    /// roles, as a player of a requesting environment is.
    fn told(
        to: &str,
        round: u8,
        phase: Phase,
        alive: &[&str],
        known: &[(&str, Role)],
    ) -> Event<Entry> {
        logged(Entry::Sent {
            to: id(to),
            message: Message::Observation(observed(round, phase, alive, known)),
        })
    }

    fn chose(from: &str, chosen: &str) -> Event<Entry> {
        logged(Entry::Replied {
            from: id(from),
            message: Message::Action(id(chosen)),
        })
    }

    /// The environment announced the phase to `to`.
    fn announced(to: &str, seq: u64, round: u8, phase: Phase, alive: &[&str]) -> Event<Entry> {
        logged(Entry::Sent {
            to: id(to),
            message: Message::Announce {
                seq,
                observation: seen(round, phase, alive),
            },
        })
    }

    /// `from` selected `target` in the phase numbered `seq`.
    fn selected(from: &str, seq: u64, target: &str) -> Event<Entry> {
        logged(Entry::Received {
            from: id(from),
            message: Message::Select {
                seq,
                from: id(from),
                target: id(target),
            },
        })
    }

    /// The werewolves elected `victim`, whom `protectors` protected.
    fn hunted(victim: &str, protectors: &[&str]) -> Event<Entry> {
        logged(Entry::Hunted(Hunt {
            victim: id(victim),
            protectors: protectors.iter().map(|name| id(name)).collect(),
        }))
    }

    fn ended(winner: Team, days: u8, survivors: &[&str]) -> Event<Entry> {
        logged(Entry::End {
            winner,
            days,
            survivors: survivors.iter().map(|name| id(name)).collect(),
        })
    }

    /// The deal of `roles`, with a night waiting a minute and a day two.
    fn dealt(roles: HashMap<PlayerId, Role>) -> Event<Entry> {
        logged(Entry::Start {
            variation: "Uniform Random".to_string(),
            roles,
            night_limit: Duration::from_secs(60),
            day_limit: Duration::from_secs(120),
        })
    }

    fn start() -> Event<Entry> {
        dealt(roles())
    }

    #[test]
    fn the_table_is_told_first_then_the_limits() {
        let mut narrator = Narrator::default();
        assert_eq!(
            narrator.narrate(&start()),
            [
                "Uniform Random with 4 players:",
                "ann (villager)",
                "doctor (doctor)",
                "seer (seer)",
                "wolf (werewolf)",
                "A night waits 1m and a day 2m."
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
            ["", "Night 1."]
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
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn the_werewolves_choice_is_told_after_the_deeds_and_before_who_dies() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        narrator.narrate(&chose("wolf", "ann"));
        assert!(narrator.narrate(&hunted("ann", &[])).is_empty());
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"])),
            [
                "wolf (werewolf) votes to kill ann (villager).",
                "The werewolves choose ann (villager).",
                "ann dies.",
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn a_victim_the_doctor_protects_is_told_as_protected() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        narrator.narrate(&chose("wolf", "ann"));
        narrator.narrate(&chose("doctor", "ann"));
        narrator.narrate(&hunted("ann", &["doctor"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone)),
            [
                "doctor (doctor) protects ann (villager).",
                "wolf (werewolf) votes to kill ann (villager).",
                "The werewolves choose ann (villager), whom doctor (doctor) protects.",
                "Nobody dies.",
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn several_protectors_take_a_plural_verb() {
        let mut narrator = Narrator::default();
        narrator.narrate(&dealt(HashMap::from([
            (id("wolf"), Role::Werewolf),
            (id("doc1"), Role::Doctor),
            (id("doc2"), Role::Doctor),
            (id("ann"), Role::Villager),
        ])));
        let everyone = ["wolf", "doc1", "doc2", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        narrator.narrate(&hunted("ann", &["doc2", "doc1"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone)),
            [
                "The werewolves choose ann (villager), whom doc1 (doctor) and doc2 (doctor) protect.",
                "Nobody dies.",
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn a_player_is_not_told_whom_the_werewolves_chose() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&told(
            "doctor",
            1,
            Phase::Night,
            &everyone,
            &[("doctor", Role::Doctor)],
        ));
        assert!(narrator.narrate(&hunted("ann", &["doctor"])).is_empty());
        assert_eq!(
            narrator.narrate(&told(
                "doctor",
                1,
                Phase::Day,
                &everyone,
                &[("doctor", Role::Doctor)]
            )),
            ["Nobody dies.", "", "Day 1."]
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
        assert_eq!(
            narrator.narrate(&dealt(roles)),
            [
                "Uniform Random with 3 players:",
                "player1 (seer)",
                "player2 (werewolf)",
                "player10 (villager)",
                "A night waits 1m and a day 2m."
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
                "player1 and player10 die.",
                "Day 1 took 0s.",
                "",
                "The werewolves win after 1 day.",
                "Survivors:",
                "player2 (werewolf)"
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
            ["ann dies.", "Night 1 took 0s.", "", "Day 1."]
        );
    }

    #[test]
    fn a_phase_in_which_nobody_dies_says_so() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone)),
            ["Nobody dies.", "Night 1 took 0s.", "", "Day 1."]
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
                "Nobody dies.",
                "Day 1 took 0s.",
                "",
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
            ["Nobody dies.", "Day 1 took 0s.", "", "Night 2."]
        );
        narrator.narrate(&shown("wolf", 2, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 3, Phase::Night, &["seer", "doctor"])),
            ["wolf dies.", "Day 2 took 0s.", "", "Night 3."]
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
                "wolf dies.",
                "Day 1 took 0s.",
                "",
                "The villagers win after 1 day.",
                "Survivors:",
                "doctor (doctor)",
                "seer (seer)"
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
            ["", "Night 1."]
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
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
        assert!(narrator.narrate(&selected("seer", 2, "wolf")).is_empty());
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["seer", "doctor"])),
            [
                "seer (seer) votes against wolf (werewolf).",
                "wolf dies.",
                "Day 1 took 0s.",
                "",
                "The villagers win after 1 day.",
                "Survivors:",
                "doctor (doctor)",
                "seer (seer)"
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
        let received = logged(Entry::Received {
            from: id("wolf"),
            message: Message::Action(id("ann")),
        });
        assert!(narrator.narrate(&received).is_empty());
    }

    #[test]
    fn a_message_going_the_other_way_is_passed_over() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let backwards = logged(Entry::Sent {
            to: id("wolf"),
            message: Message::Action(id("ann")),
        });
        assert!(narrator.narrate(&backwards).is_empty());
        let backwards = logged(Entry::Replied {
            from: id("wolf"),
            message: observation(1, Phase::Night, &["wolf"]),
        });
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
            [
                "seer (seer) asks about stranger.",
                "Nobody dies.",
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn a_survivor_the_deal_never_named_is_named_alone() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["stranger"])),
            [
                "",
                "The villagers win after 1 day.",
                "Survivors:",
                "stranger"
            ]
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
                "Night 1 took 0s.",
                "",
                "The werewolves win after 0 days.",
                "Survivors:",
                "ann (villager)",
                "wolf (werewolf)"
            ]
        );
    }

    #[test]
    fn several_dead_by_day_take_a_plural_verb() {
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
                "doctor and seer die.",
                "Day 1 took 0s.",
                "",
                "The werewolves win after 1 day.",
                "Survivors:",
                "ann (villager)",
                "wolf (werewolf)"
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
                "",
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
            ["ann dies.", "", "Day 1."]
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
            ["You are wolf, the only werewolf.", "", "Night 1."]
        );
    }

    #[test]
    fn a_player_shown_no_role_is_named_alone() {
        let mut narrator = Narrator::for_player(id("ann"));
        assert_eq!(
            narrator.narrate(&shown("ann", 1, Phase::Day, &["wolf", "ann"])),
            ["You are ann.", "", "Day 1."]
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
            ["You are seer, the seer.", "", "Night 1."]
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
            ["You learn wolf is a werewolf.", "ann dies.", "", "Day 1."]
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
            ["Nobody dies.", "", "Night 2."]
        );
    }

    #[test]
    fn the_doctor_protects_by_night_and_votes_by_day() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        assert_eq!(
            narrator.narrate(&told("doctor", 1, Phase::Night, &everyone, &known)),
            ["You are doctor, the doctor.", "", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("doctor", "seer")),
            ["You protect seer."]
        );
        assert_eq!(
            narrator.narrate(&told("doctor", 1, Phase::Day, &everyone, &known)),
            ["Nobody dies.", "", "Day 1."]
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
            ["You are ann, a villager.", "", "Day 1."]
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
            ["wolf died.", "", "Day 2."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                3,
                Phase::Day,
                &["seer", "doctor", "ann"],
                &[("ann", Role::Villager)]
            )),
            ["Nobody died.", "", "Day 3."]
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
        let announcement = logged(Entry::Sent {
            to: id("wolf"),
            message: Message::Announce {
                seq: 1,
                observation: observed(1, Phase::Night, &everyone, &[("wolf", Role::Werewolf)]),
            },
        });
        assert_eq!(
            narrator.narrate(&announcement),
            ["You are wolf, the only werewolf.", "", "Night 1."]
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
                "",
                "The werewolves win after 0 days.",
                "Survivors:",
                "doctor",
                "seer (seer)",
                "wolf (werewolf)"
            ]
        );
    }

    #[test]
    fn a_phase_is_timed_from_its_first_announcement_to_the_next_and_the_last_to_the_end() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&at(10.0, announced("wolf", 1, 1, Phase::Night, &everyone)));
        // A later announcement of the same phase does not restart it.
        narrator.narrate(&at(12.0, announced("seer", 1, 1, Phase::Night, &everyone)));
        assert_eq!(
            narrator.narrate(&at(50.4, announced("wolf", 2, 1, Phase::Day, &everyone))),
            ["Nobody dies.", "Night 1 took 40s.", "", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&at(
                126.0,
                ended(Team::Villagers, 1, &["seer", "doctor", "ann"])
            )),
            [
                "wolf dies.",
                "Day 1 took 1m 16s.",
                "",
                "The villagers win after 1 day.",
                "Survivors:",
                "ann (villager)",
                "doctor (doctor)",
                "seer (seer)"
            ]
        );
    }

    #[test]
    fn a_phases_time_is_rounded_to_the_nearest_second() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&at(0.0, shown("wolf", 1, Phase::Night, &everyone)));
        assert_eq!(
            narrator.narrate(&at(2.5, shown("wolf", 1, Phase::Day, &everyone))),
            ["Nobody dies.", "Night 1 took 3s.", "", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&at(2.9, shown("wolf", 2, Phase::Night, &everyone))),
            ["Nobody dies.", "Day 1 took 0s.", "", "Night 2."]
        );
    }

    #[test]
    fn a_phase_announced_before_the_clock_went_back_took_no_time() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&at(10.0, shown("wolf", 1, Phase::Night, &everyone)));
        assert_eq!(
            narrator.narrate(&at(5.0, shown("wolf", 1, Phase::Day, &everyone))),
            ["Nobody dies.", "Night 1 took 0s.", "", "Day 1."]
        );
    }

    #[test]
    fn a_player_is_told_neither_the_limits_nor_how_long_a_phase_took() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert!(narrator.narrate(&start()).is_empty());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("wolf", Role::Werewolf)];
        assert_eq!(
            narrator.narrate(&at(10.0, told("wolf", 1, Phase::Night, &everyone, &known))),
            ["You are wolf, the only werewolf.", "", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&at(50.0, told("wolf", 1, Phase::Day, &everyone, &known))),
            ["Nobody dies.", "", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&at(90.0, ended(Team::Werewolves, 1, &["wolf", "ann"]))),
            [
                "doctor and seer die.",
                "",
                "The werewolves win after 1 day.",
                "Survivors:",
                "ann",
                "wolf (werewolf)"
            ]
        );
    }

    #[test]
    fn the_configuration_adds_nothing_to_either_account() {
        let configuration = logged(Entry::Configuration(Box::new(Configuration {
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
        })));
        assert!(Narrator::default().narrate(&configuration).is_empty());
        assert!(
            Narrator::for_player(id("wolf"))
                .narrate(&configuration)
                .is_empty()
        );
    }
}
