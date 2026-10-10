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
    let help = printed(&["werewolf", "--help"]);
    assert!(
        help.contains("uniform-random") && help.contains("llm"),
        "{help}"
    );
    for variant in ["uniform-random", "llm"] {
        let help = printed(&["werewolf", variant, "--help"]);
        for count in ["--werewolves", "--villagers", "--doctors", "--seers"] {
            assert!(help.contains(count), "{help}");
        }
    }
}

/// An example configuration file, by its name in the examples directory.
fn example(name: &str) -> String {
    format!(
        "{}/examples/werewolf/llm/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn the_llm_variant_prints_its_settings_then_every_prompt_and_is_not_playable_yet() {
    let printed = printed(&["werewolf", "llm", "--config", &example("personas.toml")]);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines[0], "model: qwen2.5-7b-instruct at http://localhost:1234/v1",
        "{printed}"
    );
    // Every seat for every role, in order, each under a header.
    let headers: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("--- "))
        .collect();
    let expected: Vec<String> = (1..=7)
        .flat_map(|seat| {
            ["werewolf", "villager", "doctor", "seer"]
                .map(|role| format!("--- player{seat} as {role} ---"))
        })
        .collect();
    assert_eq!(headers, expected, "{printed}");
    assert!(
        printed.contains("You are player1, a werewolf.\n"),
        "{printed}"
    );
    assert!(printed.contains("You are player1, the seer."), "{printed}");
    assert_eq!(
        lines.last().unwrap(),
        &"The model-played game is not playable yet.",
        "{printed}"
    );
}

#[test]
fn a_template_that_cannot_render_is_an_error_before_anything_is_printed() {
    let path = format!("{}/broken-template.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(
        &path,
        "[model]\nbase_url = \"http://localhost:1234/v1\"\nid = \"m\"\n[prompt]\nsystem = \"{{ rulez }}\"\n",
    )
    .unwrap();
    let output = run(&["werewolf", "llm", "--config", &path]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for expected in ["player1", "werewolf", "rulez"] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn the_llm_variant_hears_the_command_line_over_the_file() {
    let printed = printed(&[
        "werewolf",
        "llm",
        "--config",
        &example("personas.toml"),
        "--werewolves",
        "1",
        "--day-limit",
        "2m",
    ]);
    assert!(
        printed.contains("roles: 1 werewolf, 3 villagers"),
        "{printed}"
    );
    assert!(printed.contains("day limit: 2m\n"), "{printed}");
}

#[test]
fn the_llm_variant_needs_a_configuration_file() {
    assert_usage_error(&["werewolf", "llm"]);
    assert_usage_error(&["werewolf", "llm", "--werewolves", "1"]);
}

#[test]
fn a_configuration_file_that_cannot_be_read_is_an_error_naming_it() {
    let output = run(&["werewolf", "llm", "--config", "no-such-file.toml"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no-such-file.toml"), "{stderr}");
}
