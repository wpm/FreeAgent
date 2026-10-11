//! A legible account of a game: of everything, told from the log as it is
//! written, or of what one player saw, told from the player's memory.
//!
//! The log is written in actions and observations, by every actor. The
//! omniscient account reads both: an announcement is told from the
//! environment's action, and a selection when the environment's
//! observation of it is logged, joined to the player's action by the
//! player and its number for the message. A player's memory holds the
//! messages themselves, and its account reads them.

use super::llm::Memory;
use super::{Entry, Hunt, Message, Phase, PlayerId, Role, Team, View};
use free_agent::{ActorId, Event};
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

/// Turns the events of a game, one at a time, into lines of prose. The
/// omniscient narrator reads the log's events through [`Narrator::narrate`]
/// and tells the table, a player to a line, and how long a night and a day
/// wait, then each night and day with what every player did, how it ended,
/// and how long it took, then who won, how long the whole game took, and
/// the survivors, a player to a line. A player's narrator reads the
/// player's memory through [`Narrator::recall`] and tells the same nights
/// and days from what that player was shown, in the second person, with no
/// table, no limits, and no times.
/// Each names a player with its role in parentheses whenever it knows the
/// role, except in reporting its death, and lists players in seat order.
/// A blank line sets each phase off from what came before it, and another
/// the winner from the last.
/// Both learn who died from who is shown as alive next, so a phase's
/// result is told as the next one begins, and the omniscient narrator
/// holds the phase's deeds until then too, to tell them in seat order,
/// then whom the werewolves chose and who protected them, which no player
/// is shown; a player's narrator tells the player's own deed as it is
/// done. A phase begins at its first announcement and ends at the next
/// phase's, or at the end of the game, and the omniscient narrator tells
/// the time between, to the nearest second, after who died. The omniscient
/// narrator tells a selection when the environment logs observing it,
/// joined to the selection's own action by the player and its number for
/// the message: it is a deed of the phase it was for, unless the
/// environment observed it in a later phase, and then it is told as too
/// late to count, for the phase it was for. A player's narrator tells the
/// player's selection as too late when a later phase has been announced
/// since. A player's failure to select is told
/// by the omniscient narrator as it happens, with what went wrong, and for
/// the phase it was for if that phase has ended; a player's narrator
/// leaves it out, since the player is not told of it.
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
    phase: Option<Told>,
    /// The deeds of the phase being told, each with whose it is, held in
    /// the omniscient account until the phase ends.
    deeds: Vec<(PlayerId, String)>,
    /// What came of the night's hunt, held in the omniscient account until
    /// the night ends.
    hunted: Option<String>,
    /// Every phase announced so far, by its number.
    announced: BTreeMap<u64, (NonZero<u8>, Phase)>,
    /// Every selection logged as its player's action and not yet observed
    /// by the environment, by the player and its number for the message:
    /// the phase it was for, and whom it chose.
    selections: HashMap<(ActorId, u64), (u64, PlayerId)>,
}

/// The phase being told: which it is, and when it began.
#[derive(Debug)]
struct Told {
    /// The round and its half.
    phase: (NonZero<u8>, Phase),
    /// When the phase was first announced.
    began: SystemTime,
}

impl Narrator {
    /// The account `me` would give of its own game. Feed it `me`'s
    /// memories through [`Narrator::recall`]: each announcement `me`
    /// heard, and each selection it made. It opens with who `me` is, names
    /// the roles `me` knows and nobody else's, and tells `me`'s deeds as
    /// "You ...".
    pub fn for_player(me: PlayerId) -> Self {
        Self {
            voice: Voice::Player(me),
            ..Self::default()
        }
    }

    /// The lines the log's `event` adds to the account, often none. The
    /// omniscient account is told from the log; a player's reads only the
    /// end of the game from it.
    pub fn narrate(&mut self, event: &Event<Entry>) -> Vec<String> {
        let time = event.time;
        if let Entry::End {
            winner,
            days,
            survivors,
            took,
        } = &event.payload
        {
            return self.end(*winner, *days, survivors, *took, time);
        }
        if let Voice::Player(_) = self.voice {
            return vec![];
        }
        match &event.payload {
            Entry::Start {
                variation,
                roles,
                night_limit,
                day_limit,
            } => {
                self.roles = roles.clone();
                self.alive = roles.keys().cloned().collect();
                self.table(variation, *night_limit, *day_limit)
            }
            Entry::Action {
                message: Message::Announce { seq, view, .. },
                ..
            } => self.announce(*seq, view, time),
            Entry::Action {
                from,
                n,
                message: Message::Select { seq, target, .. },
                ..
            } => {
                self.selections
                    .insert((from.clone(), *n), (*seq, target.clone()));
                vec![]
            }
            Entry::Observation { from, n, seq, .. } => {
                match self.selections.remove(&(from.clone(), *n)) {
                    Some((sent_in, chosen)) if sent_in != *seq => {
                        vec![self.lateness(from, &chosen, self.announced.get(&sent_in))]
                    }
                    Some((_, chosen)) => self.did(from, &chosen),
                    None => vec![],
                }
            }
            Entry::Hunted { hunt, .. } => {
                self.hunted = Some(self.hunt(hunt));
                vec![]
            }
            Entry::Failed { player, seq, error } => {
                let player = self.seat(player);
                vec![match self.late(*seq) {
                    Some(phase) => format!(
                        "{player} fails to select for {}: {error}",
                        title(phase).to_lowercase()
                    ),
                    None => format!("{player} fails to select: {error}"),
                }]
            }
            _ => vec![],
        }
    }

    /// The end of the game at `time`: how the last phase ended, then a
    /// blank line, who won, how long the game took, and the survivors, a
    /// player to a line.
    fn end(
        &mut self,
        winner: Team,
        days: u8,
        survivors: &HashSet<PlayerId>,
        took: Duration,
        time: SystemTime,
    ) -> Vec<String> {
        let mut lines = self.outcome(survivors, false, time);
        lines.push(String::new());
        lines.push(format!(
            "The {winner} win after {days} {}.",
            if days == 1 { "day" } else { "days" }
        ));
        lines.push(format!("The game took {}.", whole_seconds(took)));
        lines.push("Survivors:".to_string());
        lines.extend(
            in_order(survivors)
                .into_iter()
                .map(|player| self.seat(player).to_string()),
        );
        lines
    }

    /// The lines a player's memory `event` adds to its account, often
    /// none: an announcement it heard begins a phase, and a selection it
    /// made is its deed, or too late to count if a later phase has been
    /// announced since.
    pub fn recall(&mut self, event: &Event<Memory>) -> Vec<String> {
        match &event.payload {
            Memory::Observed {
                message: Message::Announce { seq, view, .. },
                ..
            } => self.announce(*seq, view, event.time),
            Memory::Acted {
                message:
                    Message::Select {
                        seq, from, target, ..
                    },
            } => match self.late(*seq) {
                Some(phase) => vec![self.lateness(from, target, Some(phase))],
                None => self.did(from, target),
            },
            _ => vec![],
        }
    }

    /// What the announcement at `time` of the phase numbered `seq`, showing
    /// `view`, adds to the account.
    fn announce(&mut self, seq: u64, view: &View, time: SystemTime) -> Vec<String> {
        self.announced.insert(seq, (view.round, view.phase.clone()));
        self.show(view, time)
    }

    /// The deal: how many players, then each with its role on a line of
    /// its own, then how long a night and a day wait for them.
    fn table(&self, variation: &str, night_limit: Duration, day_limit: Duration) -> Vec<String> {
        let heading = format!("{variation} with {} players:", self.roles.len());
        let seats = in_order(self.roles.keys())
            .into_iter()
            .map(|player| self.seat(player).to_string());
        let limits = format!(
            "A night waits {} and a day {}.",
            format_duration(night_limit),
            format_duration(day_limit)
        );
        std::iter::once(heading)
            .chain(seats)
            .chain(std::iter::once(limits))
            .collect()
    }

    /// `player` as this account names it: with its role when known.
    fn seat<'a>(&self, player: &'a PlayerId) -> Seat<'a> {
        Seat {
            player,
            role: self.roles.get(player).copied(),
        }
    }

    /// Takes in the roles an view shows that were not known. In a
    /// player's account, its first view says who it is, and a role
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

    /// What a player being shown `view` at `time` adds to the
    /// account: the roles it learns, and the phase it begins.
    fn show(&mut self, view: &View, time: SystemTime) -> Vec<String> {
        let mut lines = self.learn(&view.roles);
        lines.extend(self.begin(view, time));
        lines
    }

    /// A phase begins when a player is first shown it, at `time`. The
    /// phase before it ends then, with whoever is no longer alive, and a
    /// blank line sets the new phase off from what came before it.
    fn begin(&mut self, view: &View, time: SystemTime) -> Vec<String> {
        let phase = (view.round, view.phase.clone());
        if self.phase.as_ref().is_some_and(|told| told.phase == phase) {
            return vec![];
        }
        let slept = self
            .phase
            .as_ref()
            .is_some_and(|told| ordinal(&phase) != ordinal(&told.phase) + 1);
        let mut lines = self.outcome(&view.alive, slept, time);
        self.alive = view.alive.clone();
        lines.push(String::new());
        lines.push(format!("{}.", title(&phase)));
        self.phase = Some(Told { phase, began: time });
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
        if matches!(self.voice, Voice::Omniscient) {
            let took = time.duration_since(told.began).unwrap_or_default();
            lines.push(format!(
                "{} took {}.",
                title(&told.phase),
                whole_seconds(took)
            ));
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
        let phase = self.phase.as_ref().map(|told| &told.phase.1);
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
    /// phase had begun: nothing. The phase is named when it is known, as
    /// it is unless the choice was for a phase never announced.
    fn lateness(
        &self,
        player: &PlayerId,
        chosen: &PlayerId,
        phase: Option<&(NonZero<u8>, Phase)>,
    ) -> String {
        let when = phase.map_or(String::new(), |phase| {
            format!(" for {}", title(phase).to_lowercase())
        });
        let chosen = self.seat(chosen);
        match &self.voice {
            Voice::Player(_) => format!("You choose {chosen}{when} too late to count."),
            Voice::Omniscient => {
                format!(
                    "{} chooses {chosen}{when} too late to count.",
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

/// Who `me` is, from the roles its first view shows: its own, and
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
    use crate::werewolf::llm::Configuration;
    use crate::werewolf::tests::{announcement, logged, selection};
    use crate::werewolf::{ENVIRONMENT, Rules, to_the_environment};
    use std::collections::BTreeMap;
    use std::time::{Duration, UNIX_EPOCH};

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// `event`, logged or remembered `seconds` into the game.
    fn at<L>(seconds: f64, event: Event<L>) -> Event<L> {
        Event {
            time: UNIX_EPOCH + Duration::from_secs_f64(seconds),
            ..event
        }
    }

    /// `from`'s selection of `target` for the phase numbered `seq`, as a
    /// message. A player selects once a phase in these accounts, so its
    /// number for the message is the phase's.
    fn choice(from: &str, seq: u64, target: &str) -> Message {
        selection(seq, from, seq, target)
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
    fn observed(round: u8, phase: Phase, alive: &[&str], known: &[(&str, Role)]) -> View {
        View {
            round: NonZero::new(round).unwrap(),
            phase,
            roles: known.iter().map(|(name, role)| (id(name), *role)).collect(),
            alive: alive.iter().map(|name| id(name)).collect(),
        }
    }

    fn seen(round: u8, phase: Phase, alive: &[&str]) -> View {
        observed(round, phase, alive, &[])
    }

    /// The number of round `round`'s `phase`, counting from 1 for the first
    /// night.
    fn seq_of(round: u8, phase: &Phase) -> u64 {
        let half = match phase {
            Phase::Night => 0,
            Phase::Day => 1,
        };
        u64::from(round - 1) * 2 + 1 + half
    }

    fn shown(to: &str, round: u8, phase: Phase, alive: &[&str]) -> Event<Entry> {
        told(to, round, phase, alive, &[])
    }

    /// The environment's action of announcing round `round`'s `phase` to
    /// `to`, with `alive` alive and `known` roles shown.
    fn told(
        to: &str,
        round: u8,
        phase: Phase,
        alive: &[&str],
        known: &[(&str, Role)],
    ) -> Event<Entry> {
        let seq = seq_of(round, &phase);
        logged(sent(to, seq, observed(round, phase, alive, known)))
    }

    /// The environment's action of announcing the phase numbered `seq` to
    /// `to`.
    fn announced(to: &str, seq: u64, round: u8, phase: Phase, alive: &[&str]) -> Event<Entry> {
        logged(sent(to, seq, seen(round, phase, alive)))
    }

    /// The environment's action of announcing the phase numbered `seq` to
    /// `to`, showing `view`.
    fn sent(to: &str, seq: u64, view: View) -> Entry {
        Entry::Action {
            from: ENVIRONMENT.to_string(),
            n: seq,
            to: HashSet::from([id(to)]),
            message: announcement(seq, view),
            seen: None,
        }
    }

    /// `from`'s action of selecting `target` for the phase numbered `seq`.
    fn acted(from: &str, seq: u64, target: &str) -> Event<Entry> {
        logged(Entry::Action {
            from: id(from),
            n: seq,
            to: to_the_environment(),
            message: choice(from, seq, target),
            seen: None,
        })
    }

    /// The environment's observation, in the phase numbered `observed_in`,
    /// of the message `from` numbered `n`.
    fn observed_by_the_environment(from: &str, n: u64, observed_in: u64) -> Event<Entry> {
        logged(Entry::Observation {
            by: ENVIRONMENT.to_string(),
            from: id(from),
            n,
            seq: observed_in,
        })
    }

    /// Tell `narrator` that `from` selected `target` for the phase numbered
    /// `seq` and that the environment observed it in the phase numbered
    /// `observed_in`: what the observation adds, the action adding nothing.
    fn selected_in(
        narrator: &mut Narrator,
        from: &str,
        seq: u64,
        target: &str,
        observed_in: u64,
    ) -> Vec<String> {
        assert!(narrator.narrate(&acted(from, seq, target)).is_empty());
        narrator.narrate(&observed_by_the_environment(from, seq, observed_in))
    }

    /// Tell `narrator` that `from` selected `target` for the phase numbered
    /// `seq`, which the environment observed in that phase: what it adds.
    fn selected(narrator: &mut Narrator, from: &str, seq: u64, target: &str) -> Vec<String> {
        selected_in(narrator, from, seq, target, seq)
    }

    /// A player's memory of hearing round `round`'s `phase` announced, with
    /// `alive` alive and `known` roles shown.
    fn heard(round: u8, phase: Phase, alive: &[&str], known: &[(&str, Role)]) -> Event<Memory> {
        let seq = seq_of(round, &phase);
        logged(Memory::Observed {
            from: ENVIRONMENT.to_string(),
            message: announcement(seq, observed(round, phase, alive, known)),
        })
    }

    /// `me`'s memory of selecting `target` for the phase numbered `seq`.
    fn chose(me: &str, seq: u64, target: &str) -> Event<Memory> {
        logged(Memory::Acted {
            message: choice(me, seq, target),
        })
    }

    /// The werewolves elected `victim` the first night, whom `protectors`
    /// protected.
    fn hunted(victim: &str, protectors: &[&str]) -> Event<Entry> {
        logged(Entry::Hunted {
            seq: 1,
            hunt: Hunt {
                victim: id(victim),
                protectors: protectors.iter().map(|name| id(name)).collect(),
            },
        })
    }

    /// The game ended at once, so far as its log says.
    fn ended(winner: Team, days: u8, survivors: &[&str]) -> Event<Entry> {
        ended_after(winner, days, survivors, Duration::ZERO)
    }

    /// The game ended, having `took` as long as it did.
    fn ended_after(winner: Team, days: u8, survivors: &[&str], took: Duration) -> Event<Entry> {
        logged(Entry::End {
            winner,
            days,
            survivors: survivors.iter().map(|name| id(name)).collect(),
            took,
        })
    }

    /// The deal of `roles`, with a night waiting a minute and a day two.
    fn dealt(roles: HashMap<PlayerId, Role>) -> Event<Entry> {
        let rules = Rules {
            day_limit: Duration::from_secs(120),
            ..Rules::default()
        };
        logged(rules.start(roles))
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
        assert!(selected(&mut narrator, "wolf", 1, "ann").is_empty());
        assert!(selected(&mut narrator, "doctor", 1, "seer").is_empty());
        assert!(selected(&mut narrator, "seer", 1, "wolf").is_empty());
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
        selected(&mut narrator, "wolf", 1, "ann");
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
        selected(&mut narrator, "wolf", 1, "ann");
        selected(&mut narrator, "doctor", 1, "ann");
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
        let known = [("doctor", Role::Doctor)];
        narrator.recall(&heard(1, Phase::Night, &everyone, &known));
        assert!(narrator.narrate(&hunted("ann", &["doctor"])).is_empty());
        assert_eq!(
            narrator.recall(&heard(1, Phase::Day, &everyone, &known)),
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
        selected(&mut narrator, "player10", 2, "player2");
        selected(&mut narrator, "player2", 2, "player10");
        selected(&mut narrator, "player1", 2, "player2");
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
                "The game took 0s.",
                "Survivors:",
                "player2 (werewolf)"
            ]
        );
    }

    #[test]
    fn the_end_tells_how_long_the_whole_game_took_to_the_nearest_second() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "ann", "seer", "doctor"],
        ));
        let told = narrator.narrate(&ended_after(
            Team::Werewolves,
            0,
            &["wolf", "ann"],
            Duration::from_millis(95_400),
        ));
        assert_eq!(
            told[told.len() - 5..],
            [
                "The werewolves win after 0 days.",
                "The game took 1m 35s.",
                "Survivors:",
                "ann (villager)",
                "wolf (werewolf)",
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
        assert!(selected(&mut narrator, "wolf", 2, "seer").is_empty());
        assert!(selected(&mut narrator, "seer", 2, "wolf").is_empty());
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
                "The game took 0s.",
                "Survivors:",
                "doctor (doctor)",
                "seer (seer)"
            ]
        );
    }

    #[test]
    fn a_selection_the_environment_observed_in_a_later_phase_is_told_as_too_late() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone));
        narrator.narrate(&announced("seer", 1, 1, Phase::Night, &everyone));
        selected(&mut narrator, "wolf", 1, "ann");
        narrator.narrate(&announced(
            "wolf",
            2,
            1,
            Phase::Day,
            &["wolf", "seer", "doctor"],
        ));
        assert_eq!(
            selected_in(&mut narrator, "seer", 1, "wolf", 2),
            ["seer (seer) chooses wolf (werewolf) for night 1 too late to count."]
        );
        // A selection in time is held for the end of its phase.
        assert!(selected(&mut narrator, "seer", 2, "wolf").is_empty());
    }

    #[test]
    fn a_selection_for_a_phase_never_announced_is_told_as_too_late_without_naming_one() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone));
        assert_eq!(
            selected_in(&mut narrator, "wolf", 0, "ann", 1),
            ["wolf (werewolf) chooses ann (villager) too late to count."]
        );
    }

    #[test]
    fn an_observation_of_a_message_whose_action_is_not_a_selection_adds_nothing() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone));
        // The wolf's observation of the announcement.
        let announcement = logged(Entry::Observation {
            by: id("wolf"),
            from: ENVIRONMENT.to_string(),
            n: 1,
            seq: 1,
        });
        assert!(narrator.narrate(&announcement).is_empty());
        // An observation of a selection whose action never reached the log.
        assert!(
            narrator
                .narrate(&observed_by_the_environment("wolf", 7, 1))
                .is_empty()
        );
    }

    #[test]
    fn a_player_is_told_its_own_selection_came_too_late() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.recall(&heard(1, Phase::Night, &everyone, &[]));
        narrator.recall(&heard(1, Phase::Day, &everyone, &[]));
        narrator.recall(&heard(2, Phase::Night, &everyone, &[]));
        assert_eq!(
            narrator.recall(&chose("doctor", 2, "ann")),
            ["You choose ann for day 1 too late to count."]
        );
    }

    #[test]
    fn a_memory_that_is_neither_an_announcement_heard_nor_a_selection_made_adds_nothing() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let reminder = logged(Memory::Observed {
            from: ENVIRONMENT.to_string(),
            message: Message::EndPhase { seq: 1 },
        });
        assert!(narrator.recall(&reminder).is_empty());
    }

    /// `player` failed to select in the phase numbered `seq`, the server
    /// having refused the request.
    fn failed(player: &str, seq: u64) -> Event<Entry> {
        logged(Entry::Failed {
            player: id(player),
            seq,
            error: "POST http://localhost:1234/v1/chat/completions was answered 400 Bad Request"
                .to_string(),
        })
    }

    #[test]
    fn a_failure_to_select_is_told_as_it_happens_with_what_went_wrong() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone));
        assert_eq!(
            narrator.narrate(&failed("wolf", 1)),
            ["wolf (werewolf) fails to select: POST \
              http://localhost:1234/v1/chat/completions was answered 400 Bad Request"]
        );
        // A failure that comes once a later phase has begun names its own.
        narrator.narrate(&announced("wolf", 2, 1, Phase::Day, &everyone));
        assert_eq!(
            narrator.narrate(&failed("wolf", 1)),
            ["wolf (werewolf) fails to select for night 1: POST \
              http://localhost:1234/v1/chat/completions was answered 400 Bad Request"]
        );
    }

    #[test]
    fn a_player_is_not_told_of_its_own_failure_to_select() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert!(narrator.narrate(&failed("wolf", 1)).is_empty());
    }

    #[test]
    fn an_action_that_is_neither_an_announcement_nor_a_selection_is_passed_over() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let reminder = logged(Entry::Action {
            from: id("wolf"),
            n: 1,
            to: to_the_environment(),
            message: Message::EndPhase { seq: 1 },
            seen: None,
        });
        assert!(narrator.narrate(&reminder).is_empty());
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
        selected(&mut narrator, "seer", 1, "stranger");
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
                "The game took 0s.",
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
                "The game took 0s.",
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
                "The game took 0s.",
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
            narrator.recall(&heard(1, Phase::Night, &everyone, &pack)),
            [
                "You are wolf, a werewolf. The werewolves are bob and wolf.",
                "",
                "Night 1."
            ]
        );
        assert_eq!(
            narrator.recall(&chose("wolf", 1, "ann")),
            ["You vote to kill ann."]
        );
        assert_eq!(
            narrator.recall(&heard(
                1,
                Phase::Day,
                &["wolf", "bob", "seer", "doctor"],
                &pack
            )),
            ["ann dies.", "", "Day 1."]
        );
        assert_eq!(
            narrator.recall(&chose("wolf", 2, "seer")),
            ["You vote against seer."]
        );
    }

    #[test]
    fn a_werewolf_without_a_pack_is_told_it_is_alone() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert_eq!(
            narrator.recall(&heard(
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
            narrator.recall(&heard(1, Phase::Day, &["wolf", "ann"], &[])),
            ["You are ann.", "", "Day 1."]
        );
    }

    #[test]
    fn the_seer_asks_by_night_and_learns_by_morning() {
        let mut narrator = Narrator::for_player(id("seer"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.recall(&heard(1, Phase::Night, &everyone, &[("seer", Role::Seer)])),
            ["You are seer, the seer.", "", "Night 1."]
        );
        assert_eq!(
            narrator.recall(&chose("seer", 1, "wolf")),
            ["You ask about wolf."]
        );
        assert_eq!(
            narrator.recall(&heard(
                1,
                Phase::Day,
                &["wolf", "seer", "doctor"],
                &[("seer", Role::Seer), ("wolf", Role::Werewolf)]
            )),
            ["You learn wolf is a werewolf.", "ann dies.", "", "Day 1."]
        );
        assert_eq!(
            narrator.recall(&chose("seer", 2, "wolf")),
            ["You vote against wolf (werewolf)."]
        );
        assert_eq!(
            narrator.recall(&heard(
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
            narrator.recall(&heard(1, Phase::Night, &everyone, &known)),
            ["You are doctor, the doctor.", "", "Night 1."]
        );
        assert_eq!(
            narrator.recall(&chose("doctor", 1, "seer")),
            ["You protect seer."]
        );
        assert_eq!(
            narrator.recall(&heard(1, Phase::Day, &everyone, &known)),
            ["Nobody dies.", "", "Day 1."]
        );
        assert_eq!(
            narrator.recall(&chose("doctor", 2, "wolf")),
            ["You vote against wolf."]
        );
    }

    #[test]
    fn a_villager_sleeps_through_the_nights() {
        let mut narrator = Narrator::for_player(id("ann"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("ann", Role::Villager)];
        assert_eq!(
            narrator.recall(&heard(1, Phase::Day, &everyone, &known)),
            ["You are ann, a villager.", "", "Day 1."]
        );
        assert_eq!(
            narrator.recall(&chose("ann", 2, "wolf")),
            ["You vote against wolf."]
        );
        assert_eq!(
            narrator.recall(&heard(2, Phase::Day, &["seer", "doctor", "ann"], &known)),
            ["wolf died.", "", "Day 2."]
        );
        assert_eq!(
            narrator.recall(&heard(3, Phase::Day, &["seer", "doctor", "ann"], &known)),
            ["Nobody died.", "", "Day 3."]
        );
    }

    #[test]
    fn the_logs_actions_and_observations_add_nothing_to_a_players_account() {
        // A player's account is told from its memory, whoever the log's
        // entries are about.
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let pack = [("wolf", Role::Werewolf)];
        assert!(
            narrator
                .narrate(&told("wolf", 1, Phase::Night, &everyone, &pack))
                .is_empty()
        );
        assert!(
            narrator
                .narrate(&told("seer", 1, Phase::Night, &everyone, &pack))
                .is_empty()
        );
        assert!(narrator.narrate(&acted("wolf", 1, "ann")).is_empty());
        assert!(
            narrator
                .narrate(&observed_by_the_environment("wolf", 1, 1))
                .is_empty()
        );
    }

    #[test]
    fn a_player_names_the_survivors_it_knows_the_roles_of() {
        let mut narrator = Narrator::for_player(id("seer"));
        narrator.recall(&heard(
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
                "The game took 0s.",
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
                "The game took 0s.",
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
            narrator.recall(&at(10.0, heard(1, Phase::Night, &everyone, &known))),
            ["You are wolf, the only werewolf.", "", "Night 1."]
        );
        assert_eq!(
            narrator.recall(&at(50.0, heard(1, Phase::Day, &everyone, &known))),
            ["Nobody dies.", "", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&at(90.0, ended(Team::Werewolves, 1, &["wolf", "ann"]))),
            [
                "doctor and seer die.",
                "",
                "The werewolves win after 1 day.",
                "The game took 0s.",
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
