//! Every cue survives the trip to the frontend as JSON.

use serde::{Serialize, de::DeserializeOwned};
use twenty_questions_gui_types::*;
use uuid::Uuid;

fn round_trips<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<T>(&json).unwrap(), value, "{json}");
}

fn alice() -> PlayerId {
    "Alice".to_string()
}

fn every_beat() -> Vec<Beat> {
    let text = "Is it alive?".to_string();
    vec![
        Beat::Cast {
            seats: vec![
                Seat {
                    name: "Host".to_string(),
                    part: Part::Host,
                },
                Seat {
                    name: alice(),
                    part: Part::Player {
                        color: 0,
                        strategy: "everyone".to_string(),
                    },
                },
            ],
            guesses: 20,
            time_limit_ms: 600_000,
        },
        Beat::Secret {
            object: "a teapot".to_string(),
        },
        Beat::Clock { rate: 1.0 },
        Beat::Began,
        Beat::Told(vec!["Alice fails to play: no model".to_string()]),
        Beat::Said {
            who: alice(),
            text: "Let's ask.".to_string(),
        },
        Beat::Guessed {
            who: alice(),
            text: text.clone(),
            guess: 1,
        },
        Beat::Answered {
            guess: 1,
            text: "No.".to_string(),
            named: false,
        },
        Beat::Uncounted { who: alice(), text },
        Beat::Thinking { who: alice() },
        Beat::Thought { who: alice() },
        Beat::Over {
            outcome: Outcome::Won {
                by: Victory::OutOfTime,
                how: "The host wins.".to_string(),
            },
        },
        Beat::Over {
            outcome: Outcome::Stopped,
        },
        Beat::Over {
            outcome: Outcome::Failed("no model".to_string()),
        },
        Beat::Over {
            outcome: Outcome::Unfinished,
        },
    ]
}

#[test]
fn every_beat_round_trips() {
    for beat in every_beat() {
        round_trips(Cue { at: 1_234, beat });
    }
}

#[test]
fn an_opened_log_round_trips() {
    let cues = every_beat()
        .into_iter()
        .map(|beat| Cue { at: 5, beat })
        .collect();
    round_trips(Opened {
        name: "game.jsonl".to_string(),
        recordings: Recordings {
            games: vec![Recording {
                episode: Uuid::from_u128(1),
                cues,
            }],
            problems: vec!["the log is bad at line 3: EOF".to_string()],
        },
    });
}
