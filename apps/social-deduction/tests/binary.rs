//! The binary names a game and its variant, plays one game, tells it on
//! standard output, and logs it to standard error as JSON lines.

use free_agent::Event;
use social_deduction::model::ToolChoice;
use social_deduction::model::canned::{
    day_refusal, playing, playing_only, playing_within, refusal, refusing, refusing_days,
    refusing_required, serving, unreachable, wanting_a_key,
};
use social_deduction::werewolf::prompted::{Configuration, ContextWindow, Source};
use social_deduction::werewolf::{Ending, Entry, Message, PlayerId, Role};
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

/// Play Werewolf in `variant` with `args` after the game and its variant:
/// the events on standard error, parsed, and what was printed.
fn played(variant: &str, args: &[&str]) -> (Vec<Event<Entry>>, String) {
    let output = succeeded(&[&["werewolf", variant], args].concat());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, String::from_utf8(output.stdout).unwrap())
}

/// The variation and the deal the log opens with.
fn start(events: &[Event<Entry>]) -> (&str, &HashMap<PlayerId, Role>) {
    let Some(Entry::Start {
        variation, roles, ..
    }) = events.first().map(|event| &event.payload)
    else {
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

/// Play a game of `variant` with `args` after it, whose log must name
/// `variation`, with the default table, and check that it is logged from
/// the deal to the result under one episode id and told from the table to
/// the winner, with `notes` told after the limits. The events, for whatever
/// else the variant logs, and what was printed.
fn assert_plays_the_default_table(
    variant: &str,
    variation: &str,
    args: &[&str],
    notes: &[&str],
) -> (Vec<Event<Entry>>, String) {
    let (events, printed) = played(variant, args);

    let episode = events[0].episode;
    assert!(events.iter().all(|event| event.episode == episode));
    let (named, roles) = start(&events);
    assert_eq!(named, variation);
    // The default table seats seven.
    assert_eq!(roles.len(), 7);
    let Some(Entry::End { winner, .. }) = events.last().map(|event| &event.payload) else {
        panic!("{events:?}");
    };
    // The account opens with the table, a player to a line, then the
    // limits and the notes, goes through the first night, set off by a
    // blank line, tells
    // how long the last phase took, and closes with the winner, how long
    // the game took, and the survivors, a player to a line.
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines[0],
        format!("{variation} with 7 players:"),
        "{printed}"
    );
    for seat in &lines[1..8] {
        assert!(
            seat.starts_with("player") && seat.ends_with(')'),
            "{printed}"
        );
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

#[test]
fn the_log_runs_from_the_deal_to_the_result_under_one_episode_id() {
    assert_plays_the_default_table("uniform-random", "Uniform Random", &[], &[]);
}

#[test]
fn no_log_leaves_standard_error_empty_and_the_game_is_still_told() {
    let output = succeeded(&["werewolf", "uniform-random", "--no-log"]);
    assert!(output.stderr.is_empty(), "{output:?}");
    let printed = String::from_utf8(output.stdout).unwrap();
    assert!(
        printed.starts_with("Uniform Random with 7 players:\n"),
        "{printed}"
    );
    assert!(printed.contains("\nSurvivors:\n"), "{printed}");
    // Given after --no-log, --log puts the log back.
    let (events, _) = played("uniform-random", &["--no-log", "--log"]);
    assert!(
        matches!(events[0].payload, Entry::Start { .. }),
        "{events:?}"
    );
}

#[test]
fn the_table_is_dealt_as_asked() {
    let (events, _) = played(
        "uniform-random",
        &[
            "--werewolves",
            "1",
            "--villagers",
            "2",
            "--doctors",
            "0",
            "--seers",
            "0",
        ],
    );

    let (_, roles) = start(&events);
    let count = |role| roles.values().filter(|r| **r == role).count();
    assert_eq!(roles.len(), 3);
    assert_eq!(count(Role::Werewolf), 1);
    assert_eq!(count(Role::Villager), 2);
}

#[test]
fn the_uniform_random_variant_takes_the_limits_and_needs_no_configuration_file() {
    let (events, _) = played(
        "uniform-random",
        &["--night-limit", "5s", "--day-limit", "1m"],
    );
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
    // The uniform-random variant's day is not ratified, and it takes no
    // ratify limit.
    assert_eq!(*ratify_limit, Duration::ZERO);
    assert_usage_error(&["werewolf", "uniform-random", "--night-limit", "soon"]);
    assert_usage_error(&["werewolf", "uniform-random", "--ratify-limit", "5s"]);
    assert_usage_error(&["werewolf", "uniform-random", "game.toml"]);
}

#[test]
fn a_uniform_random_day_ends_once_everyone_has_voted_and_is_logged_as_such() {
    let (events, printed) = played("uniform-random", &[]);
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
    let help = printed(&["--help"]);
    assert!(
        help.contains("werewolf") && help.contains("models"),
        "{help}"
    );
    let help = printed(&["werewolf", "--help"]);
    let options = [
        "--werewolves",
        "--villagers",
        "--doctors",
        "--seers",
        "--night-limit",
        "--day-limit",
    ];
    for variant in ["uniform-random", "llm"] {
        assert!(help.contains(variant), "{help}");
        let help = printed(&["werewolf", variant, "--help"]);
        for option in options {
            assert!(help.contains(option), "{help}");
        }
        assert_eq!(help.contains("--ratify-limit"), variant == "llm", "{help}");
    }
}

/// The model the examples name.
const EXAMPLE_MODEL: &str = "qwen2.5-7b-instruct";

/// An example configuration file, by its name in the examples directory.
fn example(name: &str) -> String {
    format!(
        "{}/examples/werewolf/llm/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A configuration file written for this test: the example `name` with its
/// provider replaced by one at `base_url` and its model by `id`, each on
/// the line of the `[model]` table that names it and nowhere else, in the
/// target directory, at a path of its own so the tests do not collide.
fn configured(test: &str, name: &str, base_url: &str, id: &str) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    let example = std::fs::read_to_string(example(name)).unwrap();
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
            .find(|(named, _)| table == "[model]" && key == Some(named));
        file.push_str(replacement.map_or(line, |(_, written)| written));
        file.push('\n');
    }
    for (_, line) in &replacements {
        assert!(
            file.lines().any(|written| written == line),
            "{name} has no line of its [model] table to write {line} on"
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

/// The configuration the log of `events` has right after the deal.
fn configuration(events: &[Event<Entry>]) -> &Configuration {
    let Some(Entry::Configuration(configuration)) = events.get(1).map(|event| &event.payload)
    else {
        panic!("{events:?}");
    };
    configuration
}

/// What the account says of the model's context window when nothing gives
/// one.
const DEFAULT_WINDOW: &str = "The model's context window is 4096 tokens, the default.";

#[test]
fn the_context_window_the_provider_reports_is_used_unless_the_command_line_gives_one() {
    let base_url = playing_within(&[EXAMPLE_MODEL], 32768);
    let config = configured("window", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let args = [
        &config,
        "--night-limit",
        "1s",
        "--day-limit",
        "2s",
        "--ratify-limit",
        "1s",
    ];
    let told = "The model's context window is 32768 tokens, as the provider reports.";
    let (events, _) = assert_plays_the_default_table("llm", "LLM", &args, &[told]);
    assert_eq!(
        configuration(&events).context_window,
        ContextWindow {
            tokens: 32768,
            source: Source::Provider,
        }
    );
    let args = [&args[..], &["--context-window", "8192"]].concat();
    let told = "The model's context window is 8192 tokens, as the command line says.";
    let (events, _) = assert_plays_the_default_table("llm", "LLM", &args, &[told]);
    assert_eq!(
        configuration(&events).context_window,
        ContextWindow {
            tokens: 8192,
            source: Source::CommandLine,
        }
    );
}

/// A context window just large enough for the default table: a day's
/// request leaves room for little of the story.
const SMALL_WINDOW: &str = "1800";

#[test]
fn in_a_small_context_window_the_game_plays_to_the_end() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("forgetting", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let args = [
        &config,
        "--night-limit",
        "1s",
        "--day-limit",
        "2s",
        "--ratify-limit",
        "1s",
        "--context-window",
        SMALL_WINDOW,
    ];
    let told =
        format!("The model's context window is {SMALL_WINDOW} tokens, as the command line says.");
    let (events, _) = assert_plays_the_default_table("llm", "LLM", &args, &[&told]);
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
    let config = configured("window-small", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let stderr = refused(&["werewolf", "llm", &config, "--context-window", "1500"]);
    assert!(
        stderr.contains("--context-window is 1500 tokens, smaller than the"),
        "{stderr}"
    );
}

#[test]
fn a_context_window_of_0_is_refused_before_anything_is_printed() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("window-0", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let stderr = refused(&["werewolf", "llm", &config, "--context-window", "0"]);
    assert!(
        stderr.contains("--context-window is 0; a context window holds at least 1 token"),
        "{stderr}"
    );
}

#[test]
fn the_llm_variant_plays_a_game_of_model_players_logged_from_the_configuration_to_the_result() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("plays", "personas.toml", &base_url, EXAMPLE_MODEL);
    // Short limits, so that a phase nobody selects in ends soon, and a
    // day's votes stand sooner than the day runs out.
    let args = [
        &config,
        "--night-limit",
        "1s",
        "--day-limit",
        "2s",
        "--ratify-limit",
        "1s",
    ];
    let (events, printed) = assert_plays_the_default_table("llm", "LLM", &args, &[DEFAULT_WINDOW]);
    let (_, roles) = start(&events);
    let Entry::Start { ratify_limit, .. } = &events[0].payload else {
        panic!("{events:?}");
    };
    assert_eq!(*ratify_limit, Duration::from_secs(1));
    let configuration = configuration(&events);
    assert_eq!(configuration.base_url, base_url);
    assert_eq!(configuration.model, EXAMPLE_MODEL);
    assert_eq!(configuration.night_limit, Duration::from_secs(1));
    assert_eq!(configuration.ratify_limit, Duration::from_secs(1));
    assert_eq!(configuration.tool_choice, ToolChoice::Required);
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
        let told = match role {
            Role::Seer => format!("You are {player}, the seer."),
            role => format!("You are {player}, a {role}."),
        };
        assert!(prompt.contains(&told), "{player} as {role}: {prompt}");
    }
    assert_eq!(
        configuration.personas["player1"],
        "You are cautious and slow to accuse."
    );
    // The configuration is logged once.
    let configurations = events
        .iter()
        .filter(|event| matches!(event.payload, Entry::Configuration(_)))
        .count();
    assert_eq!(configurations, 1);
}

#[test]
fn a_model_that_refuses_to_be_made_to_call_a_tool_plays_when_asked_to_call_one() {
    let base_url = refusing_required(&[EXAMPLE_MODEL]);
    let config = configured("auto", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let args = [
        &config,
        "--night-limit",
        "1s",
        "--day-limit",
        "2s",
        "--ratify-limit",
        "1s",
    ];
    let note = format!(
        "{EXAMPLE_MODEL} does not accept a required tool call, \
         so its players are asked to call one."
    );
    let (events, _) = assert_plays_the_default_table("llm", "LLM", &args, &[DEFAULT_WINDOW, &note]);
    assert_eq!(configuration(&events).tool_choice, ToolChoice::Auto);
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
        "[model]\nbase_url = \"http://localhost:1234/v1\"\nid = \"m\"\n[prompt]\nsystem = \"{{ rulez }}\"\n",
    )
    .unwrap();
    let stderr = refused(&["werewolf", "llm", &path]);
    for expected in ["player1", "werewolf", "rulez"] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn the_llm_variant_hears_the_command_line_over_the_file() {
    let base_url = playing(&[EXAMPLE_MODEL, "gpt-5.5"]);
    let config = configured("hears", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let (events, _) = played(
        "llm",
        &[
            &config,
            "--werewolves",
            "1",
            "--night-limit",
            "1s",
            "--day-limit",
            "2s",
            "--ratify-limit",
            "1s",
            "--model-id",
            "gpt-5.5",
        ],
    );
    let (_, roles) = start(&events);
    assert_eq!(roles.len(), 6);
    let configuration = configuration(&events);
    assert_eq!(configuration.role_counts[&Role::Werewolf], 1);
    assert_eq!(configuration.role_counts[&Role::Villager], 3);
    assert_eq!(configuration.day_limit, Duration::from_secs(2));
    assert_eq!(configuration.ratify_limit, Duration::from_secs(1));
    assert_eq!(configuration.model, "gpt-5.5");
    assert_eq!(configuration.base_url, base_url);
}

#[test]
fn the_llm_variant_without_a_ratify_limit_plays_days_that_are_not_ratified() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("unratified", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let file = std::fs::read_to_string(&config).unwrap();
    let unratified: String = file
        .lines()
        .filter(|line| !line.starts_with("ratify_limit"))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(unratified, file);
    std::fs::write(&config, unratified).unwrap();
    let args = [&config, "--night-limit", "1s", "--day-limit", "2s"];
    let (events, printed) = assert_plays_the_default_table("llm", "LLM", &args, &[DEFAULT_WINDOW]);
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
    let config = configured("ratify", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let stderr = refused(&["werewolf", "llm", &config, "--day-limit", "10s"]);
    assert!(
        stderr.contains("the ratify limit (20s) is longer than the day limit (10s)"),
        "{stderr}"
    );
}

#[test]
fn the_llm_variant_needs_a_configuration_file() {
    assert_usage_error(&["werewolf", "llm"]);
    assert_usage_error(&["werewolf", "llm", "--werewolves", "1"]);
    assert!(printed(&["werewolf", "llm", "--help"]).contains("<CONFIG>"));
}

#[test]
fn a_configuration_file_that_cannot_be_read_is_an_error_naming_it() {
    let stderr = refused(&["werewolf", "llm", "no-such-file.toml"]);
    assert!(stderr.contains("no-such-file.toml"), "{stderr}");
}

#[test]
fn the_llm_variant_refuses_a_model_that_does_not_play_when_asked_in_the_provider_s_words() {
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
        let config = configured(name, "minimal.toml", &base_url, EXAMPLE_MODEL);
        let stderr = refused(&["werewolf", "llm", &config]);
        assert!(
            stderr.starts_with(&format!(
                "Error: {EXAMPLE_MODEL} did not answer a {phase}'s request"
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
fn the_llm_variant_refuses_a_model_its_provider_does_not_serve() {
    let base_url = serving(&["llama-3.1-8b-instruct"]);
    let config = configured("unserved", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let stderr = refused(&["werewolf", "llm", &config]);
    assert!(stderr.contains(&base_url), "{stderr}");
    assert!(stderr.contains(EXAMPLE_MODEL), "{stderr}");
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
