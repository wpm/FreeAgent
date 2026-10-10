//! The binary names a game and its variant, plays one game, tells it on
//! standard output, and logs it to standard error as JSON lines.

use free_agent::Event;
use social_deduction::model::canned::{playing, serving, unreachable};
use social_deduction::werewolf::llm::Configuration;
use social_deduction::werewolf::{Entry, PlayerId, Role};
use std::collections::HashMap;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

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

/// Play a game of `variant` with `args` after it, whose log must name
/// `variation`, with the default table, and check that it is logged from
/// the deal to the result under one episode id and told from the table to
/// the winner. The events, for whatever else the variant logs.
fn assert_plays_the_default_table(
    variant: &str,
    variation: &str,
    args: &[&str],
) -> Vec<Event<Entry>> {
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
    // The account opens with the table, goes through the first night, and
    // closes with the winner.
    let lines: Vec<&str> = printed.lines().collect();
    assert!(
        lines[0].starts_with(&format!("{variation} with 7 players: ")),
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
    events
}

#[test]
fn the_log_runs_from_the_deal_to_the_result_under_one_episode_id() {
    assert_plays_the_default_table("uniform-random", "Uniform Random", &[]);
}

#[test]
fn the_scripted_variant_plays_the_announcing_environment_without_a_model() {
    assert_plays_the_default_table("scripted", "Scripted", &[]);
}

#[test]
fn the_table_is_dealt_as_asked() {
    for variant in ["uniform-random", "scripted"] {
        let (events, _) = played(
            variant,
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
}

#[test]
fn the_scripted_variant_takes_the_limits_and_needs_no_configuration_file() {
    succeeded(&[
        "werewolf",
        "scripted",
        "--night-limit",
        "5s",
        "--day-limit",
        "1m",
    ]);
    assert_usage_error(&["werewolf", "scripted", "--night-limit", "soon"]);
    assert_usage_error(&["werewolf", "scripted", "game.toml"]);
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
    for variant in ["uniform-random", "scripted", "llm"] {
        assert!(help.contains(variant), "{help}");
    }
    let counts = ["--werewolves", "--villagers", "--doctors", "--seers"];
    let limits = ["--night-limit", "--day-limit"];
    for (variant, limited) in [("uniform-random", false), ("scripted", true), ("llm", true)] {
        let help = printed(&["werewolf", variant, "--help"]);
        for option in counts.iter().chain(limits.iter().filter(|_| limited)) {
            assert!(help.contains(option), "{help}");
        }
    }
}

/// The base URL the examples name. Something may well be listening there,
/// so no test asks it anything.
const EXAMPLE_BASE_URL: &str = "http://localhost:1234/v1";

/// The model the examples name, which is known to make tool calls.
const EXAMPLE_MODEL: &str = "qwen2.5-7b-instruct";

/// An example configuration file, by its name in the examples directory.
fn example(name: &str) -> String {
    format!(
        "{}/examples/werewolf/llm/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// A configuration file written for this test: the example `name` with its
/// provider replaced by one at `base_url` and its model by `id`, in the
/// target directory, at a path of its own so the tests do not collide.
fn configured(test: &str, name: &str, base_url: &str, id: &str) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    let example = std::fs::read_to_string(example(name)).unwrap();
    for named in [EXAMPLE_BASE_URL, EXAMPLE_MODEL] {
        assert!(example.contains(named), "{name} no longer names {named}");
    }
    let file = example
        .replace(EXAMPLE_BASE_URL, base_url)
        .replace(EXAMPLE_MODEL, id);
    std::fs::write(&path, file).unwrap();
    path
}

/// Short limits, so that a phase nobody selects in ends soon.
const QUICK: [&str; 4] = ["--night-limit", "1s", "--day-limit", "1s"];

/// The configuration the log of `events` has right after the deal.
fn configuration(events: &[Event<Entry>]) -> &Configuration {
    let Some(Entry::Configuration(configuration)) = events.get(1).map(|event| &event.payload)
    else {
        panic!("{events:?}");
    };
    configuration
}

#[test]
fn the_llm_variant_plays_a_game_of_model_players_logged_from_the_configuration_to_the_result() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured("plays", "personas.toml", &base_url, EXAMPLE_MODEL);
    let started = Instant::now();
    let args = [&[config.as_str()], &QUICK[..]].concat();
    let events = assert_plays_the_default_table("llm", "LLM", &args);
    // Every player selects at once, so no phase waits out its limit.
    assert!(started.elapsed() < Duration::from_secs(10));
    let (_, roles) = start(&events);
    let configuration = configuration(&events);
    assert_eq!(configuration.base_url, base_url);
    assert_eq!(configuration.model, EXAMPLE_MODEL);
    assert_eq!(configuration.night_limit, Duration::from_secs(1));
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
    // The configuration is logged once, and nothing else is printed.
    let configurations = events
        .iter()
        .filter(|event| matches!(event.payload, Entry::Configuration(_)))
        .count();
    assert_eq!(configurations, 1);
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
    assert_eq!(configuration.model, "gpt-5.5");
    assert_eq!(configuration.base_url, base_url);
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
fn the_llm_variant_refuses_a_model_not_known_to_make_tool_calls_before_asking_anyone() {
    // The example provider is not there, which goes unnoticed: the check
    // needs no network.
    let config = configured("unknown", "minimal.toml", EXAMPLE_BASE_URL, "gpt-0");
    let stderr = refused(&["werewolf", "llm", &config]);
    assert!(stderr.contains("gpt-0"), "{stderr}");
    assert!(stderr.contains("src/models.toml"), "{stderr}");
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
fn the_models_command_lists_the_provider_s_models_sorted_and_marks_those_that_make_tool_calls() {
    let base_url = serving(&["zephyr-7b", EXAMPLE_MODEL, "llama-3.1-8b-instruct"]);
    let printed = printed(&["models", &base_url]);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines,
        [
            "  llama-3.1-8b-instruct",
            "* qwen2.5-7b-instruct",
            "  zephyr-7b",
            "",
            "* marks a model known to make tool calls.",
        ],
        "{printed}"
    );
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
