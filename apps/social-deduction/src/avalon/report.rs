//! A legible account of a game, of everything, told from the log as it is
//! written.
//!
//! The log is written in actions and observations, by every actor. The
//! account reads the environment's: each phase is told from its
//! announcements, and how it came out from the entry that ends it. Talk is
//! told from its speaker's action.

use super::{Ballot, Choice, Deed, Entry, Message, PROPOSALS, Phase, Role, Tally, Victory, View};
use crate::model::ToolChoice;
use crate::model::window::{ContextWindow, Source};
use crate::report::{in_order, whole_seconds};
use crate::seat::PlayerId;
use free_agent::Event;
use humantime::format_duration;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::time::{Duration, SystemTime};

/// Turns the events of a game, one at a time, into lines of prose. It
/// reads the log's events through [`Narrator::narrate`] and tells the
/// table, a player to a line with its role and strategy, and how long each
/// phase waits, then each model in play, its context window and whether
/// its players are made to call a tool, then each phase as it opens and how it came out, and how
/// long it took, then who won, how, and how long the whole game took. It
/// names a player with its role in parentheses, and lists players in seat
/// order. A blank line sets each phase off from what came before it, and
/// another the winner from the last. Talk is told as it comes, or as too
/// late to say if its phase has ended; a message a player withheld because
/// its phase had ended is told as too late, and a player's failure to act
/// as it happens, with what went wrong.
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

impl Narrator {
    /// The lines the log's `event` adds to the account, often none.
    pub fn narrate(&mut self, event: &Event<Entry>) -> Vec<String> {
        let time = event.time;
        match &event.payload {
            Entry::Start {
                roles,
                strategies,
                propose_limit,
                vote_limit,
                quest_limit,
                assassinate_limit,
            } => {
                self.roles = roles.clone();
                self.strategies = strategies.clone();
                self.table([
                    *propose_limit,
                    *vote_limit,
                    *quest_limit,
                    *assassinate_limit,
                ])
            }
            Entry::Configuration(configuration) => {
                let mut lines = Vec::new();
                for (name, model) in &configuration.models {
                    lines.push(context_window(name, model.context_window));
                    if model.tool_choice == ToolChoice::Auto {
                        lines.push(format!(
                            "{name} does not accept a required tool call, so its players are asked to call one."
                        ));
                    }
                }
                lines
            }
            Entry::Action {
                message: Message::Announce { seq, view, .. },
                ..
            } => self.announce(*seq, view, time),
            Entry::Action {
                message: Message::Say {
                    seq, from, text, ..
                },
                ..
            } => vec![match self.late(*seq) {
                true => self.unsaid(from, *seq, text),
                false => self.said(from, text),
            }],
            Entry::Withheld {
                from,
                message: Message::Say { seq, text, .. },
                ..
            } => vec![self.unsaid(from, *seq, text)],
            Entry::Withheld { from, message, .. } => message
                .choice()
                .map(|(seq, _, choice)| self.too_late(from, &choice, seq))
                .into_iter()
                .collect(),
            Entry::Failed { player, seq, error } => {
                let player = self.seat(player);
                vec![
                    match self.late(*seq).then(|| self.phase_name(*seq)).flatten() {
                        Some(phase) => format!("{player} fails to play for {phase}: {error}"),
                        None => format!("{player} fails to play: {error}"),
                    },
                ]
            }
            Entry::Proposed { seq, team } => {
                let leader = match self.announced.get(seq) {
                    Some(view) => self.seat(&view.leader).to_string(),
                    None => "The leader".to_string(),
                };
                vec![match team {
                    Some(team) => format!("{leader} proposes {}.", self.listed(team)),
                    None => format!("{leader} proposes nobody in time."),
                }]
            }
            Entry::Tallied { tally, .. } => self.tallied(tally),
            Entry::Quested { result, .. } => {
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
                vec![format!("The quest {comes}, with {sabotages}.")]
            }
            Entry::Assassinated { target, merlin, .. } => vec![match target {
                Some(target) => format!(
                    "The Assassin names {}, who is {}Merlin.",
                    self.seat(target),
                    if *merlin { "" } else { "not " }
                ),
                None => "The Assassin names nobody in time.".to_string(),
            }],
            Entry::End { by, took, .. } => self.end(*by, *took, time),
            _ => vec![],
        }
    }

    /// The deal: how many players, then each with its role and its
    /// strategy on a line of its own, then how long each phase waits.
    fn table(&self, [propose, vote, quest, assassinate]: [Duration; 4]) -> Vec<String> {
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
            .collect()
    }

    /// `player` as this account names it: with its role when known.
    fn seat<'a>(&self, player: &'a PlayerId) -> Seat<'a> {
        Seat {
            player,
            role: self.roles.get(player).copied(),
        }
    }

    /// `players`, as this account names them, as a list in prose, in the
    /// order given.
    fn listed(&self, players: &[PlayerId]) -> String {
        let named: Vec<String> = players
            .iter()
            .map(|player| self.seat(player).to_string())
            .collect();
        match named.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => named.concat(),
        }
    }

    /// What the announcement at `time` of the phase numbered `seq`, showing
    /// `view`, adds to the account: nothing if the phase has begun, and
    /// otherwise the end of the phase before it, then a blank line and what
    /// the new phase is.
    fn announce(&mut self, seq: u64, view: &View, time: SystemTime) -> Vec<String> {
        if self.announced.contains_key(&seq) {
            return vec![];
        }
        self.announced.insert(seq, view.clone());
        let mut lines = self.finish(time);
        lines.push(String::new());
        lines.push(match view.phase {
            Phase::Propose => format!(
                "Quest {}, proposal {} of {PROPOSALS}. {} leads and needs a team of {}.",
                view.quest,
                view.proposal,
                self.seat(&view.leader),
                view.size
            ),
            Phase::Vote => "Everyone votes on the team.".to_string(),
            Phase::Quest => {
                let team: Vec<PlayerId> = view.team.iter().flatten().cloned().collect();
                let fail = match view.sabotages_needed {
                    1 => "1 sabotage fails".to_string(),
                    needed => format!("{needed} sabotages fail"),
                };
                format!(
                    "{} go on quest {}, which {fail}.",
                    self.listed(&team),
                    view.quest
                )
            }
            Phase::Assassinate => "The Assassin tries to name Merlin.".to_string(),
        });
        self.phase = Some(Told {
            phase: view.phase,
            began: time,
        });
        lines
    }

    /// How long the phase being told took, if one was, as it ends at
    /// `time`.
    fn finish(&mut self, time: SystemTime) -> Vec<String> {
        self.phase
            .take()
            .map(|told| {
                let took = time.duration_since(told.began).unwrap_or_default();
                let phase = match told.phase {
                    Phase::Propose => "proposal",
                    Phase::Vote => "vote",
                    Phase::Quest => "quest",
                    Phase::Assassinate => "assassination",
                };
                format!("The {phase} took {}.", whole_seconds(took))
            })
            .into_iter()
            .collect()
    }

    /// How a vote came out: who approved, who rejected, and who did not
    /// vote, each in seat order and left out when nobody did, then whether
    /// the team goes.
    fn tallied(&self, tally: &Tally) -> Vec<String> {
        let players = in_order(self.roles.keys());
        let voted = |ballot| -> Vec<PlayerId> {
            players
                .iter()
                .filter(|player| tally.ballots.get(**player) == ballot)
                .map(|player| (*player).clone())
                .collect()
        };
        let mut lines: Vec<String> = [
            (voted(Some(&Ballot::Approve)), "approves", "approve"),
            (voted(Some(&Ballot::Reject)), "rejects", "reject"),
            (voted(None), "does not vote", "do not vote"),
        ]
        .into_iter()
        .filter(|(who, _, _)| !who.is_empty())
        .map(|(who, one, several)| {
            let verb = if who.len() == 1 { one } else { several };
            format!("{} {verb}.", self.listed(&who))
        })
        .collect();
        lines.push(
            match tally.approved {
                true => "The team goes on the quest.",
                false => "The team does not go.",
            }
            .to_string(),
        );
        lines
    }

    /// The end of the game at `time`: how long the last phase took, then a
    /// blank line, who won and how, and how long the game took.
    fn end(&mut self, by: Victory, took: Duration, time: SystemTime) -> Vec<String> {
        let mut lines = self.finish(time);
        lines.push(String::new());
        lines.push(
            match by {
                Victory::Quests => "Good wins with three successful quests.",
                Victory::Sabotage => "Evil wins with three failed quests.",
                Victory::Rejections => "Evil wins with five failed proposals in a row.",
                Victory::Assassination => "Evil wins by naming Merlin.",
            }
            .to_string(),
        );
        lines.push(format!("The game took {}.", whole_seconds(took)));
        lines
    }

    /// `player` says `text`.
    fn said(&self, player: &PlayerId, text: &str) -> String {
        format!("{} says, \"{text}\"", self.seat(player))
    }

    /// `player` was too late to say `text` in the phase numbered `seq`,
    /// which had ended: when its model answered, so it withheld it, or
    /// before the utterance reached the log. The phase is named when it was
    /// announced.
    fn unsaid(&self, player: &PlayerId, seq: u64, text: &str) -> String {
        let seat = self.seat(player);
        match self.phase_name(seq) {
            Some(phase) => format!("{seat} is too late to say, for {phase}: \"{text}\""),
            None => format!("{seat} is too late to say: \"{text}\""),
        }
    }

    /// `player` was too late to make `choice` in the phase numbered `seq`,
    /// which had ended when its model answered. The phase is named when it
    /// was announced.
    fn too_late(&self, player: &PlayerId, choice: &Choice, seq: u64) -> String {
        let what = match choice {
            Choice::Team(team) => format!("propose {}", self.listed(team)),
            Choice::Ballot(Ballot::Approve) => "approve the team".to_string(),
            Choice::Ballot(Ballot::Reject) => "reject the team".to_string(),
            Choice::Deed(Deed::Support) => "support the quest".to_string(),
            Choice::Deed(Deed::Sabotage) => "sabotage the quest".to_string(),
            Choice::Target(target) => format!("name {} as Merlin", self.seat(target)),
        };
        let when = self
            .phase_name(seq)
            .map_or(String::new(), |phase| format!(" for {phase}"));
        format!("{} is too late to {what}{when}.", self.seat(player))
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

    /// Whether a later phase than the one numbered `seq` has been announced
    /// since: a message for that phase comes late.
    fn late(&self, seq: u64) -> bool {
        self.announced.range(seq + 1..).next().is_some()
    }
}

/// A player as an account names it: `player (role)` when the role is
/// known, the name alone when it is not.
#[derive(Debug, Clone, Copy)]
struct Seat<'a> {
    player: &'a PlayerId,
    role: Option<Role>,
}

impl fmt::Display for Seat<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.role {
            Some(role) => write!(f, "{} ({role})", self.player),
            None => f.write_str(self.player),
        }
    }
}

/// The context window of the model `name` and where it came from, in a
/// line.
fn context_window(name: &str, ContextWindow { tokens, source }: ContextWindow) -> String {
    let source = match source {
        Source::File => "as the configuration file says",
        Source::Provider => "as the provider reports",
        Source::ProviderDefault => "the provider's default",
        Source::Default => "the default",
    };
    format!("{name}'s context window is {tokens} tokens, {source}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::config::Configuration;
    use crate::avalon::tests::{configured, dealt, five, logged, utterance, view_of};
    use crate::avalon::{QuestResult, Rules};
    use crate::model::spec::ModelInPlay;
    use crate::seat::ENVIRONMENT;
    use crate::strategy::RANDOM;
    use std::collections::HashSet;
    use std::time::UNIX_EPOCH;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// `entry`, logged `seconds` into the game.
    fn at(seconds: u64, entry: Entry) -> Event<Entry> {
        Event {
            time: UNIX_EPOCH + Duration::from_secs(seconds),
            ..logged(entry)
        }
    }

    /// A narrator that has been told the deal of the default table.
    fn dealt_five() -> Narrator {
        let mut narrator = Narrator::default();
        narrator.narrate(&logged(Rules::default().start(&dealt(&five(), RANDOM))));
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
        at(
            seconds,
            announced("player1", seq, view_of("player1", phase, team)),
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
        let told = narrator.narrate(&logged(rules.start(&dealt(&five(), RANDOM))));
        assert_eq!(
            told,
            [
                "5 players:",
                "player1, Merlin, plays random",
                "player2, Assassin, plays random",
                "player3, Servant, plays random",
                "player4, Servant, plays random",
                "player5, Minion, plays random",
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
                "Quest 1, proposal 1 of 5. player1 (Merlin) leads and needs a team of 2."
            ]
        );
        // The same phase announced to another player adds nothing.
        let again = announced("player2", 1, view_of("player2", Phase::Propose, &[]));
        assert!(narrator.narrate(&at(0, again)).is_empty());
        assert_eq!(
            narrator.narrate(&opens(3, 2, Phase::Vote, &["player1", "player3"])),
            ["The proposal took 3s.", "", "Everyone votes on the team."]
        );
        assert_eq!(
            narrator.narrate(&opens(5, 3, Phase::Quest, &["player1", "player3"])),
            [
                "The vote took 2s.",
                "",
                "player1 (Merlin) and player3 (Servant) go on quest 1, which 1 sabotage fails."
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
            narrator.narrate(&at(
                70,
                Entry::End {
                    winner: crate::avalon::Team::Good,
                    by: Victory::Quests,
                    took: Duration::from_millis(70_400),
                }
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
        let mut view = view_of("player1", Phase::Quest, &["player1", "player2", "player3"]);
        view.sabotages_needed = 2;
        assert_eq!(
            narrator.narrate(&logged(announced("player1", 1, view))),
            [
                "",
                "player1 (Merlin), player2 (Assassin) and player3 (Servant) go on quest 1, which 2 sabotages fail."
            ]
        );
    }

    #[test]
    fn a_proposal_names_its_team_or_nobody_in_time() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let proposed = Entry::Proposed {
            seq: 1,
            team: Some(team(&["player1", "player4", "player5"])),
        };
        assert_eq!(
            narrator.narrate(&logged(proposed)),
            ["player1 (Merlin) proposes player1 (Merlin), player4 (Servant) and player5 (Minion)."]
        );
        let lapsed = Entry::Proposed { seq: 1, team: None };
        assert_eq!(
            narrator.narrate(&logged(lapsed)),
            ["player1 (Merlin) proposes nobody in time."]
        );
        // A proposal never announced names no leader.
        let unknown = Entry::Proposed {
            seq: 9,
            team: Some(team(&["player2"])),
        };
        assert_eq!(
            narrator.narrate(&logged(unknown)),
            ["The leader proposes player2 (Assassin)."]
        );
    }

    #[test]
    fn a_tally_tells_who_approved_who_rejected_who_did_not_vote_and_whether_the_team_goes() {
        let narrator = dealt_five();
        let tally = Tally::of(
            HashMap::from([
                (id("player1"), Ballot::Approve),
                (id("player3"), Ballot::Approve),
                (id("player2"), Ballot::Reject),
            ]),
            5,
        );
        assert_eq!(
            narrator.tallied(&tally),
            [
                "player1 (Merlin) and player3 (Servant) approve.",
                "player2 (Assassin) rejects.",
                "player4 (Servant) and player5 (Minion) do not vote.",
                "The team does not go."
            ]
        );
        let ballots = ["player1", "player2", "player3", "player4", "player5"]
            .map(|player| {
                let ballot = match player {
                    "player2" | "player5" => Ballot::Reject,
                    _ => Ballot::Approve,
                };
                (id(player), ballot)
            })
            .into();
        let mut narrator = dealt_five();
        assert_eq!(
            narrator.narrate(&logged(Entry::Tallied {
                seq: 2,
                tally: Tally::of(ballots, 5),
            })),
            [
                "player1 (Merlin), player3 (Servant) and player4 (Servant) approve.",
                "player2 (Assassin) and player5 (Minion) reject.",
                "The team goes on the quest."
            ]
        );
        let lone = Tally::of(HashMap::from([(id("player4"), Ballot::Reject)]), 5);
        assert_eq!(
            narrator.tallied(&lone)[1],
            "player1 (Merlin), player2 (Assassin), player3 (Servant) and player5 (Minion) do not vote."
        );
    }

    #[test]
    fn a_quest_tells_whether_it_succeeds_and_how_many_sabotaged_it() {
        let mut narrator = dealt_five();
        let quested = |sabotages, succeeded| {
            logged(Entry::Quested {
                seq: 3,
                result: QuestResult {
                    team: team(&["player1", "player2"]),
                    sabotages,
                    succeeded,
                },
            })
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
            logged(Entry::Assassinated {
                seq: 10,
                target: target.map(id),
                merlin,
            })
        };
        assert_eq!(
            narrator.narrate(&named(Some("player1"), true)),
            ["The Assassin names player1 (Merlin), who is Merlin."]
        );
        assert_eq!(
            narrator.narrate(&named(Some("player3"), false)),
            ["The Assassin names player3 (Servant), who is not Merlin."]
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
            let end = Entry::End {
                winner: crate::avalon::Team::Evil,
                by,
                took: Duration::from_secs(40),
            };
            assert_eq!(
                narrator.narrate(&logged(end)),
                ["", told, "The game took 40s."]
            );
        }
    }

    #[test]
    fn talk_is_told_as_it_is_said_or_as_too_late_to_say() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let spoke = |seq, text| {
            logged(Entry::Action {
                from: id("player3"),
                n: 1,
                to: HashSet::from([id("player1")]),
                message: utterance(seq, "player3", 1, text),
                seen: None,
                forgot: None,
            })
        };
        assert_eq!(
            narrator.narrate(&spoke(1, "Take me.")),
            ["player3 (Servant) says, \"Take me.\""]
        );
        narrator.narrate(&opens(1, 2, Phase::Vote, &["player1", "player3"]));
        assert_eq!(
            narrator.narrate(&spoke(1, "Take me.")),
            ["player3 (Servant) is too late to say, for proposal 1 of quest 1: \"Take me.\""]
        );
        let withheld = |message| {
            logged(Entry::Withheld {
                from: id("player3"),
                message,
                seen: Some(4),
                forgot: None,
            })
        };
        assert_eq!(
            narrator.narrate(&withheld(utterance(2, "player3", 0, "Yes."))),
            [
                "player3 (Servant) is too late to say, for the vote on proposal 1 of quest 1: \"Yes.\""
            ]
        );
        assert_eq!(
            narrator.narrate(&withheld(utterance(0, "player3", 0, "Hm."))),
            ["player3 (Servant) is too late to say: \"Hm.\""]
        );
    }

    #[test]
    fn a_choice_withheld_is_told_as_too_late() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        narrator.narrate(&opens(0, 3, Phase::Quest, &["player1", "player2"]));
        narrator.narrate(&opens(0, 4, Phase::Assassinate, &[]));
        let withheld = |seq, choice: Choice| {
            logged(Entry::Withheld {
                from: id("player2"),
                message: choice.message(seq, id("player2"), 0),
                seen: Some(4),
                forgot: None,
            })
        };
        let cases = [
            (
                1,
                Choice::Team(team(&["player2", "player5"])),
                "player2 (Assassin) is too late to propose player2 (Assassin) and player5 (Minion) for proposal 1 of quest 1.",
            ),
            (
                2,
                Choice::Ballot(Ballot::Approve),
                "player2 (Assassin) is too late to approve the team.",
            ),
            (
                2,
                Choice::Ballot(Ballot::Reject),
                "player2 (Assassin) is too late to reject the team.",
            ),
            (
                3,
                Choice::Deed(Deed::Support),
                "player2 (Assassin) is too late to support the quest for quest 1.",
            ),
            (
                3,
                Choice::Deed(Deed::Sabotage),
                "player2 (Assassin) is too late to sabotage the quest for quest 1.",
            ),
            (
                4,
                Choice::Target(id("player1")),
                "player2 (Assassin) is too late to name player1 (Merlin) as Merlin for the assassination.",
            ),
        ];
        for (seq, choice, told) in cases {
            assert_eq!(narrator.narrate(&withheld(seq, choice)), [told]);
        }
        // Nothing else a player would send is a choice.
        assert!(narrator.narrate(&withheld_wake()).is_empty());
    }

    /// A wake-up withheld, which no player withholds.
    fn withheld_wake() -> Event<Entry> {
        logged(Entry::Withheld {
            from: id("player2"),
            message: Message::Wake,
            seen: None,
            forgot: None,
        })
    }

    #[test]
    fn a_failure_is_told_as_it_happens_with_what_went_wrong() {
        let mut narrator = dealt_five();
        narrator.narrate(&opens(0, 1, Phase::Propose, &[]));
        let failed = |seq| {
            logged(Entry::Failed {
                player: id("player4"),
                seq,
                error: "connection refused".to_string(),
            })
        };
        assert_eq!(
            narrator.narrate(&failed(1)),
            ["player4 (Servant) fails to play: connection refused"]
        );
        narrator.narrate(&opens(0, 2, Phase::Vote, &["player1", "player2"]));
        assert_eq!(
            narrator.narrate(&failed(1)),
            ["player4 (Servant) fails to play for proposal 1 of quest 1: connection refused"]
        );
    }

    #[test]
    fn an_observation_or_a_choice_sent_adds_nothing_and_an_unknown_player_is_named_alone() {
        let mut narrator = dealt_five();
        let observed = Entry::Observation {
            by: ENVIRONMENT.to_string(),
            from: id("player1"),
            n: 1,
            seq: 1,
        };
        assert!(narrator.narrate(&logged(observed)).is_empty());
        let chose = Entry::Action {
            from: id("player1"),
            n: 1,
            to: HashSet::from([ENVIRONMENT.to_string()]),
            message: Choice::Ballot(Ballot::Approve).message(2, id("player1"), 1),
            seen: None,
            forgot: None,
        };
        assert!(narrator.narrate(&logged(chose)).is_empty());
        assert_eq!(narrator.seat(&id("stranger")).to_string(), "stranger");
        assert_eq!(narrator.listed(&[]), "");
    }

    /// The configuration of a game whose requests to each of `models`, by
    /// name, are made under its tool choice, within the default context
    /// window, as the log records it.
    fn configured_with(models: &[(&str, ToolChoice)]) -> Event<Entry> {
        let models = models
            .iter()
            .map(|(name, tool_choice)| {
                let model = ModelInPlay {
                    base_url: "http://localhost:1234/v1".to_string(),
                    id: format!("{name}-7b-instruct"),
                    request_timeout: Duration::from_secs(10),
                    api_key_env: None,
                    tool_choice: *tool_choice,
                    context_window: ContextWindow {
                        tokens: 4096,
                        source: Source::Default,
                    },
                };
                (name.to_string(), model)
            })
            .collect();
        logged(Entry::Configuration(Box::new(Configuration {
            models,
            ..configured(&five(), &Rules::default())
        })))
    }

    #[test]
    fn a_configuration_is_told_model_by_model_and_with_no_models_not_at_all() {
        let configuration =
            configured_with(&[("claude", ToolChoice::Auto), ("qwen", ToolChoice::Required)]);
        assert_eq!(
            Narrator::default().narrate(&configuration),
            [
                "claude's context window is 4096 tokens, the default.",
                "claude does not accept a required tool call, \
                 so its players are asked to call one.",
                "qwen's context window is 4096 tokens, the default.",
            ]
        );
        assert!(
            Narrator::default()
                .narrate(&configured_with(&[]))
                .is_empty()
        );
    }

    #[test]
    fn the_context_window_is_told_with_where_it_came_from() {
        let told = |tokens, source| context_window("qwen", ContextWindow { tokens, source });
        assert_eq!(
            told(4096, Source::Default),
            "qwen's context window is 4096 tokens, the default."
        );
        assert_eq!(
            told(32768, Source::Provider),
            "qwen's context window is 32768 tokens, as the provider reports."
        );
        assert_eq!(
            told(128000, Source::ProviderDefault),
            "qwen's context window is 128000 tokens, the provider's default."
        );
        assert_eq!(
            told(8192, Source::File),
            "qwen's context window is 8192 tokens, as the configuration file says."
        );
    }
}
