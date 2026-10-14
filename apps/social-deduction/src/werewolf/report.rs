//! A legible account of a game: of everything, told from the log as it is
//! written, or of what one player saw, told from the player's memory.
//!
//! The log is written in actions and observations, by every actor. The
//! omniscient account reads both: an announcement is told from the
//! environment's action, and a selection when the environment's
//! observation of it is logged, joined to the player's action by the
//! player and its number for the message. A player's memory holds the
//! messages themselves, and its account reads them.

use super::prompted::Memory;
use super::{
    Ending, Entry, Hunt, Limits, Message, Move, Outcome, Phase, Role, Team, View, Werewolf, awake,
    ratified,
};
use crate::game::Narrate;
use crate::prompted::Account;
use crate::report::{
    Line, Seat, configured, in_order, in_prose, late, seat_order, told_within, whole_seconds,
};
use crate::seat::{ENVIRONMENT, PlayerId};
use free_agent::{ActorId, Event};
use humantime::format_duration;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
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
/// and tells the table, a player to a line with its role and strategy, how
/// long a night and a day wait, and how long a vote must stand if the
/// day's vote is ratified, then what it was told of each model in play,
/// each night and day with what every player did, how it ended, and how
/// long it took, then who won, how long the whole game took, and the
/// survivors, a player to a line. A player's narrator reads the player's
/// memory through [`Narrator::recall`] and tells the same nights and days
/// from what that player was shown, in the second person, with no table,
/// no limits, and no times.
/// Each names a player with its role in parentheses whenever it knows the
/// role, except in reporting its death, and lists players in seat order.
/// A blank line sets each phase off from what came before it, and another
/// the winner from the last.
/// Both learn who died from who is shown as alive next, so a phase's result
/// is told as the next one begins, and the omniscient narrator holds the
/// phase's deeds until then too, to tell them in seat order, then whom the
/// werewolves chose and who protected them, which no player is shown, or
/// how a day's vote ended; a player's narrator tells the player's own deed
/// as it is done. In a day whose vote is ratified, votes may change or be
/// retracted and their order matters, so the omniscient narrator tells each
/// vote and retraction as the environment observes it, and both tell each
/// tentative decision as it is announced, and a player's narrator tells the
/// other players' votes and retractions as the player hears them, and as
/// the day ends its tally: each player alive that day, in seat order, with
/// its vote standing or that it did not vote, which takes the place of the
/// day's votes, retractions and tentative decisions in the player's
/// account. In a day whose vote is not ratified, a player's first vote
/// stands, and both tell a later one as not counting. Both tell a
/// retraction with no vote standing for it to take back as not counting.
/// A phase begins at its first announcement and ends at the next phase's,
/// or at the end of the game, and the omniscient
/// narrator tells the time between, to the nearest second, after who died.
/// The omniscient narrator tells a selection or retraction when the
/// environment logs observing it, joined to its own action by the player
/// and its number for the message, as a deed of the phase it was for, when
/// the environment observed it in that phase. The other players'
/// observations of a public vote add nothing. A player's failure to act is
/// told by the omniscient narrator as it happens, with what went wrong, as
/// a failure to select by night and to play by day, and for the phase it
/// was for if that phase has ended; a player's narrator leaves it out,
/// since the player is not told of it. Talk is told as it comes: the
/// omniscient narrator tells each utterance from its speaker's action, in
/// order with the day's votes, while its phase lasts, and a player's
/// narrator tells each it made or heard. A message that comes after its
/// phase has ended, whether withheld or observed late, stays in the log
/// and out of the account. A player's memory holds only the phases it
/// lived through, in order, so nothing in it is late.
#[derive(Debug, Default)]
pub struct Narrator {
    /// Whose account this is.
    voice: Voice,
    /// Every role known so far: from the deal, and from what players are
    /// shown.
    roles: HashMap<PlayerId, Role>,
    /// Every player's strategy, from the deal.
    strategies: HashMap<PlayerId, String>,
    /// Who was alive when the current phase began.
    alive: HashSet<PlayerId>,
    /// The phase being told, once one has begun.
    phase: Option<Told>,
    /// Whether the day's vote is ratified: in the omniscient account, as
    /// the deal says; in a player's, as the player was told.
    ratified: bool,
    /// The deeds of the phase being told, each with whose it is, held in
    /// the omniscient account until the phase ends.
    deeds: Vec<(PlayerId, String)>,
    /// What came of the phase, the night's hunt or how the day's vote
    /// ended, held in the omniscient account until the phase ends.
    resolution: Option<String>,
    /// Every phase announced so far, by its number.
    announced: BTreeMap<u64, (NonZero<u8>, Phase)>,
    /// Every ballot logged as its player's action and not yet observed by
    /// the environment, by the player and its number for the message: the
    /// phase it was for, and the ballot.
    ballots: HashMap<(ActorId, u64), (u64, Ballot)>,
    /// Each player's vote standing in the day being told, for a player's
    /// account's tally, and to tell whether a retraction takes one back:
    /// its latest in a day whose vote is ratified, and otherwise its first.
    votes: HashMap<PlayerId, PlayerId>,
    /// In a player's account, the lines told so far, in order, each with
    /// what it is.
    account: Vec<(String, Kind)>,
}

/// A player's selection or retraction, as cast.
#[derive(Debug)]
enum Ballot {
    /// A selection of this player.
    Vote(PlayerId),
    /// A retraction of the player's vote.
    Retract,
}

/// What a line of a player's account is, which decides what becomes of it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Something a player said, which a story may forget.
    Talk,
    /// A vote or a tentative decision of the day being told, which the
    /// day's tally takes the place of once the day is past.
    Passing,
    /// Anything else, which stays.
    Kept,
}

/// The phase being told: which it is, and when it began.
#[derive(Debug)]
struct Told {
    /// The round and its half.
    phase: (NonZero<u8>, Phase),
    /// When the phase was first announced.
    began: SystemTime,
}

impl Narrate<Werewolf> for Narrator {
    /// The lines the log's `event` adds to the account, often none. The
    /// omniscient account is told from the log; a player's reads only the
    /// end of the game from it.
    fn narrate(&mut self, event: &Event<Entry>) -> Vec<Line> {
        let time = event.time;
        if let Entry::Event(Outcome::End {
            winner,
            days,
            survivors,
            took,
        }) = &event.payload
        {
            return self.end(*winner, *days, survivors, *took, time);
        }
        if let Voice::Player(_) = self.voice {
            return vec![];
        }
        match &event.payload {
            Entry::Start {
                roles,
                strategies,
                limits,
            } => {
                self.roles = roles.clone();
                self.strategies = strategies.clone();
                self.alive = roles.keys().cloned().collect();
                self.ratified = ratified(limits.ratify_limit);
                self.table(limits)
            }
            Entry::Action {
                message: Message::Announce { seq, view, .. },
                ..
            } => self.announce(*seq, view, time),
            Entry::Action {
                from,
                n,
                message: Message::Move(Move::Select { seq, target, .. }),
                ..
            } => {
                self.ballots
                    .insert((from.clone(), *n), (*seq, Ballot::Vote(target.clone())));
                vec![]
            }
            Entry::Action {
                from,
                n,
                message: Message::Move(Move::Retract { seq, .. }),
                ..
            } => {
                self.ballots
                    .insert((from.clone(), *n), (*seq, Ballot::Retract));
                vec![]
            }
            Entry::Action {
                message: Message::Move(Move::Tentative { target, ratify, .. }),
                ..
            } => vec![Line::Told(self.tentative(target.as_ref(), *ratify))],
            Entry::Action {
                message: Message::Say {
                    seq, from, text, ..
                },
                ..
            } => (!late(&self.announced, *seq))
                .then(|| self.said(from, text))
                .into_iter()
                .collect(),
            Entry::Observation { by, from, n, seq } if by == ENVIRONMENT => {
                match self.ballots.remove(&(from.clone(), *n)) {
                    Some((sent_in, Ballot::Vote(chosen))) if sent_in == *seq => {
                        self.did(from, &chosen)
                    }
                    Some((sent_in, Ballot::Retract)) if sent_in == *seq => self.withdrew(from),
                    _ => vec![],
                }
            }
            Entry::Event(Outcome::Hunted { hunt, .. }) => {
                self.resolution = Some(self.hunt(hunt));
                vec![]
            }
            Entry::Event(Outcome::Voted { by, .. }) => {
                self.resolution = match by {
                    Ending::Ratified => Some("The votes stand.".to_string()),
                    Ending::DayLimit => Some("The day runs out.".to_string()),
                    Ending::AllSelected => None,
                };
                vec![]
            }
            Entry::Configuration(configuration) => configured(&configuration.models),
            Entry::Failed { player, seq, error } => {
                let player = self.seat(player);
                let verb = match self.announced.get(seq) {
                    Some((_, Phase::Day)) => "play",
                    _ => "select",
                };
                vec![Line::Told(
                    match late(&self.announced, *seq)
                        .then(|| self.phase_name(*seq))
                        .flatten()
                    {
                        Some(phase) => format!("{player} fails to {verb} for {phase}: {error}"),
                        None => format!("{player} fails to {verb}: {error}"),
                    },
                )]
            }
            _ => vec![],
        }
    }
}

impl Narrator {
    /// The account `me` would give of its own game. Feed it `me`'s
    /// memories through [`Narrator::recall`]: each announcement `me`
    /// heard, and each selection it made. It opens with who `me` is, names
    /// the roles `me` knows and nobody else's, and tells `me`'s deeds as
    /// "You ...", in a game whose day's vote is `ratified` or not.
    pub fn for_player(me: PlayerId, ratified: bool) -> Self {
        Self {
            voice: Voice::Player(me),
            ratified,
            ..Self::default()
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
    ) -> Vec<Line> {
        let mut lines = self.outcome(survivors, false, time);
        lines.push(Line::Blank);
        lines.push(Line::Told(won(winner, days)));
        lines.push(Line::Told(format!(
            "The game took {}.",
            whole_seconds(took)
        )));
        lines.push(Line::Told("Survivors:".to_string()));
        lines.extend(
            in_order(survivors)
                .into_iter()
                .map(|player| Line::Told(self.seat(player).to_string())),
        );
        lines
    }

    /// The lines a player's memory `event` adds to its account, often
    /// none: an announcement it heard begins a phase, and one of a phase it
    /// sleeps through says so; a selection or retraction it made, or heard
    /// another player make, is a deed; an utterance it made, or heard, is told as said;
    /// and a tentative decision it heard is told as announced. A player
    /// remembers nothing from a phase other than the one it was in, so
    /// nothing it remembers is late. The lines join the player's
    /// [account](Narrator::account), where the day's tally takes the place
    /// of its votes and tentative decisions.
    pub fn recall(&mut self, event: &Event<Memory>) -> Vec<String> {
        let kind = match &event.payload {
            Memory::Acted {
                message: Message::Say { .. },
            }
            | Memory::Observed {
                message: Message::Say { .. },
            } => Kind::Talk,
            memory if self.passes(memory) => Kind::Passing,
            _ => Kind::Kept,
        };
        let lines: Vec<String> = self.remember(event).iter().map(Line::to_string).collect();
        self.account
            .extend(lines.iter().map(|text| (text.clone(), kind)));
        lines
    }

    /// A player's account so far: the current phase in full, and each past
    /// day with its talk and its tally in place of its votes and tentative
    /// decisions.
    pub fn account(&self) -> Vec<&str> {
        self.account.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// The lines a player's memory `event` adds, as [`Narrator::recall`]
    /// tells them.
    fn remember(&mut self, event: &Event<Memory>) -> Vec<Line> {
        match &event.payload {
            Memory::Observed {
                message: Message::Announce { seq, view, .. },
            } => self.announce(*seq, view, event.time),
            Memory::Observed {
                message: Message::Move(Move::Tentative { target, ratify, .. }),
            } => vec![Line::Told(self.tentative(target.as_ref(), *ratify))],
            Memory::Acted {
                message: Message::Move(Move::Select { from, target, .. }),
            }
            | Memory::Observed {
                message: Message::Move(Move::Select { from, target, .. }),
            } => self.did(from, target),
            Memory::Acted {
                message: Message::Move(Move::Retract { from, .. }),
            }
            | Memory::Observed {
                message: Message::Move(Move::Retract { from, .. }),
            } => self.withdrew(from),
            Memory::Acted {
                message: Message::Say { from, text, .. },
            }
            | Memory::Observed {
                message: Message::Say { from, text, .. },
            } => vec![self.said(from, text)],
            _ => vec![],
        }
    }

    /// Whether what `memory` adds to a player's account gives way to the
    /// day's tally once the day is past: a vote, a retraction or a
    /// tentative decision made or heard by day.
    fn passes(&self, memory: &Memory) -> bool {
        self.by_day()
            && matches!(
                memory,
                Memory::Acted {
                    message: Message::Move(Move::Select { .. })
                        | Message::Move(Move::Retract { .. })
                } | Memory::Observed {
                    message: Message::Move(Move::Select { .. })
                        | Message::Move(Move::Retract { .. })
                        | Message::Move(Move::Tentative { .. })
                }
            )
    }

    /// The environment's tentative decision: `target` will be voted out
    /// once the votes have selected them for `ratify`, or nobody is about
    /// to be.
    fn tentative(&self, target: Option<&PlayerId>, ratify: Duration) -> String {
        match target {
            Some(target) => format!(
                "{} will be voted out in {} unless the votes stop selecting them.",
                self.seat(target),
                format_duration(ratify)
            ),
            None => "Nobody is about to be voted out.".to_string(),
        }
    }

    /// What the announcement at `time` of the phase numbered `seq`, showing
    /// `view`, adds to the account.
    fn announce(&mut self, seq: u64, view: &View, time: SystemTime) -> Vec<Line> {
        self.announced.insert(seq, (view.round, view.phase.clone()));
        self.show(view, time)
    }

    /// The deal: how many players, then each with its role and its
    /// strategy on a line of its own, then how long a night and a day wait
    /// for them, and how long a vote must stand if the day's vote is
    /// ratified, which it is when the ratify limit is above zero.
    fn table(&self, limits: &Limits) -> Vec<Line> {
        let Limits {
            night_limit,
            day_limit,
            ratify_limit,
        } = *limits;
        let heading = format!("{} players:", self.roles.len());
        let seats = in_order(self.roles.keys()).into_iter().map(|player| {
            format!(
                "{player}, a {}, plays {}",
                self.roles[player], self.strategies[player]
            )
        });
        let mut limits = format!(
            "A night waits {} and a day {}",
            format_duration(night_limit),
            format_duration(day_limit)
        );
        if ratified(ratify_limit) {
            write!(
                limits,
                ", and a vote stands after {}",
                format_duration(ratify_limit)
            )
            .unwrap();
        }
        limits.push('.');
        std::iter::once(heading)
            .chain(seats)
            .chain(std::iter::once(limits))
            .map(Line::Told)
            .collect()
    }

    /// `player` as this account names it: with its role when known.
    fn seat<'a>(&self, player: &'a PlayerId) -> Seat<'a, Role> {
        Seat::named(player, &self.roles)
    }

    /// Takes in the roles an view shows that were not known. In a
    /// player's account, its first view says who it is, and a role
    /// it is shown later is one it learned.
    fn learn(&mut self, roles: &HashMap<PlayerId, Role>) -> Vec<Line> {
        let new: Vec<(&PlayerId, Role)> = in_order(roles.keys())
            .into_iter()
            .filter(|player| !self.roles.contains_key(*player))
            .map(|player| (player, roles[player]))
            .collect();
        let lines = match &self.voice {
            Voice::Omniscient => vec![],
            Voice::Player(me) if self.phase.is_none() => vec![Line::Told(identity(me, roles))],
            Voice::Player(_) => new
                .iter()
                .map(|(player, role)| Line::Told(format!("You learn {player} is a {role}.")))
                .collect(),
        };
        self.roles
            .extend(new.into_iter().map(|(player, role)| (player.clone(), role)));
        lines
    }

    /// What a player being shown `view` at `time` adds to the
    /// account: the roles it learns, and the phase it begins.
    fn show(&mut self, view: &View, time: SystemTime) -> Vec<Line> {
        let mut lines = self.learn(&view.roles);
        lines.extend(self.begin(view, time));
        lines
    }

    /// A phase begins when a player is first shown it, at `time`. The
    /// phase before it ends then, with whoever is no longer alive, and a
    /// blank line sets the new phase off from what came before it. In a
    /// player's account, a phase the player sleeps through says so.
    fn begin(&mut self, view: &View, time: SystemTime) -> Vec<Line> {
        let phase = (view.round, view.phase.clone());
        if self.phase.as_ref().is_some_and(|told| told.phase == phase) {
            return vec![];
        }
        let slept = self
            .phase
            .as_ref()
            .is_some_and(|told| ordinal(&phase) != ordinal(&told.phase) + 1);
        let mut lines = self.tally();
        lines.extend(self.outcome(&view.alive, slept, time));
        self.alive = view.alive.clone();
        lines.push(Line::Blank);
        lines.push(Line::Heading(format!("{}.", title(&phase))));
        if let Voice::Player(me) = &self.voice
            && !awake(me, view)
        {
            lines.push(Line::Told("You sleep.".to_string()));
        }
        self.phase = Some(Told { phase, began: time });
        lines
    }

    /// How the phase being told ended at `time`, given who is alive after
    /// it: the deeds held for it, the hunt if there was one or how the
    /// day's vote ended, then who died, by name alone and in the same words
    /// by night and by day, then in the omniscient account how long the
    /// phase took. A player that `slept` through phases since is told who
    /// died in the past tense. Several dead take a plural verb.
    fn outcome(
        &mut self,
        alive_after: &HashSet<PlayerId>,
        slept: bool,
        time: SystemTime,
    ) -> Vec<Line> {
        let mut lines = self.done();
        lines.extend(self.resolution.take().map(Line::Told));
        let Some(told) = &self.phase else {
            return lines;
        };
        let dead = in_order(self.alive.difference(alive_after));
        let listed = in_prose(&dead);
        let line = match (slept, dead.len()) {
            (true, 0) => "Nobody died.".to_string(),
            (true, _) => format!("{listed} died."),
            (false, 0) => "Nobody dies.".to_string(),
            (false, 1) => format!("{listed} dies."),
            (false, _) => format!("{listed} die."),
        };
        self.alive = alive_after.clone();
        lines.push(Line::Told(line));
        if matches!(self.voice, Voice::Omniscient) {
            let took = time.duration_since(told.began).unwrap_or_default();
            lines.push(Line::Told(format!(
                "{} took {}.",
                title(&told.phase),
                whole_seconds(took)
            )));
        }
        lines
    }

    /// `player`'s deed of choosing `chosen`, as [`Narrator::tell`] tells
    /// it. By day it is `player`'s vote, unless the day's vote is not
    /// ratified and `player` has voted already, and then it does not count.
    fn did(&mut self, player: &PlayerId, chosen: &PlayerId) -> Vec<Line> {
        let line = self.deed(player, chosen);
        if !self.counts(player) {
            let line = format!(
                "{}, but the vote does not count.",
                line.trim_end_matches('.')
            );
            return self.tell(player, line);
        }
        if self.by_day() {
            self.votes.insert(player.clone(), chosen.clone());
        }
        self.tell(player, line)
    }

    /// Whether a choice by `player` counts: by night, or in a day whose
    /// vote is ratified, or as `player`'s first vote of the day.
    fn counts(&self, player: &PlayerId) -> bool {
        !self.by_day() || self.ratified || !self.votes.contains_key(player)
    }

    /// `player`'s deed of retracting its vote, as [`Narrator::tell`] tells
    /// it: undecided again if the retraction counts, and otherwise a
    /// withdrawal that does not count.
    fn withdrew(&mut self, player: &PlayerId) -> Vec<Line> {
        let (subject, s) = self.subject(player);
        let (whose, is) = if self.is_me(player) {
            ("your", "are")
        } else {
            ("their", "is")
        };
        if !self.withdrawable(player) {
            let line =
                format!("{subject} withdraw{s} {whose} vote, but the withdrawal does not count.");
            return self.tell(player, line);
        }
        self.votes.remove(player);
        let line = format!("{subject} withdraw{s} {whose} vote and {is} undecided again.");
        self.tell(player, line)
    }

    /// Whether `player` has a vote standing that a retraction takes back:
    /// by day, in a day whose vote is ratified.
    fn withdrawable(&self, player: &PlayerId) -> bool {
        self.ratified && self.by_day() && self.votes.contains_key(player)
    }

    /// `line`, `player`'s deed: told at once in a player's account, and in
    /// the omniscient one in a day whose vote is ratified, since the votes
    /// may change and their order matters; otherwise held for the end of
    /// the phase, to be told in seat order.
    fn tell(&mut self, player: &PlayerId, line: String) -> Vec<Line> {
        match self.voice {
            Voice::Omniscient if !(self.ratified && self.by_day()) => {
                self.deeds.push((player.clone(), line));
                vec![]
            }
            _ => vec![Line::Vote(line)],
        }
    }

    /// Whether the phase being told is a day.
    fn by_day(&self) -> bool {
        matches!(&self.phase, Some(told) if told.phase.1 == Phase::Day)
    }

    /// In a player's account, the tally of the day being told, as it ends:
    /// each player alive that day, in seat order, with its vote standing
    /// or that it did not vote. The day's votes and tentative decisions leave
    /// the account, for the tally to take their place.
    fn tally(&mut self) -> Vec<Line> {
        let votes = std::mem::take(&mut self.votes);
        if !(matches!(self.voice, Voice::Player(_)) && self.by_day()) {
            return vec![];
        }
        self.account.retain(|(_, kind)| *kind != Kind::Passing);
        in_order(&self.alive)
            .into_iter()
            .map(|player| {
                Line::Vote(match votes.get(player) {
                    Some(chosen) => self.deed(player, chosen),
                    None if self.is_me(player) => "You do not vote.".to_string(),
                    None => format!("{} does not vote.", self.seat(player)),
                })
            })
            .collect()
    }

    /// What came of the werewolves' `hunt`: whom they chose, and who
    /// protected them if anyone did.
    fn hunt(&self, hunt: &Hunt) -> String {
        let victim = self.seat(&hunt.victim);
        let protectors = in_order(&hunt.protectors);
        let listed = in_prose(protectors.iter().map(|doctor| self.seat(doctor)));
        match protectors.len() {
            0 => format!("The werewolves choose {victim}."),
            1 => format!("The werewolves choose {victim}, whom {listed} protects."),
            _ => format!("The werewolves choose {victim}, whom {listed} protect."),
        }
    }

    /// The deeds held for the phase being told, in seat order.
    fn done(&mut self) -> Vec<Line> {
        let mut deeds = std::mem::take(&mut self.deeds);
        deeds.sort_by(|a, b| seat_order(&a.0, &b.0));
        deeds
            .into_iter()
            .map(|(_, line)| Line::Vote(line))
            .collect()
    }

    /// Whether this is `player`'s own account, in which it is "you".
    fn is_me(&self, player: &PlayerId) -> bool {
        matches!(&self.voice, Voice::Player(me) if me == player)
    }

    /// `player` as the subject of a sentence about its deed, with the
    /// ending its verb takes: "You" and none in the player's own account,
    /// its seat and an s in anyone else's.
    fn subject(&self, player: &PlayerId) -> (String, &'static str) {
        if self.is_me(player) {
            ("You".to_string(), "")
        } else {
            (self.seat(player).to_string(), "s")
        }
    }

    /// `player` says `text`, named as this account names it. In a
    /// player's own account, what it says itself is told in the second
    /// person.
    fn said(&self, player: &PlayerId, text: &str) -> Line {
        if self.is_me(player) {
            return Line::Told(format!("You say, \"{text}\""));
        }
        Line::Said {
            who: player.clone(),
            speaker: self.seat(player).to_string(),
            text: text.to_string(),
        }
    }

    /// The phase numbered `seq` as prose names it mid-sentence, "day 2",
    /// if it has been announced.
    fn phase_name(&self, seq: u64) -> Option<String> {
        self.announced
            .get(&seq)
            .map(|phase| title(phase).to_lowercase())
    }

    /// What `player` did by choosing `chosen`, which depends on the phase
    /// and, by night, on the player's role. A seer's own asking is told as
    /// asking, since it does not know the answer yet; anyone else is told
    /// what the seer learns, when the role is known.
    fn deed(&self, player: &PlayerId, chosen: &PlayerId) -> String {
        let (subject, s) = self.subject(player);
        let role = self.roles.get(player).copied();
        let phase = self.phase.as_ref().map(|told| &told.phase.1);
        let chosen = self.seat(chosen);
        match (phase, role) {
            (Some(Phase::Night), Some(Role::Werewolf)) => {
                format!("{subject} vote{s} to kill {chosen}.")
            }
            (Some(Phase::Night), Some(Role::Doctor)) => format!("{subject} protect{s} {chosen}."),
            (Some(Phase::Night), Some(Role::Seer)) => match (s, chosen.role) {
                ("s", Some(seen)) => format!("{subject} learns {} is a {seen}.", chosen.player),
                _ => format!("{subject} ask{s} about {chosen}."),
            },
            _ => format!("{subject} vote{s} against {chosen}."),
        }
    }
}

/// A player's account of its own game, told a memory at a time: the
/// current phase in full, and each past day with its talk and its tally in
/// place of its votes and tentative decisions. Told within a budget, it
/// forgets talk, oldest first, until it fits.
pub struct Story {
    /// The player's narrator, which has read every memory told and keeps
    /// the account.
    narrator: Narrator,
}

impl Story {
    /// The story `me` tells of its own game, whose day's vote is
    /// `ratified` or not, with nothing in it yet.
    pub fn of(me: PlayerId, ratified: bool) -> Self {
        Self {
            narrator: Narrator::for_player(me, ratified),
        }
    }

    /// The lines told, in order.
    pub fn lines(&self) -> Vec<&str> {
        self.narrator.account()
    }
}

impl Account<Werewolf> for Story {
    fn recall(&mut self, event: &Event<Memory>) {
        self.narrator.recall(event);
    }

    fn told_within(&self, budget: u64) -> (Vec<&str>, usize) {
        told_within(&self.narrator.account, |kind| *kind == Kind::Talk, budget)
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
                    in_prose(pack)
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

/// Where a phase falls in the game, so that consecutive phases are one
/// apart.
fn ordinal((round, phase): &(NonZero<u8>, Phase)) -> u16 {
    let half = match phase {
        Phase::Night => 0,
        Phase::Day => 1,
    };
    u16::from(round.get()) * 2 + half
}

/// That `winner` won after `days` days, as the account tells it: "The
/// villagers win after 1 day."
pub(crate) fn won(winner: Team, days: u8) -> String {
    format!(
        "The {winner} win after {days} {}.",
        if days == 1 { "day" } else { "days" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ToolChoice;
    use crate::report::FORGOTTEN;
    use crate::report::tests::estimated;
    use crate::report::tests::in_play;
    use crate::seat::id;

    use crate::prompted::tests::received;
    use crate::seat::to_the_environment;
    use crate::strategy::{RANDOM, Random, Strategy};
    use crate::werewolf::config::Configuration;
    use crate::werewolf::tests::{
        announcement, logged, retraction, selection, tentative, utterance,
    };
    use crate::werewolf::{Dealt, Rules};
    use std::collections::BTreeMap;
    use std::time::{Duration, UNIX_EPOCH};

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
            forgot: None,
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
            forgot: None,
        })
    }

    /// `by`'s observation, in the phase numbered `observed_in`, of the
    /// message `from` numbered `n`.
    fn observed_by(by: &str, from: &str, n: u64, observed_in: u64) -> Event<Entry> {
        logged(Entry::Observation {
            by: id(by),
            from: id(from),
            n,
            seq: observed_in,
        })
    }

    /// The environment's observation, in the phase numbered `observed_in`,
    /// of the message `from` numbered `n`.
    fn observed_by_the_environment(from: &str, n: u64, observed_in: u64) -> Event<Entry> {
        observed_by(ENVIRONMENT, from, n, observed_in)
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
    ) -> Vec<Line> {
        assert!(narrator.narrate(&acted(from, seq, target)).is_empty());
        narrator.narrate(&observed_by_the_environment(from, seq, observed_in))
    }

    /// Tell `narrator` that `from` selected `target` for the phase numbered
    /// `seq`, which the environment observed in that phase: what it adds.
    fn selected(narrator: &mut Narrator, from: &str, seq: u64, target: &str) -> Vec<Line> {
        selected_in(narrator, from, seq, target, seq)
    }

    /// Tell `narrator` that `from` retracted its vote in the phase numbered
    /// `seq`, its `n`th message, and that the environment observed it in the
    /// phase numbered `observed_in`: what the observation adds, the action
    /// adding nothing.
    fn retracted_in(
        narrator: &mut Narrator,
        from: &str,
        seq: u64,
        n: u64,
        observed_in: u64,
    ) -> Vec<Line> {
        let action = logged(Entry::Action {
            from: id(from),
            n,
            to: to_the_environment(),
            message: retraction(seq, from, n),
            seen: None,
            forgot: None,
        });
        assert!(narrator.narrate(&action).is_empty());
        narrator.narrate(&observed_by_the_environment(from, n, observed_in))
    }

    /// A player's memory of hearing round `round`'s `phase` announced, with
    /// `alive` alive and `known` roles shown.
    fn heard(round: u8, phase: Phase, alive: &[&str], known: &[(&str, Role)]) -> Event<Memory> {
        let seq = seq_of(round, &phase);
        received(announcement(seq, observed(round, phase, alive, known)))
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
        logged(Entry::Event(Outcome::Hunted {
            seq: 1,
            hunt: Hunt {
                victim: id(victim),
                protectors: protectors.iter().map(|name| id(name)).collect(),
            },
        }))
    }

    /// The game ended at once, so far as its log says.
    fn ended(winner: Team, days: u8, survivors: &[&str]) -> Event<Entry> {
        ended_after(winner, days, survivors, Duration::ZERO)
    }

    /// The game ended, having `took` as long as it did.
    fn ended_after(winner: Team, days: u8, survivors: &[&str], took: Duration) -> Event<Entry> {
        logged(Entry::Event(Outcome::End {
            winner,
            days,
            survivors: survivors.iter().map(|name| id(name)).collect(),
            took,
        }))
    }

    /// The deal of `roles`, the werewolves playing the strategy `bold` and
    /// everyone else `random`, with a night waiting a minute and a day two,
    /// and the day's vote ratified after `ratify_limit` if it is above zero.
    fn dealt_under(roles: HashMap<PlayerId, Role>, ratify_limit: Duration) -> Event<Entry> {
        let rules = Rules {
            day_limit: Duration::from_secs(120),
            ratify_limit,
            ..Rules::default()
        };
        let deal = roles
            .into_iter()
            .map(|(player, role)| {
                let strategy = match role {
                    Role::Werewolf => "bold",
                    _ => "random",
                };
                let dealt = Dealt {
                    role,
                    strategy: strategy.to_string(),
                };
                (player, dealt)
            })
            .collect();
        logged(Entry::start(&deal, &rules))
    }

    /// The deal of `roles`, with a night waiting a minute and a day two.
    fn dealt(roles: HashMap<PlayerId, Role>) -> Event<Entry> {
        dealt_under(roles, Duration::ZERO)
    }

    fn start() -> Event<Entry> {
        dealt(roles())
    }

    /// The deal of [`roles`] under rules that ratify the day's vote after
    /// twenty seconds.
    fn ratified() -> Event<Entry> {
        dealt_under(roles(), Duration::from_secs(20))
    }

    /// The environment's action of sending the tentative decision numbered
    /// `n` for the day numbered `seq`, naming `target` or nobody, to
    /// everyone in `to`.
    fn decided(seq: u64, n: u64, target: Option<&str>, to: &[&str]) -> Event<Entry> {
        logged(Entry::Action {
            from: ENVIRONMENT.to_string(),
            n,
            to: to.iter().map(|name| id(name)).collect(),
            message: tentative(seq, n, target),
            seen: None,
            forgot: None,
        })
    }

    /// `from`'s action of saying `text` in the phase numbered `seq`, its
    /// `n`th message, to everyone in `to`.
    fn spoke(from: &str, seq: u64, n: u64, text: &str, to: &[&str]) -> Event<Entry> {
        logged(Entry::Action {
            from: id(from),
            n,
            to: to.iter().map(|name| id(name)).collect(),
            message: utterance(seq, from, n, text),
            seen: Some(3),
            forgot: None,
        })
    }

    /// `from` withheld `message`, which its model made from three memories.
    fn withheld(from: &str, message: Message) -> Event<Entry> {
        logged(Entry::Withheld {
            from: id(from),
            message,
            seen: Some(3),
            forgot: None,
        })
    }

    /// The day numbered `seq` ended `by`, voting `out`.
    fn voted(seq: u64, out: Option<&str>, by: Ending) -> Event<Entry> {
        logged(Entry::Event(Outcome::Voted {
            seq,
            out: out.map(id),
            by,
        }))
    }

    #[test]
    fn the_table_is_told_first_then_the_limits() {
        let mut narrator = Narrator::default();
        assert_eq!(
            narrator.narrate(&start()),
            [
                "4 players:",
                "ann, a villager, plays random",
                "doctor, a doctor, plays random",
                "seer, a seer, plays random",
                "wolf, a werewolf, plays bold",
                "A night waits 1m and a day 2m."
            ]
        );
    }

    #[test]
    fn the_limits_line_adds_the_ratify_limit_when_it_is_above_zero() {
        let mut narrator = Narrator::default();
        assert_eq!(
            narrator.narrate(&ratified())[5],
            "A night waits 1m and a day 2m, and a vote stands after 20s."
        );
    }

    #[test]
    fn in_a_ratified_day_each_vote_is_told_as_it_comes_and_each_tentative_decision_as_announced() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        // Seat order is not kept: the votes are told in the order the
        // environment observed them.
        assert_eq!(
            selected(&mut narrator, "wolf", 2, "ann"),
            ["wolf (werewolf) votes against ann (villager)."]
        );
        assert_eq!(
            narrator.narrate(&decided(2, 5, Some("ann"), &everyone)),
            ["ann (villager) will be voted out in 20s unless the votes stop selecting them."]
        );
        assert_eq!(
            selected(&mut narrator, "ann", 2, "wolf"),
            ["ann (villager) votes against wolf (werewolf)."]
        );
        assert_eq!(
            narrator.narrate(&decided(2, 6, None, &everyone)),
            ["Nobody is about to be voted out."]
        );
        // A change of vote is told again.
        assert_eq!(
            selected_in(&mut narrator, "ann", 2, "ann", 2),
            ["ann (villager) votes against ann (villager)."]
        );
        assert_eq!(
            narrator.narrate(&decided(2, 7, Some("ann"), &everyone)),
            ["ann (villager) will be voted out in 20s unless the votes stop selecting them."]
        );
    }

    #[test]
    fn in_a_ratified_day_a_retraction_is_told_as_it_comes_in_order_with_the_votes() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        selected(&mut narrator, "wolf", 2, "ann");
        narrator.narrate(&decided(2, 5, Some("ann"), &everyone));
        assert_eq!(
            retracted_in(&mut narrator, "wolf", 2, 3, 2),
            ["wolf (werewolf) withdraws their vote and is undecided again."]
        );
        assert_eq!(
            narrator.narrate(&decided(2, 6, None, &everyone)),
            ["Nobody is about to be voted out."]
        );
        // A retraction the environment observed in a later phase goes
        // untold.
        narrator.narrate(&shown("wolf", 2, Phase::Night, &everyone));
        assert!(retracted_in(&mut narrator, "seer", 2, 4, 3).is_empty());
    }

    #[test]
    fn a_retraction_without_a_vote_standing_is_told_as_not_counting() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        assert_eq!(
            retracted_in(&mut narrator, "seer", 2, 1, 2),
            ["seer (seer) withdraws their vote, but the withdrawal does not count."]
        );
        // Once withdrawn, a vote is not there to withdraw again.
        selected(&mut narrator, "wolf", 2, "ann");
        retracted_in(&mut narrator, "wolf", 2, 3, 2);
        assert_eq!(
            retracted_in(&mut narrator, "wolf", 2, 4, 2),
            ["wolf (werewolf) withdraws their vote, but the withdrawal does not count."]
        );
    }

    #[test]
    fn a_retraction_by_night_or_in_a_day_not_ratified_is_told_as_not_counting() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        selected(&mut narrator, "wolf", 1, "ann");
        assert!(retracted_in(&mut narrator, "wolf", 1, 2, 1).is_empty());
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone))[..2],
            [
                "wolf (werewolf) votes to kill ann (villager).",
                "wolf (werewolf) withdraws their vote, but the withdrawal does not count.",
            ]
        );
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        selected(&mut narrator, "wolf", 2, "ann");
        assert!(retracted_in(&mut narrator, "wolf", 2, 3, 2).is_empty());
        assert_eq!(
            narrator.narrate(&shown("wolf", 2, Phase::Night, &everyone))[..2],
            [
                "wolf (werewolf) votes against ann (villager).",
                "wolf (werewolf) withdraws their vote, but the withdrawal does not count.",
            ]
        );
    }

    #[test]
    fn an_utterance_is_told_as_it_is_said_in_order_with_the_votes() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        let others = ["seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&spoke("wolf", 2, 1, "I was with ann all night.", &others)),
            ["wolf (werewolf) says, \"I was with ann all night.\""]
        );
        assert_eq!(
            selected(&mut narrator, "wolf", 2, "ann"),
            ["wolf (werewolf) votes against ann (villager)."]
        );
        assert_eq!(
            narrator.narrate(&spoke("ann", 2, 1, "Liar.", &["wolf", "seer", "doctor"])),
            ["ann (villager) says, \"Liar.\""]
        );
    }

    #[test]
    fn a_withheld_message_goes_untold() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        narrator.narrate(&shown("wolf", 2, Phase::Night, &everyone));
        for message in [
            utterance(2, "wolf", 0, "Wait!"),
            selection(2, "wolf", 0, "ann"),
            retraction(2, "wolf", 0),
        ] {
            assert!(narrator.narrate(&withheld("wolf", message)).is_empty());
        }
    }

    #[test]
    fn a_player_is_told_what_is_said_and_what_it_says() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        narrator.recall(&heard(1, Phase::Day, &everyone, &known));
        assert_eq!(
            narrator.recall(&received(utterance(2, "ann", 1, "I suspect wolf."))),
            ["ann says, \"I suspect wolf.\""]
        );
        let said = logged(Memory::Acted {
            message: utterance(2, "doctor", 1, "So do I."),
        });
        assert_eq!(narrator.recall(&said), ["You say, \"So do I.\""]);
    }

    #[test]
    fn an_utterance_from_a_phase_that_has_ended_goes_untold() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        narrator.narrate(&shown("wolf", 2, Phase::Night, &everyone));
        assert!(
            narrator
                .narrate(&spoke("seer", 2, 1, "Wait!", &["wolf", "doctor", "ann"]))
                .is_empty()
        );
    }

    #[test]
    fn a_ratified_vote_is_told_as_standing_before_who_dies() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        selected(&mut narrator, "wolf", 2, "ann");
        assert!(
            narrator
                .narrate(&voted(2, Some("ann"), Ending::Ratified))
                .is_empty()
        );
        assert_eq!(
            narrator.narrate(&shown("wolf", 2, Phase::Night, &["wolf", "seer", "doctor"])),
            [
                "The votes stand.",
                "ann dies.",
                "Day 1 took 0s.",
                "",
                "Night 2."
            ]
        );
    }

    #[test]
    fn a_day_that_runs_out_says_so_before_nobody_dies() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        narrator.narrate(&voted(2, None, Ending::DayLimit));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 1, &everyone)),
            [
                "The day runs out.",
                "Nobody dies.",
                "Day 1 took 0s.",
                "",
                "The werewolves win after 1 day.",
                "The game took 0s.",
                "Survivors:",
                "ann (villager)",
                "doctor (doctor)",
                "seer (seer)",
                "wolf (werewolf)"
            ]
        );
    }

    #[test]
    fn a_day_in_which_everyone_selected_is_told_as_it_always_was() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        // Without a ratify limit the votes are held for seat order.
        assert!(selected(&mut narrator, "wolf", 2, "seer").is_empty());
        assert!(selected(&mut narrator, "seer", 2, "wolf").is_empty());
        assert!(
            narrator
                .narrate(&voted(2, None, Ending::AllSelected))
                .is_empty()
        );
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
    fn in_a_ratified_game_the_nights_deeds_are_still_held_for_seat_order() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        assert!(selected(&mut narrator, "wolf", 1, "ann").is_empty());
        assert!(selected(&mut narrator, "doctor", 1, "seer").is_empty());
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"])),
            [
                "doctor (doctor) protects seer (seer).",
                "wolf (werewolf) votes to kill ann (villager).",
                "ann dies.",
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn a_player_is_told_the_others_votes_and_each_tentative_decision() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        narrator.recall(&heard(1, Phase::Day, &everyone, &known));
        assert_eq!(
            narrator.recall(&received(choice("ann", 2, "wolf"))),
            ["ann votes against wolf."]
        );
        assert_eq!(
            narrator.recall(&received(tentative(2, 5, Some("wolf")))),
            ["wolf will be voted out in 20s unless the votes stop selecting them."]
        );
        assert_eq!(
            narrator.recall(&chose("doctor", 2, "wolf")),
            ["You vote against wolf."]
        );
        assert_eq!(
            narrator.recall(&received(choice("wolf", 2, "ann"))),
            ["wolf votes against ann."]
        );
        assert_eq!(
            narrator.recall(&received(tentative(2, 6, None))),
            ["Nobody is about to be voted out."]
        );
    }

    #[test]
    fn a_player_is_told_the_others_retractions_and_its_own() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        narrator.recall(&heard(1, Phase::Day, &everyone, &known));
        narrator.recall(&received(choice("ann", 2, "wolf")));
        assert_eq!(
            narrator.recall(&received(retraction(2, "ann", 3))),
            ["ann withdraws their vote and is undecided again."]
        );
        narrator.recall(&chose("doctor", 2, "wolf"));
        let withdrew = logged(Memory::Acted {
            message: retraction(2, "doctor", 3),
        });
        assert_eq!(
            narrator.recall(&withdrew),
            ["You withdraw your vote and are undecided again."]
        );
    }

    #[test]
    fn a_player_is_told_a_retraction_without_a_vote_standing_as_not_counting() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        narrator.recall(&heard(1, Phase::Night, &everyone, &known));
        narrator.recall(&chose("doctor", 1, "seer"));
        let withdrew = |n| {
            logged(Memory::Acted {
                message: retraction(1, "doctor", n),
            })
        };
        assert_eq!(
            narrator.recall(&withdrew(2)),
            ["You withdraw your vote, but the withdrawal does not count."]
        );
        narrator.recall(&heard(1, Phase::Day, &everyone, &known));
        assert_eq!(
            narrator.recall(&received(retraction(2, "ann", 3))),
            ["ann withdraws their vote, but the withdrawal does not count."]
        );
    }

    #[test]
    fn in_a_day_not_ratified_a_players_tally_keeps_each_first_vote() {
        let mut story = Story::of(id("doctor"), false);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        recall_all(
            &mut story,
            [
                heard(1, Phase::Day, &everyone, &known),
                received(choice("ann", 2, "wolf")),
                chose("doctor", 2, "wolf"),
                received(selection(2, "ann", 3, "seer")),
                logged(Memory::Acted {
                    message: selection(2, "doctor", 3, "ann"),
                }),
            ],
        );
        assert_eq!(
            story.lines()[3..],
            [
                "ann votes against wolf.",
                "You vote against wolf.",
                "ann votes against seer, but the vote does not count.",
                "You vote against ann, but the vote does not count.",
            ]
        );
        story.recall(&heard(2, Phase::Night, &everyone, &known));
        assert_eq!(
            story.lines()[3..7],
            [
                "ann votes against wolf.",
                "You vote against wolf.",
                "seer does not vote.",
                "wolf does not vote.",
            ]
        );
    }

    #[test]
    fn in_a_day_not_ratified_a_player_is_told_a_retraction_as_not_counting() {
        let mut story = Story::of(id("doctor"), false);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        recall_all(
            &mut story,
            [
                heard(1, Phase::Day, &everyone, &known),
                received(choice("ann", 2, "wolf")),
                received(retraction(2, "ann", 3)),
            ],
        );
        assert_eq!(
            story.lines()[4],
            "ann withdraws their vote, but the withdrawal does not count."
        );
        story.recall(&heard(2, Phase::Night, &everyone, &known));
        assert_eq!(story.lines()[3], "ann votes against wolf.");
    }

    #[test]
    fn in_a_day_not_ratified_a_vote_after_a_players_first_is_told_as_not_counting() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        selected(&mut narrator, "wolf", 2, "ann");
        let again = logged(Entry::Action {
            from: id("wolf"),
            n: 3,
            to: to_the_environment(),
            message: selection(2, "wolf", 3, "seer"),
            seen: None,
            forgot: None,
        });
        assert!(narrator.narrate(&again).is_empty());
        assert!(
            narrator
                .narrate(&observed_by_the_environment("wolf", 3, 2))
                .is_empty()
        );
        assert_eq!(
            narrator.narrate(&shown("wolf", 2, Phase::Night, &everyone))[..2],
            [
                "wolf (werewolf) votes against ann (villager).",
                "wolf (werewolf) votes against seer (seer), but the vote does not count.",
            ]
        );
    }

    #[test]
    fn a_retracted_vote_gives_way_to_not_voting_in_the_tally() {
        let mut story = Story::of(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        recall_all(
            &mut story,
            [
                heard(1, Phase::Day, &everyone, &known),
                received(choice("ann", 2, "wolf")),
                chose("doctor", 2, "wolf"),
                received(retraction(2, "ann", 3)),
                logged(Memory::Acted {
                    message: retraction(2, "doctor", 3),
                }),
                received(selection(2, "seer", 2, "ann")),
            ],
        );
        assert_eq!(
            story.lines(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann votes against wolf.",
                "You vote against wolf.",
                "ann withdraws their vote and is undecided again.",
                "You withdraw your vote and are undecided again.",
                "seer votes against ann.",
            ]
        );
        story.recall(&heard(2, Phase::Night, &everyone, &known));
        assert_eq!(
            story.lines(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann does not vote.",
                "You do not vote.",
                "seer votes against ann.",
                "wolf does not vote.",
                "Nobody dies.",
                "",
                "Night 2.",
            ]
        );
    }

    #[test]
    fn a_players_narrator_alone_tells_each_vote_of_a_past_day_once_in_its_tally() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
        for memory in a_day_of_talk_and_votes() {
            narrator.recall(&memory);
        }
        assert_eq!(
            narrator.account(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann says, \"I suspect wolf.\"",
                "ann votes against wolf.",
                "wolf will be voted out in 20s unless the votes stop selecting them.",
                "You vote against wolf.",
                "seer votes against ann.",
                "wolf says, \"Not me.\"",
                "ann votes against seer.",
                "Nobody is about to be voted out.",
            ]
        );
        narrator.recall(&heard(
            2,
            Phase::Night,
            &["seer", "doctor", "ann"],
            &[("doctor", Role::Doctor)],
        ));
        narrator.recall(&chose("doctor", 3, "seer"));
        assert_eq!(
            narrator.account(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann says, \"I suspect wolf.\"",
                "wolf says, \"Not me.\"",
                "ann votes against seer.",
                "You vote against wolf.",
                "seer votes against ann.",
                "wolf does not vote.",
                "wolf dies.",
                "",
                "Night 2.",
                "You protect seer.",
            ]
        );
    }

    /// Tell `story` each of `memories` in order.
    fn recall_all(story: &mut Story, memories: impl IntoIterator<Item = Event<Memory>>) {
        for memory in memories {
            story.recall(&memory);
        }
    }

    /// The doctor's first day: talk, votes, a vote change, and tentative
    /// decisions, with wolf never voting.
    fn a_day_of_talk_and_votes() -> Vec<Event<Memory>> {
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        vec![
            heard(1, Phase::Day, &everyone, &known),
            received(utterance(2, "ann", 1, "I suspect wolf.")),
            received(choice("ann", 2, "wolf")),
            received(tentative(2, 5, Some("wolf"))),
            chose("doctor", 2, "wolf"),
            received(selection(2, "seer", 2, "ann")),
            received(utterance(2, "wolf", 3, "Not me.")),
            received(selection(2, "ann", 4, "seer")),
            received(tentative(2, 6, None)),
        ]
    }

    #[test]
    fn the_current_day_is_told_in_full() {
        let mut story = Story::of(id("doctor"), true);
        recall_all(&mut story, a_day_of_talk_and_votes());
        assert_eq!(
            story.lines(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann says, \"I suspect wolf.\"",
                "ann votes against wolf.",
                "wolf will be voted out in 20s unless the votes stop selecting them.",
                "You vote against wolf.",
                "seer votes against ann.",
                "wolf says, \"Not me.\"",
                "ann votes against seer.",
                "Nobody is about to be voted out.",
            ]
        );
    }

    #[test]
    fn a_past_days_votes_give_way_to_its_tally_in_seat_order_and_its_talk_stays() {
        let mut story = Story::of(id("doctor"), true);
        recall_all(&mut story, a_day_of_talk_and_votes());
        story.recall(&heard(
            2,
            Phase::Night,
            &["seer", "doctor", "ann"],
            &[("doctor", Role::Doctor)],
        ));
        assert_eq!(
            story.lines(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann says, \"I suspect wolf.\"",
                "wolf says, \"Not me.\"",
                "ann votes against seer.",
                "You vote against wolf.",
                "seer votes against ann.",
                "wolf does not vote.",
                "wolf dies.",
                "",
                "Night 2.",
            ]
        );
    }

    #[test]
    fn a_player_that_does_not_vote_is_told_so_in_its_own_voice() {
        let mut story = Story::of(id("ann"), true);
        let everyone = ["wolf", "ann"];
        recall_all(
            &mut story,
            [
                heard(1, Phase::Day, &everyone, &[]),
                heard(2, Phase::Night, &everyone, &[]),
            ],
        );
        assert_eq!(
            story.lines(),
            [
                "You are ann.",
                "",
                "Day 1.",
                "You do not vote.",
                "wolf does not vote.",
                "Nobody dies.",
                "",
                "Night 2.",
                "You sleep.",
            ]
        );
    }

    #[test]
    fn a_dead_player_is_not_in_a_later_days_tally() {
        let mut story = Story::of(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let living = ["seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        recall_all(
            &mut story,
            [
                heard(1, Phase::Day, &everyone, &known),
                heard(2, Phase::Night, &living, &known),
                chose("doctor", 3, "seer"),
                heard(2, Phase::Day, &living, &known),
                received(choice("seer", 4, "ann")),
                heard(3, Phase::Night, &living, &known),
            ],
        );
        assert_eq!(
            story.lines(),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "ann does not vote.",
                "You do not vote.",
                "seer does not vote.",
                "wolf does not vote.",
                "wolf dies.",
                "",
                "Night 2.",
                "You protect seer.",
                "Nobody dies.",
                "",
                "Day 2.",
                "ann does not vote.",
                "You do not vote.",
                "seer votes against ann.",
                "Nobody dies.",
                "",
                "Night 3.",
            ]
        );
    }

    /// What ann says on the first day.
    const ACCUSED: &str =
        "I suspect wolf, who was very quiet last night and has said nothing since.";

    /// What wolf says on the first day.
    const DENIED: &str = "Not me. I was asleep all night, as every villager here was, I promise.";

    /// What ann says on the second day.
    const STILL: &str = "Wolf is gone and somebody still died, so I was wrong about the werewolf.";

    /// The doctor's story of two days of talk with a night between them.
    fn two_days_of_talk() -> Story {
        let mut story = Story::of(id("doctor"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let living = ["seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        recall_all(
            &mut story,
            [
                heard(1, Phase::Day, &everyone, &known),
                received(utterance(2, "ann", 1, ACCUSED)),
                received(choice("ann", 2, "wolf")),
                received(utterance(2, "wolf", 3, DENIED)),
                heard(2, Phase::Night, &living, &known),
                heard(2, Phase::Day, &["doctor", "ann"], &known),
                received(utterance(4, "ann", 5, STILL)),
            ],
        );
        story
    }

    #[test]
    fn a_story_within_its_budget_is_told_whole() {
        let story = two_days_of_talk();
        let whole = story.lines();
        assert_eq!(
            whole,
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                &format!("ann says, \"{ACCUSED}\""),
                &format!("wolf says, \"{DENIED}\""),
                "ann votes against wolf.",
                "You do not vote.",
                "seer does not vote.",
                "wolf does not vote.",
                "wolf dies.",
                "",
                "Night 2.",
                "seer dies.",
                "",
                "Day 2.",
                &format!("ann says, \"{STILL}\""),
            ]
        );
        assert_eq!(story.told_within(estimated(&whole)), (whole.clone(), 0));
        assert_eq!(story.told_within(u64::MAX), (whole, 0));
    }

    #[test]
    fn a_story_over_its_budget_forgets_talk_oldest_first_in_its_place() {
        let story = two_days_of_talk();
        let whole = story.lines();
        let forgetting = |forgot: usize| -> Vec<&str> {
            let talk = [3, 4, 15];
            whole
                .iter()
                .enumerate()
                .filter(|(i, _)| !talk[..forgot].contains(i) || *i == talk[forgot - 1])
                .map(|(i, line)| {
                    if talk[..forgot].contains(&i) {
                        FORGOTTEN
                    } else {
                        line
                    }
                })
                .collect()
        };
        assert_eq!(
            forgetting(2),
            [
                "You are doctor, the doctor.",
                "",
                "Day 1.",
                "You don't remember what was said before this.",
                "ann votes against wolf.",
                "You do not vote.",
                "seer does not vote.",
                "wolf does not vote.",
                "wolf dies.",
                "",
                "Night 2.",
                "seer dies.",
                "",
                "Day 2.",
                &format!("ann says, \"{STILL}\""),
            ]
        );
        // The past day's talk is forgotten first, and only as much as has
        // to be.
        for forgot in 1..=2 {
            let budget = estimated(&forgetting(forgot));
            assert!(budget < estimated(&whole));
            assert_eq!(story.told_within(budget), (forgetting(forgot), forgot));
        }
        // A token less, and the current day's talk goes too.
        let budget = estimated(&forgetting(2)) - 1;
        assert_eq!(story.told_within(budget), (forgetting(3), 3));
    }

    #[test]
    fn a_story_over_its_budget_with_no_talk_left_is_told_with_none() {
        let story = two_days_of_talk();
        let none = vec![
            "You are doctor, the doctor.",
            "",
            "Day 1.",
            "ann votes against wolf.",
            "You do not vote.",
            "seer does not vote.",
            "wolf does not vote.",
            "wolf dies.",
            "",
            "Night 2.",
            "seer dies.",
            "",
            "Day 2.",
            FORGOTTEN,
        ];
        assert_eq!(story.told_within(0), (none, 3));
        // A story with no talk is told whole however far over it is.
        let mut quiet = Story::of(id("ann"), true);
        quiet.recall(&heard(1, Phase::Day, &["wolf", "ann"], &[]));
        assert_eq!(quiet.told_within(0), (quiet.lines(), 0));
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
            (id("doc3"), Role::Doctor),
            (id("ann"), Role::Villager),
        ])));
        let everyone = ["wolf", "doc1", "doc2", "doc3", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        narrator.narrate(&hunted("ann", &["doc2", "doc3", "doc1"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone)),
            [
                "The werewolves choose ann (villager), \
                 whom doc1 (doctor), doc2 (doctor) and doc3 (doctor) protect.",
                "Nobody dies.",
                "Night 1 took 0s.",
                "",
                "Day 1."
            ]
        );
    }

    #[test]
    fn a_player_is_not_told_whom_the_werewolves_chose() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
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
    fn players_come_in_seat_order_with_player_2_before_player_10() {
        let mut narrator = Narrator::default();
        let roles = HashMap::from([
            (id("Player 10"), Role::Villager),
            (id("Player 2"), Role::Werewolf),
            (id("Player 1"), Role::Seer),
        ]);
        assert_eq!(
            narrator.narrate(&dealt(roles)),
            [
                "3 players:",
                "Player 1, a seer, plays random",
                "Player 2, a werewolf, plays bold",
                "Player 10, a villager, plays random",
                "A night waits 1m and a day 2m."
            ]
        );
        let everyone = ["Player 1", "Player 2", "Player 10"];
        narrator.narrate(&shown("Player 1", 1, Phase::Day, &everyone));
        selected(&mut narrator, "Player 10", 2, "Player 2");
        selected(&mut narrator, "Player 2", 2, "Player 10");
        selected(&mut narrator, "Player 1", 2, "Player 2");
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 1, &["Player 2"])),
            [
                "Player 1 (seer) votes against Player 2 (werewolf).",
                "Player 2 (werewolf) votes against Player 10 (villager).",
                "Player 10 (villager) votes against Player 2 (werewolf).",
                "Player 1 and Player 10 die.",
                "Day 1 took 0s.",
                "",
                "The werewolves win after 1 day.",
                "The game took 0s.",
                "Survivors:",
                "Player 2 (werewolf)"
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
    fn a_selection_the_environment_observed_in_a_later_phase_goes_untold() {
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
        assert!(selected_in(&mut narrator, "seer", 1, "wolf", 2).is_empty());
        // A selection in time is held for the end of its phase.
        assert!(selected(&mut narrator, "seer", 2, "wolf").is_empty());
    }

    #[test]
    fn an_observation_of_a_message_whose_action_is_not_a_selection_adds_nothing() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone));
        // The wolf's observation of the announcement.
        assert!(
            narrator
                .narrate(&observed_by("wolf", ENVIRONMENT, 1, 1))
                .is_empty()
        );
        // An observation of a selection whose action never reached the log.
        assert!(
            narrator
                .narrate(&observed_by_the_environment("wolf", 7, 1))
                .is_empty()
        );
    }

    #[test]
    fn a_players_observation_of_a_public_vote_adds_nothing_and_the_environments_tells_it() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        assert!(narrator.narrate(&acted("wolf", 2, "ann")).is_empty());
        // By day every player receives the vote; whether it counts is the
        // environment's to say.
        assert!(
            narrator
                .narrate(&observed_by("seer", "wolf", 2, 2))
                .is_empty()
        );
        assert_eq!(
            narrator.narrate(&observed_by_the_environment("wolf", 2, 2)),
            ["wolf (werewolf) votes against ann (villager)."]
        );
    }

    #[test]
    fn a_memory_that_is_neither_an_announcement_heard_nor_a_selection_made_adds_nothing() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
        let reminder = received(Message::EndPhase { seq: 1 });
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
    fn a_failure_by_day_is_told_as_a_failure_to_play() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&announced("wolf", 2, 1, Phase::Day, &everyone));
        assert_eq!(
            narrator.narrate(&failed("wolf", 2)),
            ["wolf (werewolf) fails to play: POST \
              http://localhost:1234/v1/chat/completions was answered 400 Bad Request"]
        );
        narrator.narrate(&announced("wolf", 3, 2, Phase::Night, &everyone));
        assert_eq!(
            narrator.narrate(&failed("wolf", 2)),
            ["wolf (werewolf) fails to play for day 1: POST \
              http://localhost:1234/v1/chat/completions was answered 400 Bad Request"]
        );
    }

    #[test]
    fn a_player_is_not_told_of_its_own_failure_to_select() {
        let mut narrator = Narrator::for_player(id("wolf"), true);
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
            forgot: None,
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
            narrator.narrate(&ended(Team::Werewolves, 1, &["wolf"])),
            [
                "ann, doctor and seer die.",
                "Day 1 took 0s.",
                "",
                "The werewolves win after 1 day.",
                "The game took 0s.",
                "Survivors:",
                "wolf (werewolf)"
            ]
        );
    }

    #[test]
    fn a_player_is_shown_no_table() {
        let mut narrator = Narrator::for_player(id("wolf"), true);
        assert!(narrator.narrate(&start()).is_empty());
    }

    #[test]
    fn a_werewolf_opens_with_its_pack_and_kills_in_the_second_person() {
        let mut narrator = Narrator::for_player(id("wolf"), true);
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
    fn a_pack_of_three_is_listed_in_prose() {
        let mut narrator = Narrator::for_player(id("wolf"), true);
        let pack = [
            ("wolf", Role::Werewolf),
            ("bob", Role::Werewolf),
            ("cat", Role::Werewolf),
        ];
        assert_eq!(
            narrator.recall(&heard(
                1,
                Phase::Night,
                &["wolf", "bob", "cat", "ann"],
                &pack
            ))[0],
            "You are wolf, a werewolf. The werewolves are bob, cat and wolf."
        );
    }

    #[test]
    fn a_werewolf_without_a_pack_is_told_it_is_alone() {
        let mut narrator = Narrator::for_player(id("wolf"), true);
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
    fn a_player_shown_no_role_is_named_alone_and_is_awake_by_day() {
        let mut narrator = Narrator::for_player(id("ann"), true);
        assert_eq!(
            narrator.recall(&heard(1, Phase::Day, &["wolf", "ann"], &[])),
            ["You are ann.", "", "Day 1."]
        );
    }

    #[test]
    fn the_seer_asks_by_night_and_learns_by_morning() {
        let mut narrator = Narrator::for_player(id("seer"), true);
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
            [
                "doctor does not vote.",
                "You vote against wolf (werewolf).",
                "wolf (werewolf) does not vote.",
                "Nobody dies.",
                "",
                "Night 2."
            ]
        );
    }

    #[test]
    fn the_doctor_protects_by_night_and_votes_by_day() {
        let mut narrator = Narrator::for_player(id("doctor"), true);
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
    fn a_villager_is_told_it_sleeps_through_each_night_and_who_died_as_the_night_begins() {
        let mut narrator = Narrator::for_player(id("ann"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("ann", Role::Villager)];
        assert_eq!(
            narrator.recall(&heard(1, Phase::Night, &everyone, &known)),
            ["You are ann, a villager.", "", "Night 1.", "You sleep."]
        );
        assert_eq!(
            narrator.recall(&heard(1, Phase::Day, &everyone, &known)),
            ["Nobody dies.", "", "Day 1."]
        );
        assert_eq!(
            narrator.recall(&chose("ann", 2, "wolf")),
            ["You vote against wolf."]
        );
        assert_eq!(
            narrator.recall(&heard(2, Phase::Night, &["seer", "doctor", "ann"], &known)),
            [
                "You vote against wolf.",
                "doctor does not vote.",
                "seer does not vote.",
                "wolf does not vote.",
                "wolf dies.",
                "",
                "Night 2.",
                "You sleep."
            ]
        );
    }

    #[test]
    fn a_player_announced_phases_it_was_not_shown_between_is_told_who_died_in_the_past_tense() {
        let mut narrator = Narrator::for_player(id("ann"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("ann", Role::Villager)];
        narrator.recall(&heard(1, Phase::Day, &everyone, &known));
        assert_eq!(
            narrator.recall(&heard(2, Phase::Day, &["seer", "doctor", "ann"], &known)),
            [
                "You do not vote.",
                "doctor does not vote.",
                "seer does not vote.",
                "wolf does not vote.",
                "wolf died.",
                "",
                "Day 2."
            ]
        );
        assert_eq!(
            narrator.recall(&heard(3, Phase::Day, &["seer", "doctor", "ann"], &known)),
            [
                "You do not vote.",
                "doctor does not vote.",
                "seer does not vote.",
                "Nobody died.",
                "",
                "Day 3."
            ]
        );
    }

    #[test]
    fn a_player_awake_by_night_is_not_told_it_sleeps_and_the_omniscient_account_tells_no_sleep() {
        let mut narrator = Narrator::for_player(id("seer"), true);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.recall(&heard(1, Phase::Night, &everyone, &[("seer", Role::Seer)])),
            ["You are seer, the seer.", "", "Night 1."]
        );
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                1,
                Phase::Night,
                &everyone,
                &[("ann", Role::Villager)]
            )),
            ["", "Night 1."]
        );
    }

    #[test]
    fn the_logs_actions_and_observations_add_nothing_to_a_players_account() {
        // A player's account is told from its memory, whoever the log's
        // entries are about.
        let mut narrator = Narrator::for_player(id("wolf"), true);
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
        assert!(
            narrator
                .narrate(&spoke("ann", 2, 1, "Hello.", &["wolf"]))
                .is_empty()
        );
        assert!(
            narrator
                .narrate(&withheld("wolf", utterance(2, "wolf", 0, "Wait!")))
                .is_empty()
        );
    }

    #[test]
    fn a_player_names_the_survivors_it_knows_the_roles_of() {
        let mut narrator = Narrator::for_player(id("seer"), true);
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
        let mut narrator = Narrator::for_player(id("wolf"), true);
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

    /// The configuration of a game whose requests to each of `models`, by
    /// name, are made under its tool choice, within the default context
    /// window, as the log records it.
    fn configured_with(models: &[(&str, ToolChoice)]) -> Event<Entry> {
        let models = in_play(models);
        logged(Entry::Configuration(Box::new(Configuration {
            version: env!("CARGO_PKG_VERSION").to_string(),
            role_counts: BTreeMap::from([(Role::Werewolf, 1), (Role::Villager, 2)]),
            limits: Limits {
                night_limit: Duration::from_secs(30),
                day_limit: Duration::from_secs(60),
                ratify_limit: Duration::from_secs(20),
            },
            strategies: BTreeMap::from([(RANDOM.to_string(), Strategy::Random(Random {}))]),
            allotments: BTreeMap::new(),
            models,
            prompts: BTreeMap::from([(id("wolf"), "You are wolf.".to_string())]),
            text: BTreeMap::new(),
        })))
    }

    #[test]
    fn a_configuration_requiring_a_tool_call_tells_only_the_context_window_and_only_in_the_omniscient_account()
     {
        let configuration = configured_with(&[("qwen", ToolChoice::Required)]);
        assert_eq!(
            Narrator::default().narrate(&configuration),
            ["qwen's context window is 4096 tokens, the default."]
        );
        assert!(
            Narrator::for_player(id("wolf"), true)
                .narrate(&configuration)
                .is_empty()
        );
    }

    #[test]
    fn a_configuration_allowing_a_tool_call_is_told_in_the_omniscient_account_alone() {
        let configuration = configured_with(&[("qwen", ToolChoice::Auto)]);
        assert_eq!(
            Narrator::default().narrate(&configuration),
            [
                "qwen's context window is 4096 tokens, the default.",
                "qwen does not accept a required tool call, \
                 so its players are asked to call one."
            ]
        );
        assert!(
            Narrator::for_player(id("wolf"), true)
                .narrate(&configuration)
                .is_empty()
        );
    }

    /// A vote, as the narrator tells it.
    fn vote(line: &str) -> Line {
        Line::Vote(line.to_string())
    }

    /// Something told that is neither a vote nor a heading.
    fn told_line(line: &str) -> Line {
        Line::Told(line.to_string())
    }

    #[test]
    fn an_utterance_is_said_by_its_speaker_named_with_its_role() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        assert_eq!(
            narrator.narrate(&spoke("wolf", 2, 1, "Trust me.", &["ann"])),
            [Line::Said {
                who: id("wolf"),
                speaker: "wolf (werewolf)".to_string(),
                text: "Trust me.".to_string(),
            }]
        );
    }

    #[test]
    fn a_phase_opens_with_a_blank_line_and_its_heading() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone)),
            [Line::Blank, Line::Heading("Night 1.".to_string())]
        );
    }

    #[test]
    fn the_nights_choices_are_votes_and_what_came_of_them_is_told() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        selected(&mut narrator, "wolf", 1, "ann");
        selected(&mut narrator, "doctor", 1, "seer");
        selected(&mut narrator, "seer", 1, "ann");
        narrator.narrate(&hunted("ann", &[]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]))[..5],
            [
                vote("doctor (doctor) protects seer (seer)."),
                vote("seer (seer) learns ann is a villager."),
                vote("wolf (werewolf) votes to kill ann (villager)."),
                told_line("The werewolves choose ann (villager)."),
                told_line("ann dies."),
            ]
        );
    }

    #[test]
    fn a_day_vote_and_its_retraction_are_votes() {
        let mut narrator = Narrator::default();
        narrator.narrate(&ratified());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone));
        assert_eq!(
            selected(&mut narrator, "wolf", 2, "ann"),
            [vote("wolf (werewolf) votes against ann (villager).")]
        );
        assert_eq!(
            retracted_in(&mut narrator, "wolf", 2, 3, 2),
            [vote(
                "wolf (werewolf) withdraws their vote and is undecided again."
            )]
        );
    }

    #[test]
    fn a_players_own_deeds_and_its_tally_are_votes() {
        let mut narrator = Narrator::for_player(id("seer"), false);
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("seer", Role::Seer)];
        narrator.remember(&heard(1, Phase::Night, &everyone, &known));
        assert_eq!(
            narrator.remember(&chose("seer", 1, "wolf")),
            [vote("You ask about wolf.")]
        );
        narrator.remember(&heard(1, Phase::Day, &everyone, &known));
        narrator.remember(&chose("seer", 2, "wolf"));
        assert_eq!(
            narrator.remember(&heard(2, Phase::Night, &everyone, &known))[..4],
            [
                vote("ann does not vote."),
                vote("doctor does not vote."),
                vote("You vote against wolf."),
                vote("wolf does not vote."),
            ]
        );
    }
}
