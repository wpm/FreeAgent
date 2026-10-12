//! The binary names a game and its configuration file, plays one game,
//! tells it on standard output, and logs it to standard error as JSON
//! lines.

use free_agent::Event;
use social_deduction::avalon;
use social_deduction::model::ToolChoice;
use social_deduction::model::canned::{
    day_refusal, playing, playing_only, playing_within, refusal, refusing, refusing_days,
    refusing_required, retracting, serving, unreachable, wanting_a_key,
};
use social_deduction::model::window::{ContextWindow, Source};
use social_deduction::seat::PlayerId;
use social_deduction::werewolf::config::Configuration;
use social_deduction::werewolf::{Ending, Entry, Message, Role};
use std::collections::{HashMap, HashSet};
use std::process::{Command, Output};
use std::time::Duration;

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

/// Run the command, which must fail before printing anything; what it said
/// on standard error.
fn refused(args: &[&str]) -> String {
    let output = run(args);
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    String::from_utf8(output.stderr).unwrap()
}

/// Play Werewolf with `args` after the game: the events on standard error,
/// parsed, and what was printed.
fn played(args: &[&str]) -> (Vec<Event<Entry>>, String) {
    let output = succeeded(&[&["werewolf"], args].concat());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, String::from_utf8(output.stdout).unwrap())
}

/// The deal the log opens with: every player's role and strategy.
fn start(events: &[Event<Entry>]) -> (&HashMap<PlayerId, Role>, &HashMap<PlayerId, String>) {
    match events.first().map(|event| &event.payload) {
        Some(Entry::Start {
            roles, strategies, ..
        }) => (roles, strategies),
        first => panic!("{first:?}"),
    }
}

/// Assert that the command refuses its arguments, with the exit code `clap`
/// reserves for a usage error.
fn assert_usage_error(args: &[&str]) {
    let output = run(args);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

/// Play a game with `args` after the game's name, with the default table,
/// and check that it is logged from the deal to the result under one
/// episode id and told from the table to the winner, with `notes` told
/// after the limits. The events, for whatever else the game logs, and what
/// was printed.
fn assert_plays_the_default_table(args: &[&str], notes: &[&str]) -> (Vec<Event<Entry>>, String) {
    let (events, printed) = played(args);

    let episode = events[0].episode;
    assert!(events.iter().all(|event| event.episode == episode));
    let (roles, strategies) = start(&events);
    // The default table seats seven.
    assert_eq!(roles.len(), 7);
    assert_eq!(strategies.len(), 7);
    let Some(Entry::End { winner, .. }) = events.last().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    // The account opens with the table, a player to a line with its role
    // and strategy, then the limits and the notes, goes through the first
    // night, set off by a blank line, tells how long the last phase took,
    // and closes with the winner, how long the game took, and the
    // survivors, a player to a line.
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[0], "7 players:", "{printed}");
    for seat in &lines[1..8] {
        let player = seat.split_once(',').unwrap().0;
        let told = format!(
            "{player}, a {}, plays {}",
            roles[player], strategies[player]
        );
        assert_eq!(*seat, told, "{printed}");
    }
    assert!(
        lines[8].starts_with("A night waits ") && lines[8].contains(" and a day "),
        "{printed}"
    );
    let blank = 9 + notes.len();
    assert_eq!(lines[9..blank], *notes, "{printed}");
    assert_eq!(lines[blank], "", "{printed}");
    assert_eq!(lines[blank + 1], "Night 1.", "{printed}");
    let won = lines
        .iter()
        .position(|line| line.starts_with(&format!("The {winner} win after ")))
        .unwrap_or_else(|| panic!("{printed}"));
    let took = lines[won - 2];
    assert!(
        (took.starts_with("Night ") || took.starts_with("Day ")) && took.contains(" took "),
        "{printed}"
    );
    assert_eq!(lines[won - 1], "", "{printed}");
    assert!(lines[won + 1].starts_with("The game took "), "{printed}");
    assert_eq!(lines[won + 2], "Survivors:", "{printed}");
    assert!(!lines[won + 3..].is_empty(), "{printed}");
    for seat in &lines[won + 3..] {
        assert!(
            seat.starts_with("player") && seat.ends_with(')'),
            "{printed}"
        );
    }
    (events, printed)
}

/// The configuration the log of `events` has right after the deal.
fn configuration(events: &[Event<Entry>]) -> &Configuration {
    match events.get(1).map(|event| &event.payload) {
        Some(Entry::Configuration(configuration)) => configuration,
        second => panic!("{second:?}"),
    }
}

#[test]
fn with_no_file_every_player_is_random_and_the_game_is_logged_from_the_deal_to_the_result() {
    let (events, _) = assert_plays_the_default_table(&[], &[]);
    let (_, strategies) = start(&events);
    assert!(strategies.values().all(|strategy| strategy == "random"));
    // The configuration is logged, with no models.
    let configuration = configuration(&events);
    assert!(configuration.models.is_empty(), "{configuration:?}");
    assert!(configuration.prompts.is_empty(), "{configuration:?}");
}

#[test]
fn the_random_example_plays_the_game_of_no_file() {
    let (events, _) = assert_plays_the_default_table(&[&example("random.toml")], &[]);
    let (none, _) = played(&[]);
    assert_eq!(configuration(&events), configuration(&none));
}

#[test]
fn no_log_leaves_standard_error_empty_and_the_game_is_still_told() {
    let output = succeeded(&["werewolf", "--no-log"]);
    assert!(output.stderr.is_empty(), "{output:?}");
    let printed = String::from_utf8(output.stdout).unwrap();
    assert!(printed.starts_with("7 players:\n"), "{printed}");
    assert!(printed.contains("\nSurvivors:\n"), "{printed}");
    // Given after --no-log, --log puts the log back.
    let (events, _) = played(&["--no-log", "--log"]);
    assert!(
        matches!(events[0].payload, Entry::Start { .. }),
        "{events:?}"
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

    let (roles, _) = start(&events);
    let count = |role| roles.values().filter(|r| **r == role).count();
    assert_eq!(roles.len(), 3);
    assert_eq!(count(Role::Werewolf), 1);
    assert_eq!(count(Role::Villager), 2);
}

#[test]
fn werewolf_takes_the_limits() {
    let (events, _) = played(&["--night-limit", "5s", "--day-limit", "1m"]);
    let Some(Entry::Start {
        night_limit,
        day_limit,
        ratify_limit,
        ..
    }) = events.first().map(|event| &event.payload)
    else {
        panic!("{events:?}");
    };
    assert_eq!(*night_limit, Duration::from_secs(5));
    assert_eq!(*day_limit, Duration::from_secs(60));
    // The day is not ratified unless something says so.
    assert_eq!(*ratify_limit, Duration::ZERO);
    assert_usage_error(&["werewolf", "--night-limit", "soon"]);
}

#[test]
fn an_unratified_day_ends_once_everyone_has_voted_and_is_logged_as_such() {
    let (events, printed) = played(&[]);
    let endings = endings(&events);
    assert!(
        endings.iter().all(|by| *by == Ending::AllSelected),
        "{endings:?}"
    );
    // Which the narration says nothing of.
    assert!(!printed.contains("The votes stand."), "{printed}");
    assert!(!printed.contains("The day runs out."), "{printed}");
    assert!(!printed.contains("a vote stands after"), "{printed}");
}

#[test]
fn a_count_that_is_not_a_number_is_refused() {
    assert_usage_error(&["werewolf", "--werewolves", "many"]);
}

#[test]
fn the_role_counts_belong_to_the_game_not_the_top_level() {
    assert_usage_error(&["--werewolves", "2"]);
}

#[test]
fn a_game_must_be_named() {
    assert_usage_error(&[]);
}

#[test]
fn the_variants_and_the_model_options_are_gone() {
    // A variant's name is read as a configuration file's.
    for variant in ["uniform-random", "llm"] {
        let stderr = refused(&["werewolf", variant]);
        assert!(stderr.contains(&format!("reading {variant}")), "{stderr}");
    }
    assert_usage_error(&["werewolf", "llm", "game.toml"]);
    for option in ["--model-base-url", "--model-id", "--context-window"] {
        assert_usage_error(&["werewolf", option, "1"]);
    }
}

#[test]
fn help_lists_the_games_the_file_the_role_counts_and_the_limits() {
    let help = printed(&["--help"]);
    assert!(
        help.contains("werewolf") && help.contains("avalon") && help.contains("models"),
        "{help}"
    );
    let help = printed(&["werewolf", "--help"]);
    let options = [
        "[CONFIG]",
        "--werewolves",
        "--villagers",
        "--doctors",
        "--seers",
        "--night-limit",
        "--day-limit",
        "--ratify-limit",
        "--log",
        "--no-log",
    ];
    for option in options {
        assert!(help.contains(option), "{help}");
    }
}

/// The model the tests' configuration files name.
const EXAMPLE_MODEL: &str = "qwen2.5-7b-instruct";

/// An example configuration file, by its name in the examples directory.
fn example(name: &str) -> String {
    format!("{}/examples/werewolf/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A configuration file written for this test: the prompted example with
/// its provider replaced by one at `base_url`, its model by `id`, each on
/// the line of the `[models.qwen]` table that names it and nowhere else,
/// and `lines` added to that table, in the target directory, at a path of
/// its own so the tests do not collide.
fn configured(test: &str, base_url: &str, id: &str, lines: &[&str]) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    let example = std::fs::read_to_string(example("prompted.toml")).unwrap();
    let replacements = [
        ("base_url", format!("base_url = \"{base_url}\"")),
        ("id", format!("id = \"{id}\"")),
    ];
    let mut table = "";
    let mut file = String::new();
    for line in example.lines() {
        if line.starts_with('[') {
            table = line;
        }
        let key = line.split_once(" = ").map(|(key, _)| key);
        let replacement = replacements
            .iter()
            .find(|(named, _)| table == "[models.qwen]" && key == Some(named));
        file.push_str(replacement.map_or(line, |(_, written)| written));
        file.push('\n');
        if key == Some("id") && table == "[models.qwen]" {
            for line in lines {
                file.push_str(line);
                file.push('\n');
            }
        }
    }
    for (_, line) in &replacements {
        assert!(
            file.lines().any(|written| written == line),
            "the example has no line of its [models.qwen] table to write {line} on"
        );
    }
    std::fs::write(&path, file).unwrap();
    path
}

/// How each day in `events` ended, in order.
fn endings(events: &[Event<Entry>]) -> Vec<Ending> {
    let endings: Vec<Ending> = events
        .iter()
        .filter_map(|event| match &event.payload {
            Entry::Voted { by, .. } => Some(*by),
            _ => None,
        })
        .collect();
    assert!(!endings.is_empty(), "{events:?}");
    endings
}

/// What the account says of the model's context window when nothing gives
/// one.
const DEFAULT_WINDOW: &str = "qwen's context window is 4096 tokens, the default.";

/// Short limits, so that a phase nobody selects in ends soon, and a day's
/// votes stand sooner than the day runs out.
const QUICK: [&str; 6] = [
    "--night-limit",
    "1s",
    "--day-limit",
    "2s",
    "--ratify-limit",
    "1s",
];

#[test]
fn the_context_window_the_provider_reports_is_used_unless_the_file_gives_one() {
    let base_url = playing_within(&[EXAMPLE_MODEL], 32768);
    let config = configured("window", &base_url, EXAMPLE_MODEL, &[]);
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let told = "qwen's context window is 32768 tokens, as the provider reports.";
    let (events, _) = assert_plays_the_default_table(&args, &[told]);
    assert_eq!(
        configuration(&events).models["qwen"].context_window,
        ContextWindow {
            tokens: 32768,
            source: Source::Provider,
        }
    );
    let config = configured(
        "window-file",
        &base_url,
        EXAMPLE_MODEL,
        &["context_window = 8192"],
    );
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let told = "qwen's context window is 8192 tokens, as the configuration file says.";
    let (events, _) = assert_plays_the_default_table(&args, &[told]);
    assert_eq!(
        configuration(&events).models["qwen"].context_window,
        ContextWindow {
            tokens: 8192,
            source: Source::File,
        }
    );
}

/// A context window just large enough for the default table: a day's
/// request leaves room for little of the story.
const SMALL_WINDOW: &str = "1900";

#[test]
fn in_a_small_context_window_the_game_plays_to_the_end() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let window = format!("context_window = {SMALL_WINDOW}");
    let config = configured("forgetting", &base_url, EXAMPLE_MODEL, &[&window]);
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let told =
        format!("qwen's context window is {SMALL_WINDOW} tokens, as the configuration file says.");
    let (events, _) = assert_plays_the_default_table(&args, &[&told]);
    // A player's first request, by night, tells the whole story, and a
    // request logs `forgot` only when it forgot talk. Whether a request
    // forgets depends on how long the deal lets the game run; the
    // in-process test of a small window checks that it does.
    let forgot: Vec<Option<usize>> = events
        .iter()
        .filter_map(|event| match &event.payload {
            Entry::Action { from, forgot, .. } if from != "environment" => Some(*forgot),
            _ => None,
        })
        .collect();
    assert_eq!(forgot[0], None);
    assert!(!forgot.contains(&Some(0)), "{forgot:?}");
}

#[test]
fn a_context_window_with_no_room_for_a_story_is_refused_before_anything_is_printed() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured(
        "window-small",
        &base_url,
        EXAMPLE_MODEL,
        &["context_window = 1500"],
    );
    let stderr = refused(&["werewolf", &config]);
    assert!(
        stderr.contains("[models.qwen] context_window is 1500 tokens, smaller than the"),
        "{stderr}"
    );
}

#[test]
fn a_context_window_of_0_is_refused_before_anything_is_printed() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured(
        "window-0",
        &base_url,
        EXAMPLE_MODEL,
        &["context_window = 0"],
    );
    let stderr = refused(&["werewolf", &config]);
    assert!(
        stderr
            .contains("[models.qwen] context_window is 0; a context window holds at least 1 token"),
        "{stderr}"
    );
}

#[test]
fn a_game_of_model_players_is_logged_from_the_configuration_to_the_result() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("plays", &base_url, EXAMPLE_MODEL, &[]);
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let (events, printed) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW]);
    let (roles, strategies) = start(&events);
    assert!(strategies.values().all(|strategy| strategy == "everyone"));
    let Entry::Start { ratify_limit, .. } = &events[0].payload else {
        panic!("{events:?}");
    };
    assert_eq!(*ratify_limit, Duration::from_secs(1));
    let configuration = configuration(&events);
    let model = &configuration.models["qwen"];
    assert_eq!(model.base_url, base_url);
    assert_eq!(model.id, EXAMPLE_MODEL);
    assert_eq!(model.tool_choice, ToolChoice::Required);
    assert_eq!(configuration.night_limit, Duration::from_secs(1));
    assert_eq!(configuration.ratify_limit, Duration::from_secs(1));
    // Everyone votes at once, so each day's votes stand after the ratify
    // limit and the selected player is voted out.
    let endings = endings(&events);
    assert!(
        endings.iter().all(|by| *by == Ending::Ratified),
        "{endings:?}"
    );
    // By day every living player speaks once, to every other living player
    // and not to the environment, and the account tells what they say. The
    // day's phases are the even-numbered ones, and each living player is
    // announced each of them.
    let said: Vec<(&PlayerId, &HashSet<PlayerId>)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            Entry::Action {
                from,
                to,
                message: Message::Say { .. },
                ..
            } => Some((from, to)),
            _ => None,
        })
        .collect();
    let announced_by_day = events
        .iter()
        .filter(|event| {
            matches!(
                &event.payload,
                Entry::Action {
                    message: Message::Announce { seq, .. },
                    ..
                } if seq % 2 == 0
            )
        })
        .count();
    assert!(!said.is_empty(), "{events:?}");
    assert_eq!(said.len(), announced_by_day, "{events:?}");
    for (from, to) in said {
        assert!(!to.contains(from), "{from} spoke to itself");
        assert!(
            !to.contains("environment"),
            "{from} spoke to the environment"
        );
    }
    assert!(
        printed.contains(" says, \"I have nothing to add.\""),
        "{printed}"
    );
    // Each player was told the prompt for the role it was dealt.
    assert_eq!(configuration.prompts.len(), 7);
    for (player, role) in roles {
        let prompt = &configuration.prompts[player];
        let told = format!("You are {player}, a {role}.");
        assert!(prompt.contains(&told), "{player} as {role}: {prompt}");
    }
    // The configuration is logged once.
    let configurations = events
        .iter()
        .filter(|event| matches!(event.payload, Entry::Configuration(_)))
        .count();
    assert_eq!(configurations, 1);
}

#[test]
fn a_table_of_random_werewolves_and_prompted_villagers_plays_to_the_end() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let path = format!("{}/mixed.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(
        &path,
        format!(
            "[models.qwen]
base_url = \"{base_url}\"
id = \"{EXAMPLE_MODEL}\"
[strategies.plain]
policy = \"prompted\"
system = \"You are {{{{ name }}}}, a {{{{ role }}}}.\"
[roles.werewolf]
strategy = \"random\"
[roles.villager]
strategy = \"plain\"
[roles.doctor]
strategy = \"plain\"
[roles.seer]
strategy = \"plain\"
"
        ),
    )
    .unwrap();
    let args = [&[path.as_str()], &QUICK[..]].concat();
    let (events, _) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW]);
    let (roles, strategies) = start(&events);
    for (player, role) in roles {
        let strategy = match role {
            Role::Werewolf => "random",
            _ => "plain",
        };
        assert_eq!(strategies[player], strategy, "{player}");
    }
    // Only the prompted players have prompts.
    let configuration = configuration(&events);
    assert_eq!(configuration.prompts.len(), 5);
}

#[test]
fn a_model_player_retracts_its_vote_and_everyone_hears_it() {
    let base_url = retracting(&[EXAMPLE_MODEL]);
    let config = configured("retracts", &base_url, EXAMPLE_MODEL, &[]);
    // Each change of the tentative decision early in a day restarts the
    // ratify timer, and on a slow machine the model calls behind them take
    // a while, so the day limit leaves the votes ample time to stand.
    let args = [
        config.as_str(),
        "--night-limit",
        "1s",
        "--day-limit",
        "10s",
        "--ratify-limit",
        "1s",
    ];
    let (events, printed) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW]);
    // The first seat alive retracts its vote each day, which leaves everyone
    // else's votes against it standing.
    let endings = endings(&events);
    assert!(
        endings.iter().all(|by| *by == Ending::Ratified),
        "{endings:?}"
    );
    let retracted: Vec<(&PlayerId, u64, &HashSet<PlayerId>)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            Entry::Action {
                from,
                n,
                to,
                message: Message::Retract { .. },
                ..
            } => Some((from, *n, to)),
            _ => None,
        })
        .collect();
    assert!(!retracted.is_empty(), "{events:?}");
    // Each retraction goes to the environment and the other players, and
    // each of them logs observing it.
    for (from, n, to) in retracted {
        assert!(to.contains("environment"), "{to:?}");
        assert!(to.len() > 1, "{to:?}");
        assert!(!to.contains(from), "{from} retracted to itself");
        for receiver in to {
            let observed = events.iter().any(|event| {
                matches!(
                    &event.payload,
                    Entry::Observation { by, from: sender, n: number, .. }
                        if by == receiver && sender == from && *number == n
                )
            });
            assert!(observed, "{receiver} did not observe {from}'s {n}");
        }
    }
    assert!(
        printed.contains(" withdraws their vote and is undecided again."),
        "{printed}"
    );
}

#[test]
fn a_model_that_refuses_to_be_made_to_call_a_tool_plays_when_asked_to_call_one() {
    let base_url = refusing_required(&[EXAMPLE_MODEL]);
    let config = configured("auto", &base_url, EXAMPLE_MODEL, &[]);
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let note = "qwen does not accept a required tool call, so its players are asked to call one.";
    let (events, _) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW, note]);
    assert_eq!(
        configuration(&events).models["qwen"].tool_choice,
        ToolChoice::Auto
    );
    // Every player played: none of them failed.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.payload, Entry::Failed { .. })),
        "{events:?}"
    );
}

#[test]
fn a_template_that_cannot_render_is_an_error_before_anything_is_printed() {
    let path = format!("{}/broken-template.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(
        &path,
        "strategy = \"plain\"\n[models.m]\nbase_url = \"http://localhost:1234/v1\"\nid = \"m\"\n\
         [strategies.plain]\npolicy = \"prompted\"\nsystem = \"{{ rulez }}\"\n",
    )
    .unwrap();
    let stderr = refused(&["werewolf", &path]);
    for expected in ["plain", "player1", "werewolf", "rulez"] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn werewolf_hears_the_command_line_over_the_file() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("hears", &base_url, EXAMPLE_MODEL, &[]);
    let (events, _) = played(&[&[config.as_str(), "--werewolves", "1"], &QUICK[..]].concat());
    let (roles, _) = start(&events);
    assert_eq!(roles.len(), 6);
    let configuration = configuration(&events);
    assert_eq!(configuration.role_counts[&Role::Werewolf], 1);
    assert_eq!(configuration.role_counts[&Role::Villager], 3);
    assert_eq!(configuration.day_limit, Duration::from_secs(2));
    assert_eq!(configuration.ratify_limit, Duration::from_secs(1));
}

#[test]
fn a_file_without_a_ratify_limit_plays_days_that_are_not_ratified() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("unratified", &base_url, EXAMPLE_MODEL, &[]);
    let file = std::fs::read_to_string(&config).unwrap();
    let unratified: String = file
        .lines()
        .filter(|line| !line.starts_with("ratify_limit"))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(unratified, file);
    std::fs::write(&config, unratified).unwrap();
    let args = [&config, "--night-limit", "1s", "--day-limit", "2s"];
    let (events, printed) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW]);
    let Entry::Start { ratify_limit, .. } = &events[0].payload else {
        panic!("{events:?}");
    };
    assert_eq!(*ratify_limit, Duration::ZERO);
    assert_eq!(configuration(&events).ratify_limit, Duration::ZERO);
    let endings = endings(&events);
    assert!(
        endings.iter().all(|by| *by == Ending::AllSelected),
        "{endings:?}"
    );
    assert!(!printed.contains("a vote stands after"), "{printed}");
    assert!(!printed.contains("The votes stand."), "{printed}");
}

#[test]
fn a_ratify_limit_longer_than_the_day_limit_is_refused_before_anything_is_printed() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("ratify", &base_url, EXAMPLE_MODEL, &[]);
    let stderr = refused(&["werewolf", &config, "--day-limit", "10s"]);
    assert!(
        stderr.contains("the ratify limit (20s) is longer than the day limit (10s)"),
        "{stderr}"
    );
}

#[test]
fn a_configuration_file_that_cannot_be_read_is_an_error_naming_it() {
    let stderr = refused(&["werewolf", "no-such-file.toml"]);
    assert!(stderr.contains("no-such-file.toml"), "{stderr}");
}

#[test]
fn a_model_that_does_not_play_when_asked_is_refused_in_the_provider_s_words() {
    // A provider refusing the night's request, and one that plays by night
    // alone and refuses the day's.
    let refusals = [
        (
            "unselecting",
            refusing(&[EXAMPLE_MODEL]),
            "night",
            refusal(EXAMPLE_MODEL),
        ),
        (
            "nights-only",
            refusing_days(&[EXAMPLE_MODEL]),
            "day",
            day_refusal(EXAMPLE_MODEL),
        ),
    ];
    for (name, base_url, phase, said) in refusals {
        let config = configured(name, &base_url, EXAMPLE_MODEL, &[]);
        let stderr = refused(&["werewolf", &config]);
        assert!(
            stderr.starts_with("Error: setting up the model qwen"),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!(
                "{EXAMPLE_MODEL} did not answer a {phase}'s request"
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!(
                "POST {base_url}/chat/completions was answered 400"
            )),
            "{stderr}"
        );
        assert!(stderr.contains(&said), "{stderr}");
    }
}

#[test]
fn a_model_its_provider_does_not_serve_is_refused() {
    let base_url = serving(&["llama-3.1-8b-instruct"]);
    let config = configured("unserved", &base_url, EXAMPLE_MODEL, &[]);
    let stderr = refused(&["werewolf", &config]);
    assert!(stderr.contains("the model qwen"), "{stderr}");
    assert!(stderr.contains(&base_url), "{stderr}");
    assert!(stderr.contains(EXAMPLE_MODEL), "{stderr}");
}

/// A configuration file written for this test: the prompted example with
/// its model `qwen` at `qwen_url` serving `qwen_id`, as [`configured`]
/// writes it, and a second model, `claude`, at `claude_url` serving
/// `claude_id`, which the seers play through a strategy of their own.
fn two_models(
    test: &str,
    qwen_url: &str,
    qwen_id: &str,
    claude_url: &str,
    claude_id: &str,
) -> String {
    let config = configured(test, qwen_url, qwen_id, &[]);
    let file = std::fs::read_to_string(&config).unwrap();
    let file = format!(
        "{file}
[models.claude]
base_url = \"{claude_url}\"
id = \"{claude_id}\"
[strategies.smart]
policy = \"prompted\"
model = \"claude\"
system = \"You are {{{{ name }}}}, a {{{{ role }}}}.\"
[roles.seer]
strategy = \"smart\"
"
    )
    .replace(
        "policy = \"prompted\"\nsystem",
        "policy = \"prompted\"\nmodel = \"qwen\"\nsystem",
    );
    std::fs::write(&config, file).unwrap();
    config
}

#[test]
fn a_table_of_two_models_plays_to_the_end() {
    let qwen_url = playing(&[EXAMPLE_MODEL]);
    let claude_url = refusing_required(&["claude-sonnet-4-5"]);
    let config = two_models(
        "two",
        &qwen_url,
        EXAMPLE_MODEL,
        &claude_url,
        "claude-sonnet-4-5",
    );
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let notes = [
        "claude's context window is 4096 tokens, the default.",
        "claude does not accept a required tool call, so its players are asked to call one.",
        DEFAULT_WINDOW,
    ];
    let (events, _) = assert_plays_the_default_table(&args, &notes);
    let (roles, strategies) = start(&events);
    for (player, role) in roles {
        let strategy = match role {
            Role::Seer => "smart",
            _ => "everyone",
        };
        assert_eq!(strategies[player], strategy, "{player}");
    }
    let models = &configuration(&events).models;
    assert_eq!(models.keys().collect::<Vec<_>>(), ["claude", "qwen"]);
    assert_eq!(models["claude"].tool_choice, ToolChoice::Auto);
    assert_eq!(models["qwen"].tool_choice, ToolChoice::Required);
}

#[test]
fn the_second_model_failing_its_probe_is_refused_naming_it_and_nothing_is_played() {
    let qwen_url = refusing(&[EXAMPLE_MODEL]);
    let claude_url = playing(&["claude-sonnet-4-5"]);
    let config = two_models(
        "second-refused",
        &qwen_url,
        EXAMPLE_MODEL,
        &claude_url,
        "claude-sonnet-4-5",
    );
    let stderr = refused(&["werewolf", &config]);
    assert!(
        stderr.starts_with("Error: setting up the model qwen"),
        "{stderr}"
    );
}

#[test]
fn the_models_command_lists_the_provider_s_models_sorted_and_plain() {
    let base_url = serving(&["zephyr-7b", EXAMPLE_MODEL, "llama-3.1-8b-instruct"]);
    let output = succeeded(&["models", &base_url]);
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "llama-3.1-8b-instruct\nqwen2.5-7b-instruct\nzephyr-7b\n"
    );
}

/// The legend the `--check` listing ends with.
const LEGEND: &str = "* marks a model that played when asked, ~ one that played when asked to call \
                      a tool but not made to, and ! one that did not.";

#[test]
fn the_models_command_with_check_asks_each_model_to_play_and_marks_its_answer() {
    let base_url = playing_only(&["zephyr-7b", EXAMPLE_MODEL], &[EXAMPLE_MODEL]);
    let output = succeeded(&["models", &base_url, "--check"]);
    let printed = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines,
        ["* qwen2.5-7b-instruct", "! zephyr-7b", "", LEGEND],
        "{printed}"
    );
    // The reason, on standard error: a line naming the model as the
    // listing marks it, then the probe's words, a cause per indented line.
    let stderr = String::from_utf8(output.stderr).unwrap();
    let reasons: Vec<&str> = stderr.lines().collect();
    // Refused as bad when made to call a tool, it was asked again to call
    // one, and refused again.
    let refused = format!(
        "POST {base_url}/chat/completions was answered 400 Bad Request. \
         The provider said: {}",
        refusal("zephyr-7b")
    );
    assert_eq!(
        reasons,
        [
            "! zephyr-7b".to_string(),
            "  zephyr-7b did not answer a night's request".to_string(),
            format!("  made to call a tool: {refused}"),
            format!("  asked to call one: {refused}"),
        ],
        "{stderr}"
    );
}

#[test]
fn the_models_command_with_check_marks_a_model_that_plays_by_night_alone_as_not_playing() {
    let base_url = refusing_days(&[EXAMPLE_MODEL]);
    let output = succeeded(&["models", &base_url, "--check"]);
    let printed = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines, ["! qwen2.5-7b-instruct", "", LEGEND], "{printed}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let reasons: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        reasons,
        [
            "! qwen2.5-7b-instruct",
            "  qwen2.5-7b-instruct did not answer a day's request",
            &format!(
                "  POST {base_url}/chat/completions was answered 400 Bad Request. \
                 The provider said: {}",
                day_refusal(EXAMPLE_MODEL)
            ),
        ],
        "{stderr}"
    );
}

#[test]
fn the_models_command_with_check_stops_at_a_model_that_could_not_be_asked() {
    // A provider that wants a key answers nothing about any model.
    let base_url = wanting_a_key(&["zephyr-7b", EXAMPLE_MODEL]);
    let stderr = refused(&["models", &base_url, "--check"]);
    assert!(
        stderr.starts_with("Error: qwen2.5-7b-instruct could not be asked to play"),
        "{stderr}"
    );
    assert!(stderr.contains("401 Unauthorized"), "{stderr}");
    assert!(!stderr.contains("zephyr-7b"), "{stderr}");
}

#[test]
fn the_models_command_with_ids_to_check_asks_only_those_and_needs_each_to_be_listed() {
    let base_url = refusing(&["zephyr-7b", EXAMPLE_MODEL, "llama-3.1-8b-instruct"]);
    let output = succeeded(&["models", &base_url, "--check", EXAMPLE_MODEL]);
    let printed = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines,
        [
            "  llama-3.1-8b-instruct",
            "! qwen2.5-7b-instruct",
            "  zephyr-7b",
            "",
            LEGEND,
        ],
        "{printed}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with(&format!("! {EXAMPLE_MODEL}\n  ")),
        "{stderr}"
    );
    assert!(stderr.contains(&refusal(EXAMPLE_MODEL)), "{stderr}");
    // The ids end with -- when the base URL follows them.
    let output = succeeded(&["models", "--check", EXAMPLE_MODEL, "--", &base_url]);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), printed);
    let stderr = refused(&["models", &base_url, "--check", EXAMPLE_MODEL, "gpt-0"]);
    assert!(stderr.contains("gpt-0"), "{stderr}");
    assert!(stderr.contains(&base_url), "{stderr}");
}

#[test]
fn the_models_command_needs_a_base_url_and_names_a_provider_it_cannot_reach() {
    assert_usage_error(&["models"]);
    let help = printed(&["models", "--help"]);
    assert!(help.contains("BASE_URL"), "{help}");
    assert!(help.contains("--api-key-env"), "{help}");
    assert!(help.contains("https://api.openai.com"), "{help}");
    assert!(help.contains("OPENAI_API_KEY"), "{help}");
    let base_url = unreachable();
    let stderr = refused(&["models", &base_url]);
    assert!(stderr.contains(&base_url), "{stderr}");
}

#[test]
fn the_models_command_refuses_a_key_variable_that_is_not_set_before_asking_anyone() {
    let base_url = unreachable();
    let name = "SOCIAL_DEDUCTION_NO_SUCH_KEY";
    let stderr = refused(&["models", &base_url, "--api-key-env", name]);
    assert!(stderr.contains(name), "{stderr}");
    assert!(!stderr.contains(&base_url), "{stderr}");
}

/// Play Avalon with `args` after the game: the events on standard error,
/// parsed, and what was printed.
fn played_avalon(args: &[&str]) -> (Vec<Event<avalon::Entry>>, String) {
    let output = succeeded(&[&["avalon"], args].concat());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, String::from_utf8(output.stdout).unwrap())
}

/// Every role the deal Avalon's log opens with seats, in seat order.
fn avalon_roles(events: &[Event<avalon::Entry>]) -> Vec<avalon::Role> {
    let Some(avalon::Entry::Start {
        roles, strategies, ..
    }) = events.first().map(|event| &event.payload)
    else {
        panic!("{events:?}");
    };
    assert!(strategies.values().all(|strategy| strategy == "random"));
    let mut roles: Vec<avalon::Role> = roles.values().copied().collect();
    roles.sort_unstable();
    roles
}

#[test]
fn avalon_plays_the_default_table_to_the_end_and_tells_it() {
    let (events, printed) = played_avalon(&[]);
    let episode = events[0].episode;
    assert!(events.iter().all(|event| event.episode == episode));
    use avalon::Role::{Assassin, Merlin, Minion, Servant};
    assert_eq!(
        avalon_roles(&events),
        [Merlin, Servant, Servant, Assassin, Minion]
    );
    let Some(avalon::Entry::End { winner, .. }) = events.last().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    // The account opens with the table, a player to a line, then the
    // limits, goes through the first proposal, set off by a blank line,
    // and closes with the winner and how long the game took.
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[0], "5 players:", "{printed}");
    assert!(lines[1].ends_with(", plays random"), "{printed}");
    assert_eq!(
        lines[6], "A proposal waits 1m, a vote 1m, a quest 1m and the assassination 1m.",
        "{printed}"
    );
    assert_eq!(lines[7], "", "{printed}");
    assert!(
        lines[8].starts_with("Quest 1, proposal 1 of 5. player1 ("),
        "{printed}"
    );
    let won = match winner {
        avalon::Team::Good => "Good wins ",
        avalon::Team::Evil => "Evil wins ",
    };
    assert!(lines[lines.len() - 2].starts_with(won), "{printed}");
    assert!(
        lines[lines.len() - 1].starts_with("The game took "),
        "{printed}"
    );
}

#[test]
fn avalon_plays_any_valid_table_under_the_limits_given() {
    let args = [
        "--servants",
        "3",
        "--minions",
        "2",
        "--percival",
        "1",
        "--morgana",
        "1",
        "--vote-limit",
        "90s",
    ];
    let (events, printed) = played_avalon(&args);
    use avalon::Role::{Assassin, Merlin, Minion, Morgana, Percival, Servant};
    assert_eq!(
        avalon_roles(&events),
        [
            Merlin, Percival, Servant, Servant, Servant, Assassin, Morgana, Minion, Minion
        ]
    );
    assert!(printed.starts_with("9 players:\n"), "{printed}");
    assert!(
        printed.contains("A proposal waits 1m, a vote 1m 30s, a quest 1m"),
        "{printed}"
    );
}

#[test]
fn avalon_no_log_leaves_standard_error_empty() {
    let output = succeeded(&["avalon", "--no-log"]);
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("5 players:\n")
    );
}

#[test]
fn avalon_refuses_an_invalid_table_saying_what_is_wrong() {
    let stderr = refused(&["avalon", "--merlin", "0", "--servants", "3"]);
    assert!(
        stderr.contains("a table has Merlin exactly when it has an Assassin"),
        "{stderr}"
    );
    let stderr = refused(&["avalon", "--servants", "0"]);
    assert!(stderr.contains("5 to 10 players"), "{stderr}");
    assert_usage_error(&["avalon", "game.toml", "other.toml"]);
}

/// The configuration Avalon's log has right after the deal.
fn avalon_configuration(events: &[Event<avalon::Entry>]) -> &avalon::config::Configuration {
    match events.get(1).map(|event| &event.payload) {
        Some(avalon::Entry::Configuration(configuration)) => configuration,
        second => panic!("{second:?}"),
    }
}

/// An Avalon example configuration file, by its name in the examples
/// directory.
fn avalon_example(name: &str) -> String {
    format!("{}/examples/avalon/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn avalon_logs_the_configuration_of_random_players_with_no_file_and_with_the_example() {
    for args in [vec![], vec![avalon_example("random.toml")]] {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let (events, printed) = played_avalon(&args);
        use avalon::Role::{Assassin, Merlin, Minion, Servant};
        assert_eq!(
            avalon_roles(&events),
            [Merlin, Servant, Servant, Assassin, Minion]
        );
        let configuration = avalon_configuration(&events);
        assert!(configuration.models.is_empty(), "{configuration:?}");
        assert!(configuration.prompts.is_empty(), "{configuration:?}");
        assert_eq!(configuration.role_counts[&Servant], 2);
        assert!(
            matches!(
                events.last().map(|event| &event.payload),
                Some(avalon::Entry::End { .. })
            ),
            "{events:?}"
        );
        assert!(printed.starts_with("5 players:\n"), "{printed}");
    }
}

/// A configuration file written for this test, holding `file`, in the
/// target directory, at a path of its own so the tests do not collide.
fn written(test: &str, file: &str) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(&path, file).unwrap();
    path
}

#[test]
fn avalon_hears_the_command_line_over_the_file() {
    let config = written(
        "avalon-hears",
        "[phases]\nvote_limit = \"2m\"\nquest_limit = \"3m\"\n[roles.servant]\ncount = 3\n[roles.minion]\ncount = 2\n",
    );
    let (events, printed) = played_avalon(&[&config, "--minions", "1", "--quest-limit", "30s"]);
    let configuration = avalon_configuration(&events);
    assert_eq!(configuration.role_counts[&avalon::Role::Servant], 3);
    assert_eq!(configuration.role_counts[&avalon::Role::Minion], 1);
    assert_eq!(configuration.vote_limit, Duration::from_secs(120));
    assert_eq!(configuration.quest_limit, Duration::from_secs(30));
    assert!(printed.starts_with("6 players:\n"), "{printed}");
    assert!(
        printed.contains("A proposal waits 1m, a vote 2m, a quest 30s"),
        "{printed}"
    );
}

#[test]
fn avalon_refuses_a_file_it_cannot_read() {
    let stderr = refused(&["avalon", "no-such-file.toml"]);
    assert!(stderr.contains("no-such-file.toml"), "{stderr}");
}

/// A configuration file written for this test: Avalon's prompted example,
/// its model served at `base_url` as [`EXAMPLE_MODEL`], with `rest` after
/// it.
fn avalon_prompted(test: &str, base_url: &str, rest: &str) -> String {
    let example = std::fs::read_to_string(avalon_example("prompted.toml")).unwrap();
    let file = example
        .replace("http://localhost:1234/v1", base_url)
        .replace(
            "id = \"qwen2.5-7b-instruct-1m\"",
            &format!("id = \"{EXAMPLE_MODEL}\""),
        );
    assert!(
        file.contains(base_url) && file.contains(EXAMPLE_MODEL),
        "{file}"
    );
    written(test, &format!("{file}{rest}"))
}

#[test]
fn avalon_plays_a_table_of_random_evil_and_prompted_good_to_the_end() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = avalon_prompted(
        "avalon-mixed",
        &base_url,
        "[roles.assassin]\nstrategy = \"random\"\n[roles.minion]\nstrategy = \"random\"\n",
    );
    let (events, printed) = played_avalon(&[&config]);
    let Some(avalon::Entry::Start {
        roles, strategies, ..
    }) = events.first().map(|event| &event.payload)
    else {
        panic!("{events:?}");
    };
    for (seat, role) in roles {
        let expected = match role.team() {
            avalon::Team::Good => "everyone",
            avalon::Team::Evil => "random",
        };
        assert_eq!(strategies[seat], expected, "{seat}");
    }
    // Only the good players are told prompts, each its own.
    let configuration = avalon_configuration(&events);
    assert_eq!(configuration.models["qwen"].id, EXAMPLE_MODEL);
    let prompted: HashSet<&PlayerId> = configuration.prompts.keys().collect();
    let good: HashSet<&PlayerId> = roles
        .iter()
        .filter(|(_, role)| role.team() == avalon::Team::Good)
        .map(|(seat, _)| seat)
        .collect();
    assert_eq!(prompted, good);
    for (seat, prompt) in &configuration.prompts {
        let role = roles[seat];
        let told = format!(
            "You are {seat}, {}, on the {} side.",
            role.told(),
            role.team()
        );
        assert!(prompt.ends_with(&told), "{prompt}");
    }
    // A player stopped in the middle of a step may log after the end, so
    // the end is somewhere in the log rather than last.
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, avalon::Entry::End { .. })),
        "{events:?}"
    );
    // The prompted players talk, and the account tells it.
    assert!(
        printed.contains("qwen's context window is 4096 tokens, the default."),
        "{printed}"
    );
    assert!(
        printed.contains("says, \"I have nothing to add.\""),
        "{printed}"
    );
}

#[test]
fn avalon_refuses_a_model_that_does_not_play_naming_it_before_anything_is_printed() {
    let base_url = refusing(&[EXAMPLE_MODEL]);
    let config = avalon_prompted("avalon-refused", &base_url, "");
    let stderr = refused(&["avalon", &config]);
    for said in [
        "setting up the model qwen".to_string(),
        format!("{EXAMPLE_MODEL} did not answer as the leader"),
    ] {
        assert!(stderr.contains(&said), "{stderr}");
    }
    assert!(stderr.contains(&refusal(EXAMPLE_MODEL)), "{stderr}");
}
