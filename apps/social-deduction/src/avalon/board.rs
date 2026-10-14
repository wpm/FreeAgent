//! The Avalon board as the viewer draws it. Every choice moves it as the
//! player sends it, read from the player's action, and each phase's
//! outcome settles it.

use super::{
    Avalon, Ballot, Deed, Entry, Message, Move, Outcome, Phase, QuestResult, Role, Rules, View,
    sabotages_needed, team_size,
};
use crate::game::Tagged;
use crate::report::in_order;
use crate::seat::PlayerId;
use crate::stage::{self, millis};
use free_agent::Event;
use social_deduction_stage as cue;
use social_deduction_stage::{AvalonPanel, AvalonSeat, Millis, Panel, ProposalRow, QuestToken};
use std::collections::HashMap;
use std::num::NonZero;
use std::time::Duration;

/// The Avalon board, once the deal has set it.
#[derive(Debug, Default)]
pub struct Board {
    /// Every role, from the deal.
    roles: HashMap<PlayerId, Role>,
    /// The latest phase announced, by its number: 0 before the first.
    seq: u64,
    /// The board, from the deal on.
    panel: Option<AvalonPanel>,
}

impl stage::Board<Avalon> for Board {
    fn fold(&mut self, event: &Event<Entry>) -> Option<Panel> {
        let before = self.panel.clone();
        self.apply(event);
        (self.panel != before)
            .then(|| self.panel.clone().map(Panel::Avalon))
            .flatten()
    }
}

impl Board {
    /// Move the board by `event`.
    fn apply(&mut self, event: &Event<Entry>) {
        let at = millis(event.time);
        match &event.payload {
            Entry::Start { roles, limits, .. } => self.start(roles, limits, at),
            Entry::Action {
                message:
                    Message::Announce {
                        seq, view, limit, ..
                    },
                ..
            } if *seq > self.seq => self.announce(*seq, view, *limit, at),
            Entry::Action {
                message: Message::Move(chosen),
                ..
            } if chosen.seq() == self.seq => self.chosen(chosen),
            Entry::Event(outcome) => self.settle(outcome),
            _ => {}
        }
    }

    /// The deal of `roles` at `at`, under `rules`: the first proposal of
    /// the first quest, led by the first seat, with the quest track, and
    /// every seat with whom it sees.
    fn start(&mut self, roles: &HashMap<PlayerId, Role>, rules: &Rules, at: Millis) {
        self.roles = roles.clone();
        let seats: Vec<PlayerId> = in_order(roles.keys()).into_iter().cloned().collect();
        let players = seats.len();
        let quests = (1..=5)
            .map(|quest| {
                let quest = NonZero::new(quest).unwrap();
                QuestToken {
                    size: team_size(players, quest),
                    sabotages_needed: sabotages_needed(players, quest),
                    result: None,
                }
            })
            .collect();
        let sees = |me: &PlayerId| {
            seats
                .iter()
                .filter(|other| *other != me && roles[me].sees(roles[*other]).is_some())
                .cloned()
                .collect()
        };
        self.panel = Some(AvalonPanel {
            quest: 1,
            proposal: 1,
            phase: cue::AvalonPhase::Propose,
            began: at,
            limit_ms: millis_of(rules.propose_limit),
            leader: seats[0].clone(),
            team: None,
            deeds: vec![],
            quests,
            proposals: vec![],
            seats: seats
                .iter()
                .map(|name| AvalonSeat {
                    name: name.clone(),
                    sees: sees(name),
                })
                .collect(),
            assassination: None,
        });
    }

    /// The phase numbered `seq` opens at `at` for `limit`, as `view` shows
    /// it.
    fn announce(&mut self, seq: u64, view: &View, limit: Duration, at: Millis) {
        self.seq = seq;
        let assassin = self.assassin();
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        panel.quest = view.quest.get();
        panel.proposal = view.proposal.get();
        panel.phase = phase(view.phase);
        panel.leader = view.leader.clone();
        panel.began = at;
        panel.limit_ms = millis_of(limit);
        panel.team = match view.phase {
            Phase::Propose => None,
            _ => view.team.clone(),
        };
        panel.deeds = match view.phase {
            Phase::Quest => view
                .team
                .iter()
                .flatten()
                .map(|member| (member.clone(), None))
                .collect(),
            _ => vec![],
        };
        match view.phase {
            Phase::Vote => {
                let team = view.team.clone();
                proposed(panel, team);
            }
            Phase::Assassinate => {
                panel.assassination = assassin.map(|assassin| cue::Assassination {
                    assassin,
                    target: None,
                    merlin: None,
                });
            }
            _ => {}
        }
    }

    /// A player sends `chosen` in the phase underway.
    fn chosen(&mut self, chosen: &Move) {
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        match chosen {
            Move::Propose { team, .. } => {
                panel.team = Some(team.clone());
                proposed(panel, Some(team.clone()));
            }
            Move::Vote { from, ballot, .. } => {
                if let Some(row) = current(panel) {
                    set(&mut row.ballots, from, Some(*ballot == Ballot::Approve));
                }
            }
            Move::Perform { from, deed, .. } => {
                set(&mut panel.deeds, from, Some(self::deed(*deed)))
            }
            Move::Select { target, .. } => {
                if let Some(assassination) = panel.assassination.as_mut() {
                    assassination.target = Some(target.clone());
                }
            }
        }
    }

    /// The phase underway comes out as `outcome` says.
    fn settle(&mut self, outcome: &Outcome) {
        let assassin = self.assassin();
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        match outcome {
            Outcome::Proposed { team, .. } => proposed(panel, team.clone()),
            Outcome::Tallied { tally, .. } => {
                if let Some(row) = current(panel) {
                    row.approved = Some(tally.approved);
                }
            }
            Outcome::Quested { result, .. } => {
                let result = quest_result(result);
                if let Some(row) = current(panel) {
                    row.result = Some(result.clone());
                }
                panel.quests[usize::from(panel.quest) - 1].result = Some(result);
            }
            Outcome::Assassinated { target, merlin, .. } => {
                panel.assassination = assassin.map(|assassin| cue::Assassination {
                    assassin,
                    target: target.clone(),
                    merlin: Some(*merlin),
                });
            }
            Outcome::End { .. } => {}
        }
    }

    /// The Assassin, if the table has one.
    fn assassin(&self) -> Option<PlayerId> {
        self.roles
            .iter()
            .find(|(_, role)| **role == Role::Assassin)
            .map(|(name, _)| name.clone())
    }
}

/// The row of the proposal underway on `panel`, if it has one.
fn current(panel: &mut AvalonPanel) -> Option<&mut ProposalRow> {
    let (quest, proposal) = (panel.quest, panel.proposal);
    panel
        .proposals
        .iter_mut()
        .find(|row| row.quest == quest && row.proposal == proposal)
}

/// The proposal underway on `panel` names `team`, or nobody yet: its row,
/// added first with every ballot still to come if it has none, takes the
/// team when there is one.
fn proposed(panel: &mut AvalonPanel, team: Option<Vec<PlayerId>>) {
    if current(panel).is_none() {
        let ballots = panel
            .seats
            .iter()
            .map(|seat| (seat.name.clone(), None))
            .collect();
        let row = ProposalRow {
            quest: panel.quest,
            proposal: panel.proposal,
            leader: panel.leader.clone(),
            team: None,
            ballots,
            approved: None,
            result: None,
        };
        panel.proposals.insert(0, row);
    }
    if let (Some(row), Some(team)) = (current(panel), team) {
        row.team = Some(team);
    }
}

/// Set `player`'s entry in `entries` to `value`.
fn set<T>(entries: &mut [(PlayerId, Option<T>)], player: &PlayerId, value: Option<T>) {
    if let Some((_, entry)) = entries.iter_mut().find(|(name, _)| name == player) {
        *entry = value;
    }
}

/// `duration` in whole milliseconds.
fn millis_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// `phase` as the board shows it.
fn phase(phase: Phase) -> cue::AvalonPhase {
    match phase {
        Phase::Propose => cue::AvalonPhase::Propose,
        Phase::Vote => cue::AvalonPhase::Vote,
        Phase::Quest => cue::AvalonPhase::Quest,
        Phase::Assassinate => cue::AvalonPhase::Assassinate,
    }
}

/// `deed` as the board shows it.
fn deed(deed: Deed) -> cue::Deed {
    match deed {
        Deed::Support => cue::Deed::Support,
        Deed::Sabotage => cue::Deed::Sabotage,
    }
}

/// `result` as the board shows it.
fn quest_result(result: &QuestResult) -> cue::QuestResult {
    cue::QuestResult {
        team: result.team.clone(),
        sabotages: result.sabotages,
        succeeded: result.succeeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::Tally;
    use crate::avalon::tests::{announcement, five, logged, quick, seated};
    use crate::seat::{ENVIRONMENT, id, to_the_environment};
    use crate::stage::Board as _;
    use std::collections::HashSet;
    use std::time::UNIX_EPOCH;

    /// The default table's names, in seat order.
    fn players() -> Vec<PlayerId> {
        (1..=5).map(|seat| format!("Player {seat}")).collect()
    }

    /// `names` as players.
    fn ids(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| id(name)).collect()
    }

    /// The default table's view of the `proposal`th proposal of the
    /// `quest`th quest in `phase`, led by `leader`, with `team` on the
    /// table.
    fn view(quest: u8, proposal: u8, phase: Phase, leader: &str, team: &[&str]) -> View {
        View {
            quest: NonZero::new(quest).unwrap(),
            proposal: NonZero::new(proposal).unwrap(),
            phase,
            leader: id(leader),
            size: 2,
            sabotages_needed: 1,
            team: (!team.is_empty()).then(|| ids(team)),
            tally: None,
            record: vec![],
            role: Role::Merlin,
            sees: HashMap::new(),
            players: players(),
        }
    }

    /// `payload`, logged `seconds` after the epoch.
    fn at(seconds: u64, payload: Entry) -> Event<Entry> {
        Event {
            time: UNIX_EPOCH + Duration::from_secs(seconds),
            ..logged(payload)
        }
    }

    /// The deal of `roles` under quick limits.
    fn start(roles: HashMap<PlayerId, Role>) -> Event<Entry> {
        let strategies = roles
            .keys()
            .map(|name| (name.clone(), "random".to_string()));
        at(
            0,
            Entry::Start {
                strategies: strategies.collect(),
                roles,
                limits: quick(),
            },
        )
    }

    /// The environment's announcement of the phase numbered `seq`, showing
    /// `view`, logged `seq` seconds in.
    fn announce(seq: u64, view: View) -> Event<Entry> {
        at(
            seq,
            Entry::Action {
                from: ENVIRONMENT.to_string(),
                n: seq,
                to: HashSet::from([id("Player 1")]),
                message: announcement(seq, view),
                seen: None,
                forgot: None,
            },
        )
    }

    /// The player's action sending `chosen`.
    fn sent(chosen: Move) -> Event<Entry> {
        logged(Entry::Action {
            from: chosen.sender(),
            n: 1,
            to: to_the_environment(),
            message: Message::Move(chosen),
            seen: None,
            forgot: None,
        })
    }

    /// `from`'s proposal of `team` in the phase numbered `seq`.
    fn propose(seq: u64, from: &str, team: &[&str]) -> Event<Entry> {
        sent(Move::Propose {
            seq,
            from: id(from),
            n: 1,
            team: ids(team),
        })
    }

    /// `from`'s ballot in the phase numbered `seq`.
    fn vote(seq: u64, from: &str, approve: bool) -> Event<Entry> {
        let ballot = if approve {
            Ballot::Approve
        } else {
            Ballot::Reject
        };
        sent(Move::Vote {
            seq,
            from: id(from),
            n: 1,
            ballot,
        })
    }

    /// `from`'s deed in the phase numbered `seq`.
    fn perform(seq: u64, from: &str, deed: Deed) -> Event<Entry> {
        sent(Move::Perform {
            seq,
            from: id(from),
            n: 1,
            deed,
        })
    }

    /// How a phase came out.
    fn settled(outcome: Outcome) -> Event<Entry> {
        logged(Entry::Event(outcome))
    }

    /// A vote approved or not.
    fn tallied(seq: u64, approved: bool) -> Event<Entry> {
        settled(Outcome::Tallied {
            seq,
            tally: Tally {
                ballots: HashMap::new(),
                approved,
            },
        })
    }

    /// The board after each of `events` in turn, `None` where it did not
    /// change, checking that the board folded out is the board held.
    fn boards(events: &[Event<Entry>]) -> Vec<Option<AvalonPanel>> {
        let mut board = Board::default();
        events
            .iter()
            .map(|event| {
                let folded = board.fold(event);
                let changed = folded.as_ref().and(board.panel.clone());
                assert_eq!(folded, changed.clone().map(Panel::Avalon));
                changed
            })
            .collect()
    }

    /// The board after the last of `events` that changed it.
    fn last(events: &[Event<Entry>]) -> AvalonPanel {
        boards(events).into_iter().flatten().last().unwrap()
    }

    /// A game of the default table up to the vote on Player 1's team of
    /// Player 1 and Player 2.
    fn to_the_vote() -> Vec<Event<Entry>> {
        vec![
            start(five()),
            announce(1, view(1, 1, Phase::Propose, "Player 1", &[])),
            propose(1, "Player 1", &["Player 1", "Player 2"]),
            settled(Outcome::Proposed {
                seq: 1,
                team: Some(ids(&["Player 1", "Player 2"])),
            }),
            announce(
                2,
                view(1, 1, Phase::Vote, "Player 1", &["Player 1", "Player 2"]),
            ),
        ]
    }

    #[test]
    fn the_deal_sets_the_first_proposal_the_quest_track_and_the_seats() {
        let board = last(&[start(five())]);
        assert_eq!((board.quest, board.proposal), (1, 1));
        assert_eq!(board.phase, cue::AvalonPhase::Propose);
        assert_eq!(board.leader, "Player 1");
        assert_eq!((board.began, board.limit_ms), (0, 10_000));
        assert_eq!(board.team, None);
        assert!(board.proposals.is_empty() && board.deeds.is_empty());
        let names: Vec<&str> = board.seats.iter().map(|seat| seat.name.as_str()).collect();
        assert_eq!(names, players());
        assert_eq!(board.assassination, None);
    }

    #[test]
    fn an_announcement_sets_the_phase_its_leader_and_its_clock() {
        let events = [
            start(five()),
            announce(3, view(2, 4, Phase::Propose, "Player 4", &[])),
        ];
        let board = last(&events);
        assert_eq!((board.quest, board.proposal), (2, 4));
        assert_eq!(board.leader, "Player 4");
        assert_eq!((board.began, board.limit_ms), (3_000, 60_000));
    }

    #[test]
    fn a_phase_announced_again_and_a_choice_sent_late_change_nothing() {
        let events = [
            start(five()),
            announce(1, view(1, 1, Phase::Propose, "Player 1", &[])),
            announce(1, view(1, 1, Phase::Propose, "Player 1", &[])),
            propose(0, "Player 1", &["Player 1", "Player 2"]),
            logged(Entry::Failed {
                player: id("Player 1"),
                seq: 1,
                error: "down".to_string(),
            }),
        ];
        let boards = boards(&events);
        assert!(boards[0].is_some() && boards[1].is_some());
        assert_eq!(boards[2..], [None, None, None]);
    }

    #[test]
    fn a_proposed_team_is_on_the_table_as_its_proposal_is_sent() {
        let events = &to_the_vote()[..3];
        let board = last(events);
        assert_eq!(board.team, Some(ids(&["Player 1", "Player 2"])));
        let row = &board.proposals[0];
        assert_eq!((row.quest, row.proposal), (1, 1));
        assert_eq!(row.leader, "Player 1");
        assert_eq!(row.team, Some(ids(&["Player 1", "Player 2"])));
        assert_eq!(row.approved, None);
        assert!(row.ballots.iter().all(|(_, ballot)| ballot.is_none()));
        // The environment's word on the team changes nothing more.
        assert_eq!(boards(&to_the_vote()[..4])[3], None);
    }

    #[test]
    fn each_ballot_is_in_the_vote_as_it_is_cast() {
        let mut events = to_the_vote();
        let board = last(&events);
        assert_eq!(board.phase, cue::AvalonPhase::Vote);
        assert_eq!(board.team, Some(ids(&["Player 1", "Player 2"])));
        assert_eq!(board.proposals.len(), 1);
        events.push(vote(2, "Player 3", true));
        events.push(vote(2, "Player 4", false));
        let ballots = last(&events).proposals[0].ballots.clone();
        let expected: Vec<(PlayerId, Option<bool>)> = players()
            .into_iter()
            .map(|name| {
                let ballot = match name.as_str() {
                    "Player 3" => Some(true),
                    "Player 4" => Some(false),
                    _ => None,
                };
                (name, ballot)
            })
            .collect();
        assert_eq!(ballots, expected);
    }

    #[test]
    fn a_rejected_proposal_is_marked_and_the_next_one_opens_with_no_team() {
        let mut events = to_the_vote();
        events.push(tallied(2, false));
        assert_eq!(last(&events).proposals[0].approved, Some(false));
        events.push(announce(3, view(1, 2, Phase::Propose, "Player 2", &[])));
        let board = last(&events);
        assert_eq!(board.team, None);
        assert_eq!(board.proposals.len(), 1);
        events.push(propose(3, "Player 2", &["Player 2", "Player 3"]));
        let board = last(&events);
        assert_eq!(board.proposals.len(), 2);
        assert_eq!(board.proposals[0].leader, "Player 2");
        assert_eq!(board.proposals[0].proposal, 2);
    }

    #[test]
    fn an_approved_proposal_takes_its_quests_result_and_deeds_come_and_go() {
        let mut events = to_the_vote();
        events.push(tallied(2, true));
        events.push(announce(
            3,
            view(1, 1, Phase::Quest, "Player 1", &["Player 1", "Player 2"]),
        ));
        let board = last(&events);
        assert_eq!(
            board.deeds,
            [(id("Player 1"), None), (id("Player 2"), None)]
        );
        events.push(perform(3, "Player 2", Deed::Sabotage));
        events.push(perform(3, "Player 1", Deed::Support));
        let board = last(&events);
        assert_eq!(
            board.deeds,
            [
                (id("Player 1"), Some(cue::Deed::Support)),
                (id("Player 2"), Some(cue::Deed::Sabotage))
            ]
        );
        let result = QuestResult {
            team: ids(&["Player 1", "Player 2"]),
            sabotages: 1,
            succeeded: false,
        };
        events.push(settled(Outcome::Quested {
            seq: 3,
            result: result.clone(),
        }));
        let board = last(&events);
        assert_eq!(board.proposals[0].approved, Some(true));
        assert_eq!(board.proposals[0].result, Some(quest_result(&result)));
        assert_eq!(board.quests[0].result, Some(quest_result(&result)));
        assert!(board.quests[1..].iter().all(|token| token.result.is_none()));
        events.push(announce(4, view(2, 1, Phase::Propose, "Player 2", &[])));
        assert!(last(&events).deeds.is_empty());
    }

    #[test]
    fn a_leader_out_of_time_leaves_a_row_with_no_team_and_a_missing_ballot_is_none() {
        let events = [
            start(five()),
            announce(1, view(1, 1, Phase::Propose, "Player 1", &[])),
            settled(Outcome::Proposed { seq: 1, team: None }),
        ];
        let board = last(&events);
        assert_eq!(board.proposals.len(), 1);
        assert_eq!(board.proposals[0].team, None);
        assert_eq!(board.proposals[0].leader, "Player 1");
        let mut events = to_the_vote();
        events.push(vote(2, "Player 1", true));
        events.push(tallied(2, false));
        let ballots = &last(&events).proposals[0].ballots;
        assert_eq!(ballots[0], (id("Player 1"), Some(true)));
        assert!(ballots[1..].iter().all(|(_, ballot)| ballot.is_none()));
    }

    #[test]
    fn a_vote_announced_with_no_proposal_seen_opens_its_own_row() {
        let events = [
            start(five()),
            announce(
                2,
                view(1, 1, Phase::Vote, "Player 1", &["Player 1", "Player 3"]),
            ),
        ];
        let board = last(&events);
        assert_eq!(
            board.proposals[0].team,
            Some(ids(&["Player 1", "Player 3"]))
        );
    }

    #[test]
    fn every_token_is_as_big_as_its_quest_needs_at_every_table() {
        for players in 5..=10 {
            let roles = seated(&vec![Role::Servant; players]);
            let board = last(&[start(roles)]);
            for (quest, token) in (1..=5).zip(&board.quests) {
                let quest = NonZero::new(quest).unwrap();
                assert_eq!(token.size, team_size(players, quest));
                let needed = if quest.get() == 4 && players >= 7 {
                    2
                } else {
                    1
                };
                assert_eq!(token.sabotages_needed, needed, "{players} players, {quest}");
            }
        }
    }

    #[test]
    fn each_seat_sees_whom_its_role_sees() {
        let roles = seated(&[
            Role::Merlin,
            Role::Percival,
            Role::Servant,
            Role::Assassin,
            Role::Morgana,
            Role::Mordred,
            Role::Oberon,
            Role::Minion,
        ]);
        let board = last(&[start(roles)]);
        let sees = |seat: usize| -> Vec<&str> {
            board.seats[seat - 1]
                .sees
                .iter()
                .map(String::as_str)
                .collect()
        };
        assert_eq!(sees(1), ["Player 4", "Player 5", "Player 7", "Player 8"]);
        assert_eq!(sees(2), ["Player 1", "Player 5"]);
        assert!(sees(3).is_empty());
        assert_eq!(sees(4), ["Player 5", "Player 6", "Player 8"]);
        assert!(sees(7).is_empty());
    }

    #[test]
    fn the_assassination_names_its_target_as_it_is_sent_and_then_whether_it_was_merlin() {
        let mut events = vec![
            start(five()),
            announce(9, view(4, 1, Phase::Assassinate, "Player 4", &[])),
        ];
        let assassination = |events: &[Event<Entry>]| last(events).assassination.unwrap();
        assert_eq!(
            assassination(&events),
            cue::Assassination {
                assassin: id("Player 2"),
                target: None,
                merlin: None
            }
        );
        events.push(sent(Move::Select {
            seq: 9,
            from: id("Player 2"),
            n: 1,
            target: id("Player 3"),
        }));
        assert_eq!(assassination(&events).target, Some(id("Player 3")));
        events.push(settled(Outcome::Assassinated {
            seq: 9,
            target: Some(id("Player 3")),
            merlin: false,
        }));
        assert_eq!(assassination(&events).merlin, Some(false));
        events.push(settled(Outcome::End {
            winner: crate::avalon::Team::Good,
            by: crate::avalon::Victory::Quests,
            took: Duration::from_secs(9),
        }));
        assert_eq!(boards(&events).last().unwrap(), &None);
    }

    #[test]
    fn nothing_moves_a_board_the_deal_has_not_set() {
        let events = [
            announce(1, view(1, 1, Phase::Propose, "Player 1", &[])),
            propose(1, "Player 1", &["Player 1", "Player 2"]),
            tallied(1, true),
        ];
        assert_eq!(boards(&events), [None, None, None]);
    }
}
