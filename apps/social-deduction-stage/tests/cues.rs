//! Every cue survives the trip to the frontend as JSON.

use serde::{Serialize, de::DeserializeOwned};
use social_deduction_stage::*;

fn round_trips<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<T>(&json).unwrap(), value, "{json}");
}

fn player(n: u8) -> PlayerId {
    format!("Player {n}")
}

fn werewolf_panel() -> WerewolfPanel {
    WerewolfPanel {
        round: 2,
        phase: WerewolfPhase::Day,
        began: 1_000,
        limit_ms: 60_000,
        ratify: Some(Ratify {
            target: Some(player(1)),
            since: 1_500,
            limit_ms: 10_000,
        }),
        closing: Some(2_000),
        seats: vec![
            SeatState {
                name: player(1),
                died: None,
            },
            SeatState {
                name: player(2),
                died: Some(Death {
                    round: 1,
                    phase: WerewolfPhase::Night,
                }),
            },
        ],
        votes: vec![(player(1), Some(player(3))), (player(3), None)],
        night: vec![
            (player(1), NightDeed::Pending),
            (player(3), NightDeed::Kill(player(2))),
            (player(4), NightDeed::Protect(player(1))),
            (player(5), NightDeed::Ask(player(3))),
        ],
        seers: vec![(
            player(5),
            vec![Learned {
                player: player(3),
                role: Role::Werewolf,
                night: 1,
            }],
        )],
        nights: vec![
            NightOutcome {
                night: 1,
                victim: Some(player(2)),
                protected_by: vec![],
                died: Some(player(2)),
            },
            NightOutcome {
                night: 2,
                victim: None,
                protected_by: vec![player(4)],
                died: None,
            },
        ],
    }
}

fn avalon_panel() -> AvalonPanel {
    let result = QuestResult {
        team: vec![player(1), player(2)],
        sabotages: 1,
        succeeded: false,
    };
    AvalonPanel {
        quest: 2,
        proposal: 1,
        phase: AvalonPhase::Quest,
        began: 2_000,
        limit_ms: 30_000,
        leader: player(1),
        team: Some(vec![player(1), player(3)]),
        deeds: vec![
            (player(1), Some(Deed::Support)),
            (player(3), Some(Deed::Sabotage)),
            (player(4), None),
        ],
        quests: vec![
            QuestToken {
                size: 2,
                sabotages_needed: 1,
                result: Some(result.clone()),
            },
            QuestToken {
                size: 3,
                sabotages_needed: 2,
                result: None,
            },
        ],
        proposals: vec![ProposalRow {
            quest: 1,
            proposal: 1,
            leader: player(2),
            team: None,
            ballots: vec![
                (player(1), Some(true)),
                (player(2), Some(false)),
                (player(3), None),
            ],
            approved: None,
            result: Some(result),
        }],
        seats: vec![AvalonSeat {
            name: player(1),
            sees: vec![player(2)],
        }],
        assassination: Some(Assassination {
            assassin: player(2),
            target: Some(player(1)),
            merlin: Some(true),
        }),
    }
}

#[test]
fn every_beat_round_trips() {
    let beats = [
        Beat::Cast {
            game: GameName::Werewolf,
            seats: vec![Seat {
                name: player(3),
                label: "Player 3 · seer".into(),
                role: Role::Seer,
                strategy: "Trust no one.".into(),
            }],
        },
        Beat::Cast {
            game: GameName::Avalon,
            seats: vec![],
        },
        Beat::Clock { rate: 1.0 },
        Beat::Heading("Night 2.".into()),
        Beat::Told(vec!["Player 2 dies.".into(), "It is day.".into()]),
        Beat::Said {
            who: player(1),
            text: "Not me.".into(),
        },
        Beat::Thinking { who: player(1) },
        Beat::Thought { who: player(1) },
        Beat::Panel(Panel::Werewolf(werewolf_panel())),
        Beat::Panel(Panel::Avalon(avalon_panel())),
        Beat::Over {
            outcome: Outcome::Won {
                side: Side::Evil,
                how: "The werewolves outnumber the villagers.".into(),
            },
        },
        Beat::Over {
            outcome: Outcome::Stopped,
        },
        Beat::Over {
            outcome: Outcome::Failed("The model is unreachable.".into()),
        },
        Beat::Over {
            outcome: Outcome::Unfinished,
        },
    ];
    for (at, beat) in beats.into_iter().enumerate() {
        round_trips(Cue {
            at: at as Millis,
            beat,
        });
    }
}

#[test]
fn the_serde_form_is_externally_tagged() {
    let cue = Cue {
        at: 7,
        beat: Beat::Thinking { who: player(1) },
    };
    assert_eq!(
        serde_json::to_string(&cue).unwrap(),
        r#"{"at":7,"beat":{"Thinking":{"who":"Player 1"}}}"#
    );
}

#[test]
fn roles_display_as_the_console_names_them() {
    let roles = [
        (Role::Villager, "villager"),
        (Role::Seer, "seer"),
        (Role::Doctor, "doctor"),
        (Role::Werewolf, "werewolf"),
        (Role::Merlin, "Merlin"),
        (Role::Percival, "Percival"),
        (Role::Servant, "Servant"),
        (Role::Assassin, "Assassin"),
        (Role::Morgana, "Morgana"),
        (Role::Mordred, "Mordred"),
        (Role::Oberon, "Oberon"),
        (Role::Minion, "Minion"),
    ];
    for (role, name) in roles {
        assert_eq!(role.to_string(), name);
        round_trips(role);
    }
}

#[test]
fn every_role_is_on_its_side_and_found_by_its_key() {
    for role in Role::ALL {
        let key = role.to_string().to_lowercase();
        assert_eq!(Role::from_key(&key), Some(role), "{key}");
    }
    let evil = [
        Role::Werewolf,
        Role::Assassin,
        Role::Morgana,
        Role::Mordred,
        Role::Oberon,
        Role::Minion,
    ];
    for role in Role::ALL {
        let side = if evil.contains(&role) {
            Side::Evil
        } else {
            Side::Good
        };
        assert_eq!(role.side(), side, "{role}");
    }
    assert_eq!(Role::from_key("jester"), None);
}
