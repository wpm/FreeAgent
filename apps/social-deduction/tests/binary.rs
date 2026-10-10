//! The binary plays one game, tells it on standard output, and logs it to
//! standard error as JSON lines.

use free_agent::Event;
use social_deduction::werewolf::Entry;
use std::process::{Command, Output};

fn play(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_social-deduction"))
        .args(args)
        .output()
        .unwrap()
}

/// The events on standard error, parsed, and what was printed.
fn played(args: &[&str]) -> (Vec<Event<Entry>>, String) {
    let output = play(args);
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, String::from_utf8(output.stdout).unwrap())
}

#[test]
fn the_log_runs_from_the_deal_to_the_result_under_one_episode_id() {
    let (events, printed) = played(&[]);

    let episode = events[0].episode;
    assert!(events.iter().all(|event| event.episode == episode));
    let Some(Entry::Start { variation, roles }) = events.first().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    assert_eq!(variation, "Uniform Random");
    // The default table seats seven.
    assert_eq!(roles.len(), 7);
    let Some(Entry::End { winner, .. }) = events.last().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    // The account opens with the table, goes through the first night, and
    // closes with the winner.
    let lines: Vec<&str> = printed.lines().collect();
    assert!(
        lines[0].starts_with("Uniform Random with 7 players: "),
        "{printed}"
    );
    assert_eq!(lines[1], "Night 1.", "{printed}");
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with(&format!("The {winner} win after ")),
        "{printed}"
    );
}

#[test]
fn the_table_is_dealt_as_asked() {
    let (events, _) = played(&[
        "--werewolves",
        "1",
        "--villagers",
        "1",
        "--doctors",
        "0",
        "--seers",
        "0",
    ]);

    let Some(Entry::Start { roles, .. }) = events.first().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    assert_eq!(roles.len(), 2);
}

#[test]
fn a_count_that_is_not_a_number_is_refused() {
    let output = play(&["--werewolves", "many"]);
    assert!(!output.status.success());
}
