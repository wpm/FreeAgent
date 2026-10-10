//! The binary names a game and its variant, plays one game, tells it on
//! standard output, and logs it to standard error as JSON lines.

use free_agent::Event;
use social_deduction::werewolf::{Entry, PlayerId, Role};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
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

/// Run the command, which must fail before printing anything; what it said
/// on standard error.
fn refused(args: &[&str]) -> String {
    let output = run(args);
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    String::from_utf8(output.stderr).unwrap()
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
    let help = printed(&["--help"]);
    assert!(
        help.contains("werewolf") && help.contains("models"),
        "{help}"
    );
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

/// A listener on a port of the OS's choosing, and the root of the API it
/// would serve.
fn bound() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    (listener, base_url)
}

/// A provider that serves `ids`: a listener that answers one request with
/// the listing, in a thread of its own, and the root of its API.
fn provider(ids: &[&str]) -> String {
    let (listener, base_url) = bound();
    let data: Vec<_> = ids.iter().map(|id| serde_json::json!({"id": id})).collect();
    let body = serde_json::json!({"data": data}).to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut buffer = [0; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0, "the request ended before its head did");
            head.extend_from_slice(&buffer[..n]);
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    base_url
}

/// The base URL the examples name. Something may well be listening there,
/// so no test asks it anything.
const EXAMPLE_BASE_URL: &str = "http://localhost:1234/v1";

/// The model the examples name, which is known to make tool calls.
const EXAMPLE_MODEL: &str = "qwen2.5-7b-instruct";

/// The root of an API nobody serves: a port that was listening and is no
/// more.
fn nobody() -> String {
    bound().1
}

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

#[test]
fn the_llm_variant_prints_its_settings_then_every_prompt_and_is_not_playable_yet() {
    let base_url = provider(&[EXAMPLE_MODEL]);
    let config = configured("prints", "personas.toml", &base_url, EXAMPLE_MODEL);
    let printed = printed(&["werewolf", "llm", "--config", &config]);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines[0],
        format!("model: qwen2.5-7b-instruct at {base_url}"),
        "{printed}"
    );
    // Every seat for every role, each under a header, after the settings.
    let headers: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("--- "))
        .collect();
    assert_eq!(headers.len(), 7 * 4, "{printed}");
    assert_eq!(headers[0], "--- player1 as werewolf ---", "{printed}");
    assert_eq!(headers[27], "--- player7 as seer ---", "{printed}");
    assert_eq!(lines[6], headers[0], "{printed}");
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
    let stderr = refused(&["werewolf", "llm", "--config", &path]);
    for expected in ["player1", "werewolf", "rulez"] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn the_llm_variant_hears_the_command_line_over_the_file() {
    let base_url = provider(&[EXAMPLE_MODEL]);
    let config = configured("hears", "personas.toml", &base_url, EXAMPLE_MODEL);
    let printed = printed(&[
        "werewolf",
        "llm",
        "--config",
        &config,
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
    let stderr = refused(&["werewolf", "llm", "--config", "no-such-file.toml"]);
    assert!(stderr.contains("no-such-file.toml"), "{stderr}");
}

#[test]
fn the_llm_variant_refuses_a_model_not_known_to_make_tool_calls_before_asking_anyone() {
    // The example provider is not there, which goes unnoticed: the check
    // needs no network.
    let config = configured("unknown", "minimal.toml", EXAMPLE_BASE_URL, "gpt-0");
    let stderr = refused(&["werewolf", "llm", "--config", &config]);
    assert!(stderr.contains("gpt-0"), "{stderr}");
    assert!(stderr.contains("src/tool_models.txt"), "{stderr}");
}

#[test]
fn the_llm_variant_refuses_a_model_its_provider_does_not_serve() {
    let base_url = provider(&["llama-3.1-8b-instruct"]);
    let config = configured("unserved", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let stderr = refused(&["werewolf", "llm", "--config", &config]);
    assert!(stderr.contains(&base_url), "{stderr}");
    assert!(stderr.contains(EXAMPLE_MODEL), "{stderr}");
}

#[test]
fn the_models_command_lists_the_provider_s_models_sorted_and_marks_those_that_make_tool_calls() {
    let base_url = provider(&["zephyr-7b", EXAMPLE_MODEL, "llama-3.1-8b-instruct"]);
    let config = configured("models", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let printed = printed(&["models", "--config", &config]);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(
        lines,
        [
            "  llama-3.1-8b-instruct",
            "* qwen2.5-7b-instruct",
            "  zephyr-7b",
            "* marks a model known to make tool calls.",
        ],
        "{printed}"
    );
}

#[test]
fn the_models_command_needs_a_configuration_file_and_names_a_provider_it_cannot_reach() {
    assert_usage_error(&["models"]);
    assert!(printed(&["models", "--help"]).contains("--config"));
    let base_url = nobody();
    let config = configured("nobody", "minimal.toml", &base_url, EXAMPLE_MODEL);
    let stderr = refused(&["models", "--config", &config]);
    assert!(stderr.contains(&base_url), "{stderr}");
}
