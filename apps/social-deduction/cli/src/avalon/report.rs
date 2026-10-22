//! A legible account of a game: of everything, told from the log as it is
//! written, or of what one player saw, told from the player's memory.
//!
//! The log is written in actions and observations, by every actor. The
//! omniscient account reads the environment's: each phase is told from its
//! announcements, and how it came out from the entry that ends it. Talk is
//! told from its speaker's action. A player's memory holds the messages
//! themselves, and its [`Story`] reads them.

use super::prompted::Memory;
use super::{
    Avalon, Ballot, Choice, Deed, Entry, Message, Outcome, PROPOSALS, Phase, QuestResult, Role,
    Rules, Seen, Tally, Victory, View,
};
use crate::game::Narrate;
use crate::prompted::Account;
use crate::report::{Line, Seat, configured, in_order, in_prose, late, told_within, whole_seconds};
use crate::seat::PlayerId;
use free_agent::Event;
use humantime::format_duration;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

/// Turns the events of a game, one at a time, into lines of prose. It
/// reads the log's events through [`Narrator::narrate`] and tells the
/// table, a player to a line with its role and strategy, and how long each
/// phase waits, then each model in play, its context window and whether
/// its players are made to call a tool, then each phase as it opens and how it came out, and how
/// long it took, then who won, how, and how long the whole game took. It
/// names a player with its role in parentheses, and lists players in seat
/// order. A blank line sets each phase off from what came before it, and
/// another the winner from the last. Talk is told as it comes, while its
/// phase lasts; a message that comes after its phase has ended, and a
/// withheld choice, stay in the log and out of the account. A
/// player's failure to act is told as it happens, with what went wrong.
#[derive(Debug, Default)]
pub struct Narrator {
    /// Every role, from the deal.
    roles: HashMap<PlayerId, Role>,
    /// Every player's strategy, from the deal.
    strategies: HashMap<PlayerId, String>,
    /// The phase being told, once one has begun.
    phase: Option<Told>,
    /// Every phase announced so far, by its number, as one player was
    /// shown it.
    announced: BTreeMap<u64, View>,
}

/// The phase being told: which it is, and when it began.
#[derive(Debug)]
struct Told {
    /// Which phase.
    phase: Phase,
    /// When the phase was first announced.
    began: SystemTime,
}

impl Narrate<Avalon> for Narrator {
    /// The lines the log's `event` adds to the account, often none.
    fn narrate(&mut self, event: &Event<Entry>) -> Vec<Line> {
        let time = event.time;
        match &event.payload {
            Entry::Start {
                roles,
                strategies,
                limits,
            } => {
                self.roles = roles.clone();
                self.strategies = strategies.clone();
                self.table(limits)
            }
            Entry::Configuration(configuration) => configured(&configuration.models),
            Entry::Action {
                message: Message::Announce { seq, view, .. },
                ..
            } => self.announce(*seq, view, time),
            Entry::Action {
                message: Message::Say {
                    seq, from, text, ..
                },
                ..
            } => (!late(&self.announced, *seq))
                .then(|| self.said(from, text))
                .into_iter()
                .collect(),
            Entry::Failed { player, seq, error } => {
                let player = self.seat(player);
                vec![Line::Told(
                    match late(&self.announced, *seq)
                        .then(|| self.phase_name(*seq))
                        .flatten()
                    {
                        Some(phase) => format!("{player} fails to play for {phase}: {error}"),
                        None => format!("{player} fails to play: {error}"),
                    },
                )]
            }
            Entry::Event(Outcome::Proposed { seq, team }) => {
                let leader = match self.announced.get(seq) {
                    Some(view) => self.seat(&view.leader).to_string(),
                    None => "The leader".to_string(),
                };
                vec![Line::Told(match team {
                    Some(team) => format!("{leader} proposes {}.", self.listed(team)),
                    None => format!("{leader} proposes nobody in time."),
                })]
            }
            Entry::Event(Outcome::Tallied { tally, .. }) => self.tallied(tally),
            Entry::Event(Outcome::Quested { result, .. }) => vec![Line::Told(quested(result))],
            Entry::Event(Outcome::Assassinated { target, merlin, .. }) => {
                vec![Line::Told(match target {
                    Some(target) => format!(
                        "The Assassin names {}, who is {}Merlin.",
                        self.seat(target),
                        if *merlin { "" } else { "not " }
                    ),
                    None => "The Assassin names nobody in time.".to_string(),
                })]
            }
            Entry::Event(Outcome::End { by, took, .. }) => self.end(*by, *took, time),
            _ => vec![],
        }
    }
}

impl Narrator {
    /// The deal: how many players, then each with its role and its
    /// strategy on a line of its own, then how long each phase waits.
    fn table(&self, rules: &Rules) -> Vec<Line> {
        let Rules {
            propose_limit: propose,
            vote_limit: vote,
            quest_limit: quest,
            assassinate_limit: assassinate,
        } = *rules;
        let heading = format!("{} players:", self.roles.len());
        let seats = in_order(self.roles.keys()).into_iter().map(|player| {
            format!(
                "{player}, {}, plays {}",
                self.roles[player], self.strategies[player]
            )
        });
        let limits = format!(
            "A proposal waits {}, a vote {}, a quest {} and the assassination {}.",
            format_duration(propose),
            format_duration(vote),
            format_duration(quest),
            format_duration(assassinate)
        );
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

    /// `players`, as this account names them, as a list in prose, in the
    /// order given.
    fn listed(&self, players: &[PlayerId]) -> String {
        in_prose(players.iter().map(|player| self.seat(player)))
    }

    /// What the announcement at `time` of the phase numbered `seq`, showing
    /// `view`, adds to the account: nothing if the phase has begun, and
    /// otherwise the end of the phase before it, then a blank line and what
    /// the new phase is.
    fn announce(&mut self, seq: u64, view: &View, time: SystemTime) -> Vec<Line> {
        if self.announced.contains_key(&seq) {
            return vec![];
        }
        self.announced.insert(seq, view.clone());
        let mut lines = self.finish(time);
        lines.push(Line::Blank);
        lines.push(match view.phase {
            Phase::Propose => Line::Heading(format!(
                "Quest {}, proposal {} of {PROPOSALS}. {} leads and needs a team of {}.",
                view.quest,
                view.proposal,
                self.seat(&view.leader),
                view.size
            )),
            Phase::Vote => Line::Told("Everyone votes on the team.".to_string()),
            Phase::Quest => {
                let team: Vec<PlayerId> = view.team.iter().flatten().cloned().collect();
                Line::Told(questing(&self.listed(&team), view))
            }
            Phase::Assassinate => Line::Told("The Assassin tries to name Merlin.".to_string()),
        });
        self.phase = Some(Told {
            phase: view.phase,
            began: time,
        });
        lines
    }

    /// How long the phase being told took, if one was, as it ends at
    /// `time`.
    fn finish(&mut self, time: SystemTime) -> Vec<Line> {
        self.phase
            .take()
            .map(|told| {
                let took = time.duration_since(told.began).unwrap_or_default();
                Line::Told(format!(
                    "The {} took {}.",
                    told.phase.told(),
                    whole_seconds(took)
                ))
            })
            .into_iter()
            .collect()
    }

    /// How a vote came out: who approved, who rejected, and who did not
    /// vote, each in seat order and left out when nobody did, then whether
    /// the team goes.
    fn tallied(&self, tally: &Tally) -> Vec<Line> {
        tallied(tally, &in_order(self.roles.keys()), |who| self.listed(who))
    }

    /// The end of the game at `time`: how long the last phase took, then a
    /// blank line, who won and how, and how long the game took.
    fn end(&mut self, by: Victory, took: Duration, time: SystemTime) -> Vec<Line> {
        let mut lines = self.finish(time);
        lines.push(Line::Blank);
        lines.push(Line::Told(won(by).to_string()));
        lines.push(Line::Told(format!(
            "The game took {}.",
            whole_seconds(took)
        )));
        lines
    }

    /// `player` says `text`, named with its role.
    fn said(&self, player: &PlayerId, text: &str) -> Line {
        Line::Said {
            who: player.clone(),
            speaker: self.seat(player).to_string(),
            text: text.to_string(),
        }
    }

    /// The phase numbered `seq` as prose names it mid-sentence, "quest 2",
    /// if it has been announced.
    fn phase_name(&self, seq: u64) -> Option<String> {
        self.announced.get(&seq).map(|view| match view.phase {
            Phase::Propose => format!("proposal {} of quest {}", view.proposal, view.quest),
            Phase::Vote => format!(
                "the vote on proposal {} of quest {}",
                view.proposal, view.quest
            ),
            Phase::Quest => format!("quest {}", view.quest),
            Phase::Assassinate => "the assassination".to_string(),
        })
    }
}

/// How a game was won `by`, as the account tells it: "Evil wins by naming
/// Merlin."
pub(crate) fn won(by: Victory) -> &'static str {
    match by {
        Victory::Quests => "Good wins with three successful quests.",
        Victory::Sabotage => "Evil wins with three failed quests.",
        Victory::Rejections => "Evil wins with five failed proposals in a row.",
        Victory::Assassination => "Evil wins by naming Merlin.",
    }
}

/// How a vote came out, told of `players` in seat order, each list of them
/// as `listed` names them: who approved, who rejected, and who did not
/// vote, each left out when nobody did, then whether the team goes.
fn tallied(
    tally: &Tally,
    players: &[&PlayerId],
    listed: impl Fn(&[PlayerId]) -> String,
) -> Vec<Line> {
    let voted = |ballot| -> Vec<PlayerId> {
        players
            .iter()
            .filter(|player| tally.ballots.get(**player) == ballot)
            .map(|player| (*player).clone())
            .collect()
    };
    let mut lines: Vec<Line> = [
        (voted(Some(&Ballot::Approve)), "approves", "approve"),
        (voted(Some(&Ballot::Reject)), "rejects", "reject"),
        (voted(None), "does not vote", "do not vote"),
    ]
    .into_iter()
    .filter(|(who, _, _)| !who.is_empty())
    .map(|(who, one, several)| {
        let verb = if who.len() == 1 { one } else { several };
        Line::Vote(format!("{} {verb}.", listed(&who)))
    })
    .collect();
    lines.push(Line::Told(
        match tally.approved {
            true => "The team goes on the quest.",
            false => "The team does not go.",
        }
        .to_string(),
    ));
    lines
}

/// The team `listed` going on the quest `view` shows, and how many
/// sabotages fail it.
fn questing(listed: &str, view: &View) -> String {
    format!(
        "{listed} go on quest {}, which {}.",
        view.quest,
        sabotages_fail(view.sabotages_needed)
    )
}

/// That `needed` sabotages fail a quest: "1 sabotage fails", "2 sabotages
/// fail".
pub(super) fn sabotages_fail(needed: usize) -> String {
    match needed {
        1 => "1 sabotage fails".to_string(),
        needed => format!("{needed} sabotages fail"),
    }
}

/// How a quest came out: whether it succeeded, and how many sabotaged it.
fn quested(result: &QuestResult) -> String {
    let sabotages = match result.sabotages {
        0 => "no sabotage".to_string(),
        1 => "1 sabotage".to_string(),
        count => format!("{count} sabotages"),
    };
    let comes = if result.succeeded {
        "succeeds"
    } else {
        "fails"
    };
    format!("The quest {comes}, with {sabotages}.")
}

/// What a line of a player's story is, which decides whether a story told
/// within a budget may forget it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Something a player said, which a story may forget.
    Talk,
    /// Anything else, which stays.
    Kept,
}

/// A player's account of its own game, told a memory at a time, in the
/// second person, with no table, no limits and no times. It opens with who
/// the player is, which side it is on, and what it sees of the others.
/// Each phase is told as it is announced, set off by a blank line, and how
/// the phase before it came out as the next one begins, from what the new
/// announcement shows: the team proposed, or that the leader proposed
/// nobody in time; the tally of the vote; or how the quest came out. The
/// player's first choice for each phase, the one the environment counts, is
/// told as it makes it, and all talk, its own and others', as it is said or
/// heard. Told within a budget, it forgets talk, oldest first, until it
/// fits.
pub struct Story {
    /// Whose story this is.
    me: PlayerId,
    /// The latest phase the player was shown, once one has been.
    shown: Option<View>,
    /// The phase of the latest choice told, by the sequence number of its
    /// announcement, once one has been.
    chosen: Option<u64>,
    /// The lines told so far, in order, each with what it is.
    account: Vec<(String, Kind)>,
}

impl Story {
    /// The story `me` tells of its own game, with nothing in it yet.
    pub fn of(me: PlayerId) -> Self {
        Self {
            me,
            shown: None,
            chosen: None,
            account: Vec::new(),
        }
    }

    /// The lines told, in order.
    pub fn lines(&self) -> Vec<&str> {
        self.account.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// What being shown `view` adds: who the player is, the first time;
    /// after that, how the phase it was shown before came out; then a blank
    /// line and what the new phase is.
    fn shown(&mut self, view: &View) -> Vec<String> {
        let mut lines = match &self.shown {
            None => vec![self.identity(view)],
            Some(before) => self.outcome(before, view),
        };
        lines.push(String::new());
        lines.push(match view.phase {
            Phase::Propose => {
                let (leader, s) = self.subject(&view.leader);
                format!(
                    "Quest {}, proposal {} of {PROPOSALS}. {leader} lead{s} and need{s} a team of {}.",
                    view.quest, view.proposal, view.size
                )
            }
            Phase::Vote => format!(
                "Everyone votes on the team of {}.",
                in_prose(view.team.iter().flatten())
            ),
            Phase::Quest => questing(&in_prose(view.team.iter().flatten()), view),
            Phase::Assassinate => "The Assassin tries to name Merlin.".to_string(),
        });
        self.shown = Some(view.clone());
        lines
    }

    /// Who the player is, as its first `view` shows it: its name, its
    /// role and its side, and whom it sees as evil, or as Merlin or
    /// Morgana.
    fn identity(&self, view: &View) -> String {
        let seen = |as_seen| {
            in_order(view.sees.keys())
                .into_iter()
                .filter(|player| view.sees[*player] == as_seen)
                .cloned()
                .collect::<Vec<PlayerId>>()
        };
        let mut said = format!(
            "You are {}, {}, on the {} side.",
            self.me,
            view.role.told(),
            view.role.team()
        );
        let evil = seen(Seen::Evil);
        if !evil.is_empty() {
            let are = if evil.len() == 1 { "is" } else { "are" };
            said.push_str(&format!(" You see that {} {are} evil.", in_prose(evil)));
        }
        match seen(Seen::MerlinOrMorgana).as_slice() {
            [] => {}
            [one, other] => said.push_str(&format!(
                " One of {one} and {other} is Merlin and the other Morgana."
            )),
            either => said.push_str(&format!(
                " You see that {} is Merlin or Morgana.",
                in_prose(either)
            )),
        }
        said
    }

    /// How the phase `before` showed came out, as `after`, the next, shows
    /// it: a proposal that ended with no team, by the leader proposing
    /// nobody in time; a vote, by its tally; and a quest, by its result. A
    /// proposal with a team is told by the vote on it.
    fn outcome(&self, before: &View, after: &View) -> Vec<String> {
        match before.phase {
            Phase::Propose if after.phase == Phase::Propose => {
                let (leader, s) = self.subject(&before.leader);
                vec![format!("{leader} propose{s} nobody in time.")]
            }
            Phase::Vote => after.tally.as_ref().map_or(vec![], |tally| {
                let players: Vec<&PlayerId> = before.players.iter().collect();
                tallied(tally, &players, |who| in_prose(who))
                    .iter()
                    .map(Line::to_string)
                    .collect()
            }),
            Phase::Quest => after.record.last().map(quested).into_iter().collect(),
            _ => vec![],
        }
    }

    /// The player's own `choice`, told as made.
    fn chose(&self, choice: &Choice) -> String {
        match choice {
            Choice::Team(team) => format!("You propose {}.", in_prose(team)),
            Choice::Ballot(Ballot::Approve) => "You approve the team.".to_string(),
            Choice::Ballot(Ballot::Reject) => "You reject the team.".to_string(),
            Choice::Deed(Deed::Support) => "You support the quest.".to_string(),
            Choice::Deed(Deed::Sabotage) => "You sabotage the quest.".to_string(),
            Choice::Target(target) => format!("You name {target} as Merlin."),
        }
    }

    /// `player` says `text`.
    fn said(&self, player: &PlayerId, text: &str) -> String {
        let (subject, s) = self.subject(player);
        format!("{subject} say{s}, \"{text}\"")
    }

    /// `player` as the subject of a sentence, with the ending its verb
    /// takes: "You" and none for the player itself, its name and an s for
    /// anyone else.
    fn subject(&self, player: &PlayerId) -> (String, &'static str) {
        if *player == self.me {
            ("You".to_string(), "")
        } else {
            (player.clone(), "s")
        }
    }
}

impl Account<Avalon> for Story {
    /// Add what `event` adds to the account: an announcement begins a
    /// phase, the player's first choice for a phase is told as made, unless
    /// a choice for a later phase was told first, and an utterance it made
    /// or heard as said. Nothing else adds anything.
    fn recall(&mut self, event: &Event<Memory>) {
        let (lines, kind) = match &event.payload {
            Memory::Observed {
                message: Message::Announce { view, .. },
            } => (self.shown(view), Kind::Kept),
            Memory::Acted {
                message: Message::Say { from, text, .. },
            }
            | Memory::Observed {
                message: Message::Say { from, text, .. },
            } => (vec![self.said(from, text)], Kind::Talk),
            Memory::Acted {
                message: Message::Move(made),
            } => {
                let (seq, _, choice) = made.choice();
                if self.chosen.is_some_and(|chosen| chosen >= seq) {
                    (vec![], Kind::Kept)
                } else {
                    self.chosen = Some(seq);
                    (vec![self.chose(&choice)], Kind::Kept)
                }
            }
            Memory::Acted { .. } => (vec![], Kind::Kept),
            Memory::Observed { .. } => (vec![], Kind::Kept),
        };
        self.account
            .extend(lines.into_iter().map(|text| (text, kind)));
    }

    fn told_within(&self, budget: u64) -> (Vec<&str>, usize) {
        told_within(&self.account, |kind| *kind == Kind::Talk, budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::tests::{
        announcement, dealt, five, logged, logged_at, seated, utterance, view_of,
    };
    use crate::avalon::{Rules, State};
    use crate::model::fake::acted_in_this_phase;
    use crate::report::FORGOTTEN;
    use crate::seat::ENVIRONMENT;
    use crate::seat::id;
    use crate::strategy::RANDOM;
    use free_agent_game::report::testing::estimated;
    use std::collections::HashSet;

    /// A narrator that has been told the deal of the default table.
    fn dealt_five() -> Narrator {
        let mut narrator = Narrator::default();
        narrator.narrate(&logged(Entry::start(
            &dealt(&five(), RANDOM),
            &Rules::default(),
        )));
        narrator
    }

    /// The environment's announcement to `to` of the phase numbered `seq`,
    /// showing `view`.
    fn announced(to: &str, seq: u64, view: View) -> Entry {
        Entry::Action {
            from: ENVIRONMENT.to_string(),
            n: seq,
            to: HashSet::from([id(to)]),
            message: Message::Announce {
                seq,
                n: seq,
                view,
                limit: Duration::from_secs(60),
            },
            seen: None,
            forgot: None,
        }
    }

    /// The announcement of the phase numbered `seq`, `phase` of the first
    /// quest with `team` on the table, to the first seat, `seconds` into
    /// the game.
    fn opens(seconds: u64, seq: u64, phase: Phase, team: &[&str]) -> Event<Entry> {
        logged_at(
            seconds,
            announced("Player 1", seq, view_of("Player 1", phase, team)),
        )
    }

    /// The players named.
    fn team(players: &[&str]) -> Vec<PlayerId> {
        players.iter().map(|player| id(player)).collect()
    }

    #[test]
    fn the_table_is_told_first_then_the_limits() {
        let mut narrator = Narrator::default();
        let rules = Rules {
            vote_limit: Duration::from_secs(90),
            ..Rules::default()
        };
        let told = narrator.narrate(&logged(Entry::start(&dealt(&five(), RANDOM), &rules)));
        assert_eq!(
            told,
            [
                "5 players:",
                "Player 1, Merlin, plays random",
                "Player 2, Assassin, plays random",
                "Player 3, Servant, plays random",
                "Player 4, Servant, plays random",
                "Player 5, Minion, plays random",
                "A proposal waits 1m, a vote 1m 30s, a quest 1m and the assassination 1m.",
            ]
        );
    }

    #[test]
    fn each_phase_is_told_as_it_opens_once_and_how_long_the_one_before_took() {
        let mut narrator = dealt_five();
        assert_eq!(
            narrator.narrate(&opens(0, 1, Phase::Propose, &[])),
            [
                "",
                "Quest 1, proposal 1 of 5. Player 1 (Merlin) leads and needs a team of 2."
            ]
        );
        // The same phase announced to another player adds nothing.
        let again = announced("Player 2", 1, view_of("Player 2", Phase::Propose, &[]));
        assert!(narrator.narrate(&logged_at(0, again)).is_empty());
        assert_eq!(
            narrator.narrate(&opens(3, 2, Phase::Vote, &["Player 1", "Player 3"])),
            ["The proposal took 3s.", "", "Everyone votes on the team."]
        );
        assert_eq!(
            narrator.narrate(&opens(5, 3, Phase::Quest, &["Player 1", "Player 3"])),
            [
                "The vote took 2s.",
                "",
                "Player 1 (Merlin) and Player 3 (Servant) go on quest 1, which 1 sabotage fails."
            ]
        );
        assert_eq!(
            narrator.narrate(&opens(6, 4, Phase::Assassinate, &[])),
            [
                "The quest took 1s.",
                "",
                "The Assassin tries to name Merlin."
            ]
        );
        assert_eq!(
            narrator.narrate(&logged_at(
                70,
                Entry::Event(Outcome::End {
                    winner: crate::avalon::Team::Good,
                    by: Victory::Quests,
                    took: Duration::from_millis(70_400),
                })
            )),
            [
                "The assassination took 1m 4s.",
                "",
                "Good wins with three successful quests.",
                "The game took 1m 10s."
            ]
        );
    }

    #[test]
    fn a_quest_that_two_sabotages_fail_says_so() {
        let mut narrator = dealt_five();
        let mut view = view_of(
            "Player 1",
            Phase::Quest,
            &["Player 1", "Player 2", "Player 3"],
        );
        view.sabotages_needed = 2;
        assert_eq!(
            narrator.narrate(&logged(announced("Player 1", 1, view))),
            [
                "",
                "Player 1 (Merlin), Player 2 (Assassin) and Player 3 (Servant) go on quest 1, which 2 sabotages fail."
            ]
        );
    }

    #[test]
    fn a_proposal_names_its_team_or_nobody_in_time() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let proposed = Entry::Event(Outcome::Proposed {
            seq: 1,
            team: Some(team(&["Player 1", "Player 4", "Player 5"])),
        });
        assert_eq!(
            narrator.narrate(&logged(proposed)),
            [
                "Player 1 (Merlin) proposes Player 1 (Merlin), Player 4 (Servant) and Player 5 (Minion)."
            ]
        );
        let lapsed = Entry::Event(Outcome::Proposed { seq: 1, team: None });
        assert_eq!(
            narrator.narrate(&logged(lapsed)),
            ["Player 1 (Merlin) proposes nobody in time."]
        );
        // A proposal never announced names no leader.
        let unknown = Entry::Event(Outcome::Proposed {
            seq: 9,
            team: Some(team(&["Player 2"])),
        });
        assert_eq!(
            narrator.narrate(&logged(unknown)),
            ["The leader proposes Player 2 (Assassin)."]
        );
    }

    #[test]
    fn a_tally_tells_who_approved_who_rejected_who_did_not_vote_and_whether_the_team_goes() {
        let narrator = dealt_five();
        let tally = Tally::of(
            HashMap::from([
                (id("Player 1"), Ballot::Approve),
                (id("Player 3"), Ballot::Approve),
                (id("Player 2"), Ballot::Reject),
            ]),
            5,
        );
        assert_eq!(
            narrator.tallied(&tally),
            [
                "Player 1 (Merlin) and Player 3 (Servant) approve.",
                "Player 2 (Assassin) rejects.",
                "Player 4 (Servant) and Player 5 (Minion) do not vote.",
                "The team does not go."
            ]
        );
        let ballots = ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"]
            .map(|player| {
                let ballot = match player {
                    "Player 2" | "Player 5" => Ballot::Reject,
                    _ => Ballot::Approve,
                };
                (id(player), ballot)
            })
            .into();
        let mut narrator = dealt_five();
        assert_eq!(
            narrator.narrate(&logged(Entry::Event(Outcome::Tallied {
                seq: 2,
                tally: Tally::of(ballots, 5),
            }))),
            [
                "Player 1 (Merlin), Player 3 (Servant) and Player 4 (Servant) approve.",
                "Player 2 (Assassin) and Player 5 (Minion) reject.",
                "The team goes on the quest."
            ]
        );
        let lone = Tally::of(HashMap::from([(id("Player 4"), Ballot::Reject)]), 5);
        assert_eq!(
            narrator.tallied(&lone)[1],
            "Player 1 (Merlin), Player 2 (Assassin), Player 3 (Servant) and Player 5 (Minion) do not vote."
        );
    }

    #[test]
    fn a_quest_tells_whether_it_succeeds_and_how_many_sabotaged_it() {
        let mut narrator = dealt_five();
        let quested = |sabotages, succeeded| {
            logged(Entry::Event(Outcome::Quested {
                seq: 3,
                result: QuestResult {
                    team: team(&["Player 1", "Player 2"]),
                    sabotages,
                    succeeded,
                },
            }))
        };
        assert_eq!(
            narrator.narrate(&quested(0, true)),
            ["The quest succeeds, with no sabotage."]
        );
        assert_eq!(
            narrator.narrate(&quested(1, true)),
            ["The quest succeeds, with 1 sabotage."]
        );
        assert_eq!(
            narrator.narrate(&quested(2, false)),
            ["The quest fails, with 2 sabotages."]
        );
    }

    #[test]
    fn the_assassination_tells_whom_the_assassin_named_and_whether_it_was_merlin() {
        let mut narrator = dealt_five();
        let named = |target: Option<&str>, merlin| {
            logged(Entry::Event(Outcome::Assassinated {
                seq: 10,
                target: target.map(id),
                merlin,
            }))
        };
        assert_eq!(
            narrator.narrate(&named(Some("Player 1"), true)),
            ["The Assassin names Player 1 (Merlin), who is Merlin."]
        );
        assert_eq!(
            narrator.narrate(&named(Some("Player 3"), false)),
            ["The Assassin names Player 3 (Servant), who is not Merlin."]
        );
        assert_eq!(
            narrator.narrate(&named(None, false)),
            ["The Assassin names nobody in time."]
        );
    }

    #[test]
    fn the_end_tells_who_won_and_how() {
        let ends = [
            (Victory::Sabotage, "Evil wins with three failed quests."),
            (
                Victory::Rejections,
                "Evil wins with five failed proposals in a row.",
            ),
            (Victory::Assassination, "Evil wins by naming Merlin."),
        ];
        for (by, told) in ends {
            let mut narrator = dealt_five();
            let end = Entry::Event(Outcome::End {
                winner: crate::avalon::Team::Evil,
                by,
                took: Duration::from_secs(40),
            });
            assert_eq!(
                narrator.narrate(&logged(end)),
                ["", told, "The game took 40s."]
            );
        }
    }

    #[test]
    fn talk_is_told_as_it_is_said_unless_its_phase_has_ended() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let spoke = |seq, text| {
            logged(Entry::Action {
                from: id("Player 3"),
                n: 1,
                to: HashSet::from([id("Player 1")]),
                message: utterance(seq, "Player 3", 1, text),
                seen: None,
                forgot: None,
            })
        };
        assert_eq!(
            narrator.narrate(&spoke(1, "Take me.")),
            ["Player 3 (Servant) says, \"Take me.\""]
        );
        narrator.narrate(&opens(1, 2, Phase::Vote, &["Player 1", "Player 3"]));
        assert!(narrator.narrate(&spoke(1, "Take me.")).is_empty());
    }

    #[test]
    fn a_choice_withheld_goes_untold() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        narrator.narrate(&opens(0, 2, Phase::Vote, &["Player 1", "Player 2"]));
        for message in [
            Choice::Ballot(Ballot::Approve).message(1, id("Player 2"), 0),
            Message::Wake,
        ] {
            let withheld = Entry::Withheld {
                from: id("Player 2"),
                message,
                seen: Some(4),
                forgot: None,
            };
            assert!(narrator.narrate(&logged(withheld)).is_empty());
        }
    }

    #[test]
    fn a_failure_is_told_as_it_happens_with_what_went_wrong() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let failed = |seq| {
            logged(Entry::Failed {
                player: id("Player 4"),
                seq,
                error: "connection refused".to_string(),
            })
        };
        assert_eq!(
            narrator.narrate(&failed(1)),
            ["Player 4 (Servant) fails to play: connection refused"]
        );
        narrator.narrate(&opens(0, 2, Phase::Vote, &["Player 1", "Player 2"]));
        assert_eq!(
            narrator.narrate(&failed(1)),
            ["Player 4 (Servant) fails to play for proposal 1 of quest 1: connection refused"]
        );
        narrator.narrate(&opens(0, 3, Phase::Quest, &["Player 1", "Player 2"]));
        narrator.narrate(&opens(0, 4, Phase::Assassinate, &[]));
        narrator.narrate(&opens(0, 5, Phase::Propose, &[]));
        let cases = [
            (2, "the vote on proposal 1 of quest 1"),
            (3, "quest 1"),
            (4, "the assassination"),
        ];
        for (seq, phase) in cases {
            assert_eq!(
                narrator.narrate(&failed(seq)),
                [format!(
                    "Player 4 (Servant) fails to play for {phase}: connection refused"
                )]
            );
        }
    }

    #[test]
    fn an_observation_or_a_choice_sent_adds_nothing_and_an_unknown_player_is_named_alone() {
        let mut narrator = dealt_five();
        let observed = Entry::Observation {
            by: ENVIRONMENT.to_string(),
            from: id("Player 1"),
            n: 1,
            seq: 1,
        };
        assert!(narrator.narrate(&logged(observed)).is_empty());
        let chose = Entry::Action {
            from: id("Player 1"),
            n: 1,
            to: HashSet::from([ENVIRONMENT.to_string()]),
            message: Choice::Ballot(Ballot::Approve).message(2, id("Player 1"), 1),
            seen: None,
            forgot: None,
        };
        assert!(narrator.narrate(&logged(chose)).is_empty());
        assert_eq!(narrator.seat(&id("stranger")).to_string(), "stranger");
        assert_eq!(narrator.listed(&[]), "");
    }

    /// A player's memory of receiving `message`.
    fn heard(message: Message) -> Event<Memory> {
        logged(Memory::Observed { message })
    }

    /// A player's memory of sending `message`.
    fn sent(message: Message) -> Event<Memory> {
        logged(Memory::Acted { message })
    }

    /// The story `me` tells of `memories`.
    fn told(me: &str, memories: impl IntoIterator<Item = Event<Memory>>) -> Story {
        let mut story = Story::of(id(me));
        for memory in memories {
            story.recall(&memory);
        }
        story
    }

    /// The opening line of the story the player in `seat` of a table of
    /// `roles`, seated from `Player 1`, tells.
    fn opening(roles: &[Role], seat: &str) -> String {
        let state = State::new(seated(roles));
        let story = told(seat, [heard(announcement(1, state.view(&id(seat))))]);
        story.lines()[0].to_string()
    }

    #[test]
    fn a_player_s_story_opens_with_who_it_is_and_what_it_sees() {
        use Role::{Assassin, Merlin, Minion, Mordred, Morgana, Oberon, Percival, Servant};
        let everyone = [
            Merlin, Percival, Servant, Assassin, Morgana, Mordred, Oberon, Minion,
        ];
        assert_eq!(
            opening(&everyone, "Player 1"),
            "You are Player 1, Merlin, on the good side. You see that Player 4, Player 5, Player 7 and Player 8 are evil."
        );
        assert_eq!(
            opening(&everyone, "Player 2"),
            "You are Player 2, Percival, on the good side. One of Player 1 and Player 5 is Merlin and the other Morgana."
        );
        assert_eq!(
            opening(&everyone, "Player 3"),
            "You are Player 3, a loyal servant of Arthur, on the good side."
        );
        assert_eq!(
            opening(&everyone, "Player 4"),
            "You are Player 4, the Assassin, on the evil side. You see that Player 5, Player 6 and Player 8 are evil."
        );
        assert_eq!(
            opening(&everyone, "Player 7"),
            "You are Player 7, Oberon, on the evil side."
        );
        let five = [Merlin, Percival, Servant, Assassin, Minion];
        assert_eq!(
            opening(&five, "Player 2"),
            "You are Player 2, Percival, on the good side. You see that Player 1 is Merlin or Morgana."
        );
        assert_eq!(
            opening(&five, "Player 5"),
            "You are Player 5, a minion of Mordred, on the evil side. You see that Player 4 is evil."
        );
    }

    #[test]
    fn a_player_s_story_tells_each_phase_how_the_one_before_came_out_and_what_it_did() {
        // Player 3, a Servant, hears Player 1 talk and propose nobody, votes
        // for Player 2's team, which it is on, supports the quest, which
        // Player 2 sabotages, and leads the next.
        let me = id("Player 3");
        let mut state = State::new(five());
        let mut memories = vec![
            heard(announcement(1, state.view(&me))),
            heard(utterance(1, "Player 1", 1, "Pick me.")),
        ];
        let next = |state: &mut State, choices: &[(&str, Choice)], seq| {
            let choices = choices
                .iter()
                .map(|(player, choice)| (id(player), choice.clone()))
                .collect();
            state.resolve(&choices, seq);
            state.next();
            heard(announcement(seq + 1, state.view(&me)))
        };
        memories.push(next(&mut state, &[], 1));
        let proposal = Choice::Team(team(&["Player 2", "Player 3"]));
        memories.push(next(&mut state, &[("Player 2", proposal)], 2));
        memories.push(sent(Choice::Ballot(Ballot::Approve).message(
            3,
            me.clone(),
            1,
        )));
        let approve = Choice::Ballot(Ballot::Approve);
        let ballots = [
            ("Player 1", approve.clone()),
            ("Player 2", approve.clone()),
            ("Player 3", approve),
            ("Player 4", Choice::Ballot(Ballot::Reject)),
        ];
        memories.push(next(&mut state, &ballots, 3));
        memories.push(sent(Choice::Deed(Deed::Support).message(4, me.clone(), 2)));
        let deeds = [("Player 2", Choice::Deed(Deed::Sabotage))];
        memories.push(next(&mut state, &deeds, 4));
        let proposed = Choice::Team(team(&["Player 1", "Player 3", "Player 4"]));
        memories.push(sent(proposed.message(5, me.clone(), 3)));
        memories.push(sent(utterance(5, "Player 3", 4, "Trust me.")));
        assert_eq!(
            told("Player 3", memories).lines(),
            [
                "You are Player 3, a loyal servant of Arthur, on the good side.",
                "",
                "Quest 1, proposal 1 of 5. Player 1 leads and needs a team of 2.",
                "Player 1 says, \"Pick me.\"",
                "Player 1 proposes nobody in time.",
                "",
                "Quest 1, proposal 2 of 5. Player 2 leads and needs a team of 2.",
                "",
                "Everyone votes on the team of Player 2 and Player 3.",
                "You approve the team.",
                "Player 1, Player 2 and Player 3 approve.",
                "Player 4 rejects.",
                "Player 5 does not vote.",
                "The team goes on the quest.",
                "",
                "Player 2 and Player 3 go on quest 1, which 1 sabotage fails.",
                "You support the quest.",
                "The quest fails, with 1 sabotage.",
                "",
                "Quest 2, proposal 1 of 5. You lead and need a team of 3.",
                "You propose Player 1, Player 3 and Player 4.",
                "You say, \"Trust me.\"",
            ]
        );
    }

    #[test]
    fn the_canned_player_knows_each_choice_its_story_tells_as_acting() {
        // The canned player that tests play reads its own choices from the
        // story, so every choice must be told in a way it recognizes, or it
        // chooses again and waits out every phase.
        let story = Story::of(id("Player 1"));
        for choice in [
            Choice::Team(team(&["Player 1", "Player 2"])),
            Choice::Ballot(Ballot::Approve),
            Choice::Ballot(Ballot::Reject),
            Choice::Deed(Deed::Support),
            Choice::Deed(Deed::Sabotage),
            Choice::Target(id("Player 2")),
        ] {
            let told = story.chose(&choice);
            let asked = format!("Quest 1.\n\nThe phase.\n{told}\n\nIt is quest 1.");
            assert!(acted_in_this_phase(&asked), "{told}");
        }
    }

    #[test]
    fn a_player_s_story_tells_only_its_first_choice_in_a_phase_the_one_counted() {
        // Player 2 votes twice on Player 1's team, the second time the other
        // way, and once more as the quest begins, then supports the quest
        // three times.
        let me = id("Player 2");
        let mut state = State::new(five());
        state.phase = Phase::Vote;
        state.team = Some(team(&["Player 1", "Player 2"]));
        let vote = |ballot, n| sent(Choice::Ballot(ballot).message(1, me.clone(), n));
        let mut memories = vec![
            heard(announcement(1, state.view(&me))),
            vote(Ballot::Reject, 1),
            vote(Ballot::Approve, 2),
        ];
        let ballot = |player, ballot| (id(player), Choice::Ballot(ballot));
        let ballots = [
            ballot("Player 1", Ballot::Approve),
            ballot("Player 2", Ballot::Reject),
            ballot("Player 3", Ballot::Approve),
            ballot("Player 4", Ballot::Approve),
        ]
        .into_iter()
        .collect();
        state.resolve(&ballots, 1);
        state.next();
        memories.push(heard(announcement(2, state.view(&me))));
        memories.push(vote(Ballot::Approve, 3));
        for n in 4..7 {
            memories.push(sent(Choice::Deed(Deed::Support).message(2, me.clone(), n)));
        }
        assert_eq!(
            told("Player 2", memories).lines(),
            [
                "You are Player 2, the Assassin, on the evil side. You see that Player 5 is evil.",
                "",
                "Everyone votes on the team of Player 1 and Player 2.",
                "You reject the team.",
                "Player 1, Player 3 and Player 4 approve.",
                "Player 2 rejects.",
                "Player 5 does not vote.",
                "The team goes on the quest.",
                "",
                "Player 1 and Player 2 go on quest 1, which 1 sabotage fails.",
                "You support the quest.",
            ]
        );
    }

    #[test]
    fn a_rejected_team_is_told_by_its_tally_and_the_assassination_by_its_heading() {
        let me = id("Player 2");
        let mut state = State::new(five());
        state.phase = Phase::Vote;
        state.team = Some(team(&["Player 1", "Player 2"]));
        let mut memories = vec![heard(announcement(1, state.view(&me)))];
        let ballots = [("Player 2", Choice::Ballot(Ballot::Reject))]
            .map(|(player, choice)| (id(player), choice))
            .into();
        state.resolve(&ballots, 1);
        state.next();
        memories.push(heard(announcement(2, state.view(&me))));
        state.phase = Phase::Assassinate;
        memories.push(heard(announcement(3, state.view(&me))));
        memories.push(sent(Choice::Target(id("Player 1")).message(
            3,
            me.clone(),
            1,
        )));
        let story = told("Player 2", memories);
        assert_eq!(
            story.lines()[3..],
            [
                "Player 2 rejects.",
                "Player 1, Player 3, Player 4 and Player 5 do not vote.",
                "The team does not go.",
                "",
                "Quest 1, proposal 2 of 5. You lead and need a team of 2.",
                "",
                "The Assassin tries to name Merlin.",
                "You name Player 1 as Merlin.",
            ]
        );
    }

    #[test]
    fn a_memory_that_is_neither_an_announcement_an_utterance_nor_a_choice_adds_nothing() {
        let view = view_of("Player 3", Phase::Propose, &[]);
        let story = told(
            "Player 3",
            [
                heard(announcement(1, view)),
                heard(Choice::Ballot(Ballot::Approve).message(1, id("Player 1"), 1)),
                sent(Message::Wake),
            ],
        );
        assert_eq!(story.lines().len(), 3);
    }

    /// `word` said at length.
    fn at_length(word: &str) -> String {
        [word; 20].join(" ")
    }

    /// Two phases of talk at length: Player 3's story of Player 1 and
    /// Player 2 talking in the first proposal, and itself in the second.
    fn two_phases_of_talk() -> Story {
        let me = id("Player 3");
        let mut state = State::new(five());
        let mut memories = vec![
            heard(announcement(1, state.view(&me))),
            heard(utterance(1, "Player 1", 1, &at_length("First."))),
            heard(utterance(1, "Player 2", 1, &at_length("Second."))),
        ];
        state.resolve(&HashMap::new(), 1);
        state.next();
        memories.push(heard(announcement(2, state.view(&me))));
        memories.push(sent(utterance(2, "Player 3", 1, &at_length("Third."))));
        told("Player 3", memories)
    }

    #[test]
    fn a_story_within_its_budget_is_told_whole() {
        let story = two_phases_of_talk();
        let lines = story.lines();
        let budget = estimated(&lines);
        assert_eq!(story.told_within(budget), (lines.clone(), 0));
        assert_eq!(story.told_within(u64::MAX), (lines, 0));
    }

    #[test]
    fn a_story_over_its_budget_forgets_talk_oldest_first_in_its_place() {
        let story = two_phases_of_talk();
        let lines = story.lines();
        let budget = estimated(&lines) - 1;
        let (told, forgot) = story.told_within(budget);
        assert_eq!(forgot, 1);
        assert_eq!(told[3], FORGOTTEN);
        assert_eq!(
            told[4],
            format!("Player 2 says, \"{}\"", at_length("Second."))
        );
        assert!(estimated(&told) <= budget, "{told:?}");
        // Short of everything, every line of talk is forgotten, and the
        // last one forgotten is where the story says so.
        let (told, forgot) = story.told_within(0);
        assert_eq!(forgot, 3);
        assert_eq!(
            told,
            [
                "You are Player 3, a loyal servant of Arthur, on the good side.",
                "",
                "Quest 1, proposal 1 of 5. Player 1 leads and needs a team of 2.",
                "Player 1 proposes nobody in time.",
                "",
                "Quest 1, proposal 2 of 5. Player 2 leads and needs a team of 2.",
                FORGOTTEN,
            ]
        );
        // The story itself keeps every line.
        assert_eq!(story.lines(), lines);
    }

    #[test]
    fn an_utterance_is_said_by_its_speaker_named_with_its_role() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let spoke = logged(Entry::Action {
            from: id("Player 3"),
            n: 1,
            to: HashSet::from([id("Player 1")]),
            message: utterance(1, "Player 3", 1, "Take me."),
            seen: None,
            forgot: None,
        });
        assert_eq!(
            narrator.narrate(&spoke),
            [Line::Said {
                who: id("Player 3"),
                speaker: "Player 3 (Servant)".to_string(),
                text: "Take me.".to_string(),
            }]
        );
    }

    #[test]
    fn a_proposal_opens_with_a_blank_line_and_its_heading() {
        let mut narrator = dealt_five();
        assert_eq!(
            narrator.narrate(&opens(0, 1, Phase::Propose, &[])),
            [
                Line::Blank,
                Line::Heading(
                    "Quest 1, proposal 1 of 5. Player 1 (Merlin) leads and needs a team of 2."
                        .to_string()
                )
            ]
        );
    }

    #[test]
    fn a_tallys_ballots_are_votes_and_whether_the_team_goes_is_told() {
        let narrator = dealt_five();
        let tally = Tally::of(
            HashMap::from([
                (id("Player 1"), Ballot::Approve),
                (id("Player 2"), Ballot::Reject),
            ]),
            5,
        );
        let vote = |line: &str| Line::Vote(line.to_string());
        assert_eq!(
            narrator.tallied(&tally),
            [
                vote("Player 1 (Merlin) approves."),
                vote("Player 2 (Assassin) rejects."),
                vote("Player 3 (Servant), Player 4 (Servant) and Player 5 (Minion) do not vote."),
                Line::Told("The team does not go.".to_string()),
            ]
        );
    }
}
