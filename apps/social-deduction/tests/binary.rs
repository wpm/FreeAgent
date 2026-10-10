//! The binary names a game and its variant, plays one game, tells it on
//! standard output, and logs it to standard error as JSON lines.

use free_agent::Event;
use social_deduction::werewolf::{Entry, PlayerId, Role};
use std::collections::HashMap;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_social-deduction"))
        .args(args)
        .output()
        .unwrap()
}

/// Run the command, which must exit well.
fn succeeded(args: &[&str]) -> Output {
    let output = run(args);
    assert!(output.status.success(), "{output:?}");
    output
}

/// What the command printed on standard output.
fn printed(args: &[&str]) -> String {
    String::from_utf8(succeeded(args).stdout).unwrap()
}

/// Play Werewolf at random with `args` after the game and its variant: the
/// events on standard error, parsed, and what was printed.
fn played(args: &[&str]) -> (Vec<Event<Entry>>, String) {
    let output = succeeded(&[&["werewolf", "uniform-random"], args].concat());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, String::from_utf8(output.stdout).unwrap())
}

/// The variation and the deal the log opens with.
fn start(events: &[Event<Entry>]) -> (&str, &HashMap<PlayerId, Role>) {
    let Some(Entry::Start { variation, roles }) = events.first().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    (variation, roles)
}

/// Assert that the command refuses its arguments, with the exit code `clap`
/// reserves for a usage error.
fn assert_usage_error(args: &[&str]) {
    let output = run(args);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn the_log_runs_from_the_deal_to_the_result_under_one_episode_id() {
    let (events, printed) = played(&[]);

    let episode = events[0].episode;
    assert!(events.iter().all(|event| event.episode == episode));
    let (variation, roles) = start(&events);
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

    let (_, roles) = start(&events);
    let count = |role| roles.values().filter(|r| **r == role).count();
    assert_eq!(roles.len(), 3);
    assert_eq!(count(Role::Werewolf), 1);
    assert_eq!(count(Role::Villager), 2);
}

#[test]
fn a_count_that_is_not_a_number_is_refused() {
    assert_usage_error(&["werewolf", "uniform-random", "--werewolves", "many"]);
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
