//! The Werewolf board as the viewer draws it. Every choice moves it as the
//! player sends it, read from the player's action, until the phase begins
//! closing, read from the environment's first `Closing`. Each night's hunt
//! and each day's vote settle it.

use super::{Entry, Hunt, Message, Move, Outcome, Phase, Role, View, Werewolf};
use crate::game::Tagged;
use crate::report::in_order;
use crate::seat::PlayerId;
use crate::stage::{self, Staged, millis, millis_of, set};
use free_agent::Event;
use social_deduction_cues::{
    Death, Learned, Millis, NightDeed, NightOutcome, Panel, Ratify, SeatState, WerewolfPanel,
    WerewolfPhase,
};
use std::collections::HashMap;
use std::time::Duration;

/// The Werewolf board, from the deal on, shown from the first phase.
#[derive(Debug, Default)]
pub struct Board {
    /// Every player's role, from the deal.
    roles: HashMap<PlayerId, Role>,
    /// The latest phase announced, by its number: 0 before the first.
    seq: u64,
    /// The board, from the deal on.
    panel: Option<WerewolfPanel>,
    /// The board as last shown.
    shown: Option<WerewolfPanel>,
}

impl stage::Board<Werewolf> for Board {
    fn fold(&mut self, event: &Event<Entry>) -> Option<Panel> {
        self.apply(event);
        let panel = self.panel.clone().filter(|_| self.seq > 0)?;
        (self.shown.as_ref() != Some(&panel)).then(|| {
            self.shown = Some(panel.clone());
            Panel::Werewolf(panel)
        })
    }
}

impl Board {
    /// Move the board by `event`.
    fn apply(&mut self, event: &Event<Entry>) {
        let at = millis(event.time);
        match &event.payload {
            Entry::Start { roles, .. } => self.start(roles),
            Entry::Action {
                message:
                    Message::Announce {
                        seq, view, limit, ..
                    },
                ..
            } if *seq > self.seq => self.announce(*seq, view, *limit, at),
            Entry::Action {
                message: Message::Closing { seq, .. },
                ..
            } if *seq == self.seq => self.closing(at),
            Entry::Action {
                message: Message::Move(chosen),
                ..
            } if chosen.seq() == self.seq && !self.is_closing() => self.chosen(chosen, at),
            Entry::Event(Outcome::Hunted { hunt, .. }) => self.hunted(hunt),
            Entry::Event(Outcome::Voted { out: Some(out), .. }) => {
                self.died(out, WerewolfPhase::Day)
            }
            _ => {}
        }
    }

    /// The deal of `roles`: every seat alive and every seer knowing
    /// nothing yet, shown once the first phase is announced, which sets
    /// the round, the phase and its clock.
    fn start(&mut self, roles: &HashMap<PlayerId, Role>) {
        self.roles = roles.clone();
        let seats = in_order(roles.keys());
        self.panel = Some(WerewolfPanel {
            round: 1,
            phase: WerewolfPhase::Night,
            began: 0,
            limit_ms: 0,
            ratify: None,
            closing: None,
            seats: seats
                .iter()
                .map(|name| SeatState {
                    name: (*name).clone(),
                    died: None,
                })
                .collect(),
            votes: vec![],
            night: vec![],
            seers: seats
                .iter()
                .filter(|name| roles[**name] == Role::Seer)
                .map(|name| ((*name).clone(), vec![]))
                .collect(),
            nights: vec![],
        });
    }

    /// The phase numbered `seq` opens at `at` for `limit`, as `view` shows
    /// it, not closing: by day every living player undecided, and a night that elected
    /// nobody recorded with no victim; by night every living werewolf,
    /// doctor and seer still choosing.
    fn announce(&mut self, seq: u64, view: &View, limit: Duration, at: Millis) {
        self.seq = seq;
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        panel.round = view.round.get();
        panel.phase = phase(&view.phase);
        panel.began = at;
        panel.limit_ms = millis_of(limit);
        panel.ratify = None;
        panel.closing = None;
        let living = in_order(&view.alive);
        match view.phase {
            Phase::Day => {
                panel.night = vec![];
                panel.votes = living
                    .into_iter()
                    .map(|name| (name.clone(), None))
                    .collect();
                if !panel.nights.iter().any(|night| night.night == panel.round) {
                    panel.nights.push(NightOutcome {
                        night: panel.round,
                        victim: None,
                        protected_by: vec![],
                        died: None,
                    });
                }
            }
            Phase::Night => {
                panel.votes = vec![];
                panel.night = living
                    .into_iter()
                    .filter(|name| view.phase.wakes(self.roles.get(*name).copied()))
                    .map(|name| (name.clone(), NightDeed::Pending))
                    .collect();
            }
        }
    }

    /// The phase underway begins closing at `at`, unless it already has.
    fn closing(&mut self, at: Millis) {
        if let Some(panel) = self.panel.as_mut() {
            panel.closing.get_or_insert(at);
        }
    }

    /// Whether the phase underway is closing, when no choice moves it.
    fn is_closing(&self) -> bool {
        self.panel
            .as_ref()
            .is_some_and(|panel| panel.closing.is_some())
    }

    /// A player, or the environment, sends `chosen` at `at` in the phase
    /// underway.
    fn chosen(&mut self, chosen: &Move, at: Millis) {
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        match chosen {
            Move::Select { from, target, .. } => match panel.phase {
                WerewolfPhase::Day => set(&mut panel.votes, from, Some(target.clone())),
                WerewolfPhase::Night => {
                    let deed = match self.roles.get(from) {
                        Some(Role::Werewolf) => NightDeed::Kill(target.clone()),
                        Some(Role::Doctor) => NightDeed::Protect(target.clone()),
                        Some(Role::Seer) => {
                            learned(panel, from, target, &self.roles);
                            NightDeed::Ask(target.clone())
                        }
                        _ => return,
                    };
                    set(&mut panel.night, from, deed);
                }
            },
            Move::Retract { from, .. } => set(&mut panel.votes, from, None),
            Move::Tentative { target, ratify, .. } => {
                let since = match &panel.ratify {
                    Some(standing) if standing.target == *target => standing.since,
                    _ => at,
                };
                panel.ratify = Some(Ratify {
                    target: target.clone(),
                    since,
                    limit_ms: millis_of(*ratify),
                });
            }
            Move::Ratify { .. } => {}
        }
    }

    /// The night's hunt came out as `hunt` says.
    fn hunted(&mut self, hunt: &Hunt) {
        let died = hunt.dead().cloned();
        if let Some(dead) = &died {
            self.died(dead, WerewolfPhase::Night);
        }
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        panel.nights.push(NightOutcome {
            night: panel.round,
            victim: Some(hunt.victim.clone()),
            protected_by: in_order(&hunt.protectors).into_iter().cloned().collect(),
            died,
        });
    }

    /// `player` died in the round underway, by night or by day.
    fn died(&mut self, player: &PlayerId, phase: WerewolfPhase) {
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        let round = panel.round;
        if let Some(seat) = panel.seats.iter_mut().find(|seat| seat.name == *player) {
            seat.died = Some(Death { round, phase });
        }
    }
}

/// `seer` learns `target`'s role, which `roles` deals, tonight: a seer
/// that asks again the same night learns the second answer in place of
/// the first.
fn learned(
    panel: &mut WerewolfPanel,
    seer: &PlayerId,
    target: &PlayerId,
    roles: &HashMap<PlayerId, Role>,
) {
    let night = panel.round;
    if let Some(role) = roles.get(target)
        && let Some((_, learned)) = panel.seers.iter_mut().find(|(name, _)| name == seer)
    {
        learned.retain(|learned| learned.night != night);
        learned.push(Learned {
            player: target.clone(),
            role: <Werewolf as Staged>::role(*role),
            night,
        });
    }
}

/// `phase` as the board shows it.
fn phase(phase: &Phase) -> WerewolfPhase {
    match phase {
        Phase::Night => WerewolfPhase::Night,
        Phase::Day => WerewolfPhase::Day,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::tests::{announcement, dealt, logged, logged_at, seated};
    use crate::seat::{ENVIRONMENT, id, to_the_environment};
    use crate::stage::Board as _;
    use crate::strategy::RANDOM;
    use crate::werewolf::Rules;
    use crate::werewolf::tests::{closing, retraction, selection, tentative};
    use social_deduction_cues::{Beat, Role as Shown};
    use std::collections::HashSet;
    use std::num::NonZero;

    /// A table of six: wolves in seats 1 and 4, a doctor in 3, a seer in
    /// 5, and villagers in 2 and 6.
    fn six() -> HashMap<PlayerId, Role> {
        seated(&[
            Role::Werewolf,
            Role::Villager,
            Role::Doctor,
            Role::Werewolf,
            Role::Seer,
            Role::Villager,
        ])
    }

    /// The deal of `roles`, with a night of a minute and a day of two.
    fn start(roles: HashMap<PlayerId, Role>) -> Event<Entry> {
        let rules = Rules {
            day_limit: Duration::from_secs(120),
            ratify_limit: Duration::from_secs(20),
            ..Rules::default()
        };
        logged(Entry::start(&dealt(&roles, RANDOM), &rules))
    }

    /// The environment's announcement of round `round`'s `phase`, the
    /// phase numbered `seq`, with the players seated in `alive` alive,
    /// logged `seq` seconds in.
    fn announce(seq: u64, round: u8, phase: Phase, alive: &[u8]) -> Event<Entry> {
        let view = View {
            round: NonZero::new(round).unwrap(),
            phase,
            roles: HashMap::new(),
            alive: alive.iter().map(|seat| format!("Player {seat}")).collect(),
        };
        logged_at(
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

    /// `from`'s action sending `message`, logged `seconds` in.
    fn sent_at(seconds: u64, from: &str, message: Message) -> Event<Entry> {
        logged_at(
            seconds,
            Entry::Action {
                from: id(from),
                n: 1,
                to: to_the_environment(),
                message,
                seen: None,
                forgot: None,
            },
        )
    }

    /// `from`'s selection of `target` in the phase numbered `seq`.
    fn select(seq: u64, from: &str, target: &str) -> Event<Entry> {
        sent_at(0, from, selection(seq, from, 1, target))
    }

    /// `from`'s retraction in the phase numbered `seq`.
    fn retract(seq: u64, from: &str) -> Event<Entry> {
        sent_at(0, from, retraction(seq, from, 1))
    }

    /// The environment's tentative decision for `target` in the day
    /// numbered `seq`, `seconds` in.
    fn decide(seconds: u64, seq: u64, target: Option<&str>) -> Event<Entry> {
        sent_at(seconds, ENVIRONMENT, tentative(seq, 1, target))
    }

    /// The night numbered `seq` elected `victim`, whom `protectors`
    /// protected.
    fn hunted(seq: u64, victim: &str, protectors: &[&str]) -> Event<Entry> {
        logged(Entry::Event(Outcome::Hunted {
            seq,
            hunt: Hunt {
                victim: id(victim),
                protectors: protectors.iter().map(|name| id(name)).collect(),
            },
        }))
    }

    /// The day numbered `seq` voted `out`.
    fn voted(seq: u64, out: Option<&str>) -> Event<Entry> {
        logged(Entry::Event(Outcome::Voted {
            seq,
            out: out.map(id),
            by: super::super::Ending::Ratified,
        }))
    }

    /// The board after each of `events` in turn, `None` where it did not
    /// change, checking that the board folded out is the board held.
    fn boards(events: &[Event<Entry>]) -> Vec<Option<WerewolfPanel>> {
        let mut board = Board::default();
        events
            .iter()
            .map(|event| {
                let folded = board.fold(event);
                let changed = folded.as_ref().and(board.panel.clone());
                assert_eq!(folded, changed.clone().map(Panel::Werewolf));
                changed
            })
            .collect()
    }

    /// The board after the last of `events` that changed it.
    fn last(events: &[Event<Entry>]) -> WerewolfPanel {
        boards(events).into_iter().flatten().last().unwrap()
    }

    /// Every one of the six alive.
    const ALL: [u8; 6] = [1, 2, 3, 4, 5, 6];

    /// The six's first night, announced.
    fn first_night() -> Vec<Event<Entry>> {
        vec![start(six()), announce(1, 1, Phase::Night, &ALL)]
    }

    /// The six's first day, the first night having killed Player 2.
    fn first_day() -> Vec<Event<Entry>> {
        let mut events = first_night();
        events.push(hunted(1, "Player 2", &[]));
        events.push(announce(2, 1, Phase::Day, &[1, 3, 4, 5, 6]));
        events
    }

    /// The players named.
    fn ids(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| id(name)).collect()
    }

    #[test]
    fn the_board_shows_from_the_first_phase_with_its_clock_and_its_night_roles() {
        let boards = boards(&first_night());
        assert_eq!(boards[0], None);
        let board = boards[1].clone().unwrap();
        assert_eq!((board.round, board.phase), (1, WerewolfPhase::Night));
        assert_eq!((board.began, board.limit_ms), (1_000, 60_000));
        assert_eq!(board.ratify, None);
        let seats: Vec<&str> = board.seats.iter().map(|seat| seat.name.as_str()).collect();
        assert_eq!(
            seats,
            ids(&[
                "Player 1", "Player 2", "Player 3", "Player 4", "Player 5", "Player 6"
            ])
        );
        assert!(board.seats.iter().all(|seat| seat.died.is_none()));
        assert!(board.votes.is_empty());
        let pending = |name: &str| (id(name), NightDeed::Pending);
        assert_eq!(
            board.night,
            [
                pending("Player 1"),
                pending("Player 3"),
                pending("Player 4"),
                pending("Player 5")
            ]
        );
        assert_eq!(board.seers, [(id("Player 5"), vec![])]);
        assert!(board.nights.is_empty());
    }

    #[test]
    fn each_night_role_does_its_own_deed_as_it_chooses_and_the_seer_learns_a_role() {
        let mut events = first_night();
        events.push(select(1, "Player 1", "Player 2"));
        events.push(select(1, "Player 3", "Player 2"));
        events.push(select(1, "Player 5", "Player 9"));
        events.push(select(1, "Player 5", "Player 6"));
        events.push(select(1, "Player 5", "Player 4"));
        let board = last(&events);
        assert_eq!(
            board.night,
            [
                (id("Player 1"), NightDeed::Kill(id("Player 2"))),
                (id("Player 3"), NightDeed::Protect(id("Player 2"))),
                (id("Player 4"), NightDeed::Pending),
                (id("Player 5"), NightDeed::Ask(id("Player 4"))),
            ]
        );
        // Asked again in a night, the seer knows the latest answer, and
        // learns nothing of a player not dealt.
        let learned = Learned {
            player: id("Player 4"),
            role: Shown::Werewolf,
            night: 1,
        };
        assert_eq!(board.seers, [(id("Player 5"), vec![learned])]);
    }

    #[test]
    fn a_choice_from_another_phase_or_a_villager_by_night_and_a_repeated_announcement_change_nothing()
     {
        let mut events = first_night();
        events.push(select(0, "Player 1", "Player 2"));
        events.push(select(1, "Player 2", "Player 1"));
        events.push(select(1, "Player 9", "Player 1"));
        events.push(sent_at(
            0,
            ENVIRONMENT,
            Message::Move(Move::Ratify { seq: 1, change: 1 }),
        ));
        events.push(announce(1, 1, Phase::Night, &ALL));
        events.push(logged(Entry::Event(Outcome::Voted {
            seq: 1,
            out: None,
            by: super::super::Ending::DayLimit,
        })));
        assert_eq!(boards(&events)[2..], [None, None, None, None, None, None]);
    }

    #[test]
    fn by_day_each_vote_is_set_as_it_is_cast_and_a_retraction_resets_it_to_undecided() {
        let mut events = first_day();
        let board = last(&events);
        assert_eq!((board.round, board.phase), (1, WerewolfPhase::Day));
        assert_eq!(board.limit_ms, 60_000);
        assert!(board.night.is_empty());
        events.push(select(2, "Player 1", "Player 5"));
        events.push(select(2, "Player 3", "Player 4"));
        let votes = last(&events).votes;
        assert_eq!(
            votes,
            [
                (id("Player 1"), Some(id("Player 5"))),
                (id("Player 3"), Some(id("Player 4"))),
                (id("Player 4"), None),
                (id("Player 5"), None),
                (id("Player 6"), None),
            ]
        );
        events.push(retract(2, "Player 1"));
        assert_eq!(last(&events).votes[0], (id("Player 1"), None));
    }

    #[test]
    fn a_protected_victim_survives_and_its_night_records_the_protectors() {
        let events = [
            start(seated(&[
                Role::Werewolf,
                Role::Villager,
                Role::Doctor,
                Role::Doctor,
            ])),
            announce(1, 1, Phase::Night, &[1, 2, 3, 4]),
            hunted(1, "Player 2", &["Player 4", "Player 3"]),
        ];
        let board = last(&events);
        assert!(board.seats.iter().all(|seat| seat.died.is_none()));
        assert_eq!(
            board.nights,
            [NightOutcome {
                night: 1,
                victim: Some(id("Player 2")),
                protected_by: ids(&["Player 3", "Player 4"]),
                died: None,
            }]
        );
    }

    #[test]
    fn a_night_with_no_hunt_records_no_victim_as_the_day_begins() {
        let events = [
            start(six()),
            announce(1, 1, Phase::Night, &ALL),
            announce(2, 1, Phase::Day, &ALL),
        ];
        assert_eq!(
            last(&events).nights,
            [NightOutcome {
                night: 1,
                victim: None,
                protected_by: vec![],
                died: None,
            }]
        );
    }

    #[test]
    fn two_seers_keep_their_own_discoveries_and_a_seer_killed_the_night_it_asks_keeps_its_own() {
        let roles = seated(&[Role::Werewolf, Role::Seer, Role::Seer, Role::Villager]);
        let events = [
            start(roles),
            announce(1, 1, Phase::Night, &[1, 2, 3, 4]),
            select(1, "Player 2", "Player 1"),
            select(1, "Player 3", "Player 4"),
            select(1, "Player 1", "Player 2"),
            hunted(1, "Player 2", &[]),
        ];
        let board = last(&events);
        let learned = |player: &str, role| Learned {
            player: id(player),
            role,
            night: 1,
        };
        assert_eq!(
            board.seers,
            [
                (id("Player 2"), vec![learned("Player 1", Shown::Werewolf)]),
                (id("Player 3"), vec![learned("Player 4", Shown::Villager)]),
            ]
        );
        assert_eq!(
            board.seats[1].died,
            Some(Death {
                round: 1,
                phase: WerewolfPhase::Night
            })
        );
    }

    #[test]
    fn a_tentative_decision_sets_the_ratify_and_only_a_change_of_target_resets_its_start() {
        let mut events = first_day();
        events.push(decide(3, 2, Some("Player 4")));
        let ratify = |events: &[Event<Entry>]| last(events).ratify.unwrap();
        assert_eq!(
            ratify(&events),
            Ratify {
                target: Some(id("Player 4")),
                since: 3_000,
                limit_ms: 20_000,
            }
        );
        events.push(decide(5, 2, Some("Player 4")));
        assert_eq!(ratify(&events).since, 3_000);
        events.push(decide(7, 2, None));
        assert_eq!(ratify(&events).target, None);
        assert_eq!(ratify(&events).since, 7_000);
    }

    #[test]
    fn a_phase_change_clears_the_votes_the_night_deeds_and_the_ratify() {
        let mut events = first_night();
        events.push(select(1, "Player 1", "Player 2"));
        events.push(hunted(1, "Player 2", &[]));
        events.push(announce(2, 1, Phase::Day, &[1, 3, 4, 5, 6]));
        assert!(last(&events).night.is_empty());
        events.push(select(2, "Player 1", "Player 5"));
        events.push(decide(3, 2, Some("Player 5")));
        events.push(voted(2, Some("Player 5")));
        events.push(announce(3, 2, Phase::Night, &[1, 3, 4, 6]));
        let board = last(&events);
        assert!(board.votes.is_empty());
        assert_eq!(board.ratify, None);
        assert_eq!(
            board.night,
            [
                (id("Player 1"), NightDeed::Pending),
                (id("Player 3"), NightDeed::Pending),
                (id("Player 4"), NightDeed::Pending),
            ]
        );
    }

    #[test]
    fn the_seats_deaths_are_the_hunts_and_the_votes() {
        let mut events = first_day();
        events.push(voted(2, Some("Player 5")));
        events.push(announce(3, 2, Phase::Night, &[1, 3, 4, 6]));
        events.push(hunted(3, "Player 3", &[]));
        let board = last(&events);
        let died: Vec<(&str, Option<Death>)> = board
            .seats
            .iter()
            .map(|seat| (seat.name.as_str(), seat.died))
            .collect();
        let at = |round, phase| Some(Death { round, phase });
        assert_eq!(
            died,
            [
                ("Player 1", None),
                ("Player 2", at(1, WerewolfPhase::Night)),
                ("Player 3", at(2, WerewolfPhase::Night)),
                ("Player 4", None),
                ("Player 5", at(1, WerewolfPhase::Day)),
                ("Player 6", None),
            ]
        );
        let nights: Vec<(u8, Option<PlayerId>)> = board
            .nights
            .iter()
            .map(|night| (night.night, night.died.clone()))
            .collect();
        assert_eq!(
            nights,
            [(1, Some(id("Player 2"))), (2, Some(id("Player 3")))]
        );
    }

    /// The environment's closing of the phase numbered `seq`, `seconds`
    /// in.
    fn close(seconds: u64, seq: u64) -> Event<Entry> {
        sent_at(seconds, ENVIRONMENT, closing(seq, 1))
    }

    #[test]
    fn a_day_closes_at_its_first_closing_its_choices_change_nothing_and_the_night_clears_it() {
        let mut events = first_day();
        events.push(select(2, "Player 1", "Player 5"));
        events.push(decide(3, 2, Some("Player 5")));
        assert_eq!(last(&events).closing, None);
        events.push(close(23, 2));
        assert_eq!(last(&events).closing, Some(23_000));
        events.push(close(24, 2));
        events.push(select(2, "Player 3", "Player 4"));
        events.push(retract(2, "Player 1"));
        events.push(decide(25, 2, None));
        assert_eq!(
            boards(&events)[events.len() - 4..],
            [None, None, None, None]
        );
        events.push(voted(2, Some("Player 5")));
        assert_eq!(last(&events).closing, Some(23_000));
        events.push(announce(3, 2, Phase::Night, &[1, 3, 4, 6]));
        assert_eq!(last(&events).closing, None);
    }

    #[test]
    fn a_night_closes_at_its_first_closing_and_the_day_clears_it() {
        let mut events = first_night();
        events.push(select(1, "Player 1", "Player 2"));
        events.push(close(30, 1));
        assert_eq!(last(&events).closing, Some(30_000));
        events.push(select(1, "Player 3", "Player 2"));
        assert_eq!(boards(&events).last(), Some(&None));
        events.push(hunted(1, "Player 2", &[]));
        events.push(announce(2, 1, Phase::Day, &[1, 3, 4, 5, 6]));
        assert_eq!(last(&events).closing, None);
    }

    #[test]
    fn the_stage_tells_a_closing_day_in_the_chat_and_shows_it_on_the_board() {
        let mut events = first_day();
        events.push(close(23, 2));
        let mut stage = stage::Stage::<Werewolf>::default();
        let beats: Vec<Beat> = events
            .iter()
            .flat_map(|event| stage.fold(event))
            .map(|cue| cue.beat)
            .collect();
        let (told, panel) = (&beats[beats.len() - 2], &beats[beats.len() - 1]);
        assert_eq!(*told, Beat::Told(vec!["Day 1 is closing.".to_string()]));
        assert!(matches!(panel, Beat::Panel(Panel::Werewolf(board)) if board.closing.is_some()));
    }

    #[test]
    fn a_closing_of_another_phase_changes_nothing() {
        let mut events = first_day();
        events.push(close(5, 1));
        assert_eq!(boards(&events).last(), Some(&None));
    }

    #[test]
    fn nothing_moves_a_board_the_deal_has_not_set() {
        let events = [
            announce(1, 1, Phase::Night, &ALL),
            select(1, "Player 1", "Player 2"),
            hunted(1, "Player 2", &[]),
            voted(1, Some("Player 3")),
        ];
        assert_eq!(boards(&events), [None, None, None, None]);
    }
}
