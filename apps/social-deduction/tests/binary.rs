//! The binary names a game and its variant, plays one game, tells it on
//! standard output, and logs it to standard error as JSON lines.

use free_agent::Event;
use social_deduction::werewolf::{Entry, Role};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_social-deduction"))
        .args(args)
        .output()
        .unwrap()
}

/// Play Werewolf at random with `args` after the game and its variant.
fn play(args: &[&str]) -> Output {
    run(&[&["werewolf", "uniform-random"], args].concat())
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

/// What the command printed on standard output, which it must exit well.
fn printed(args: &[&str]) -> String {
    let output = run(args);
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// Assert that the command fails with usage, as `clap` does.
fn assert_usage_error(args: &[&str]) {
    let output = run(args);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Usage:"), "{stderr}");
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
        "2",
        "--doctors",
        "0",
        "--seers",
        "0",
    ]);

    let Some(Entry::Start { roles, .. }) = events.first().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    let count = |role| roles.values().filter(|r| **r == role).count();
    assert_eq!(roles.len(), 3);
    assert_eq!(count(Role::Werewolf), 1);
    assert_eq!(count(Role::Villager), 2);
}

#[test]
fn a_count_left_unset_is_filled_from_the_defaults() {
    let (events, _) = played(&["--werewolves", "1"]);

    let Some(Entry::Start { roles, .. }) = events.first().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    let count = |role| roles.values().filter(|r| **r == role).count();
    assert_eq!(roles.len(), 6);
    assert_eq!(count(Role::Werewolf), 1);
    assert_eq!(count(Role::Villager), 3);
    assert_eq!(count(Role::Doctor), 1);
    assert_eq!(count(Role::Seer), 1);
}

#[test]
fn a_count_that_is_not_a_number_is_refused() {
    let output = play(&["--werewolves", "many"]);
    assert!(!output.status.success());
}

#[test]
fn the_role_counts_belong_to_the_variant_not_the_top_level() {
    assert_usage_error(&["--werewolves", "2"]);
    assert_usage_error(&["werewolf", "--werewolves", "2"]);
}

#[test]
fn a_game_and_a_variant_must_be_named() {
    assert_usage_error(&[]);
    assert_usage_error(&["werewolf"]);
}

#[test]
fn help_lists_the_games_the_variants_and_the_role_counts() {
    assert!(printed(&["--help"]).contains("werewolf"));
    assert!(printed(&["werewolf", "--help"]).contains("uniform-random"));
    let help = printed(&["werewolf", "uniform-random", "--help"]);
    for count in ["--werewolves", "--villagers", "--doctors", "--seers"] {
        assert!(help.contains(count), "{help}");
    }
}
