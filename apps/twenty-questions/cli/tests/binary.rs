//! The binary plays a configuration file's game for as many episodes as
//! asked, tells it on standard output, and logs it to standard error as
//! JSON Lines; lists the models a provider serves and marks those that
//! play; and lists and writes out the example configurations.

use free_agent::Event;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use twenty_questions::log::{Entry, Header, Victory};
use twenty_questions::model::canned::{
    playing, playing_only, refusal, refusing, serving, unreachable, wanting_a_key,
};

/// The model the tests' configuration files name.
const MODEL: &str = "qwen2.5-7b-instruct";

/// Run the command.
fn run(args: &[&str]) -> Output {
    run_in(Path::new("."), args)
}

/// Run the command in `dir`.
fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_twenty-questions"))
        .current_dir(dir)
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

/// What the command printed on standard output, having exited well.
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

/// The example configuration, playing against the model served at
/// `base_url`, with its game a minute long, written for the test `test`.
fn configured(test: &str, base_url: &str) -> String {
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/configs/prompted.toml");
    let mut file: toml::Table = toml::from_str(&std::fs::read_to_string(example).unwrap()).unwrap();
    let qwen = file["models"]["qwen"].as_table_mut().unwrap();
    qwen.insert("base_url".to_string(), base_url.into());
    qwen.insert("id".to_string(), MODEL.into());
    file.insert("time_limit".to_string(), "1m".into());
    written(test, &toml::to_string(&file).unwrap())
}

/// `text` written as the configuration file for the test `test`.
fn written(test: &str, text: &str) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn the_example_plays_to_the_end_tells_the_game_and_logs_it_between_its_header_and_its_end() {
    let config = configured("example", &playing(&[MODEL]));
    let output = run(&["play", &config]);
    assert!(output.status.success(), "{output:?}");
    let told = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = told.lines().collect();
    // The parts are dealt to the seats at random.
    let mut parts: Vec<&str> = lines[..3]
        .iter()
        .map(|line| line.split_once(" plays ").unwrap().1)
        .collect();
    parts.sort();
    assert_eq!(parts, ["closer.", "keeper.", "scout."], "{told}");
    assert_eq!(
        lines[3..5],
        [
            "They have 20 guesses and 1m.",
            "qwen's context window is 3072 tokens, as the configuration file says.",
        ]
    );
    // The category is drawn at random.
    assert!(
        lines[5].starts_with("Asked for ")
            && lines[5].ends_with(", the host is thinking of a teapot."),
        "{told}"
    );
    // A guess is put only once someone has spoken, but who speaks first
    // depends on how the players' calls are scheduled.
    assert!(
        lines.iter().any(|line| line.ends_with(": Let's ask.")),
        "{told}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with("guesses (1 of 20): Is it a teapot?")),
        "{told}"
    );
    assert!(lines.contains(&"Host: Yes, it is!"), "{told}");
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("The players named a teapot with 1 of their 20 guesses, in "),
        "{told}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events: Vec<Event<Value>> = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let header: Header = serde_json::from_value(events[0].payload["Header"].clone()).unwrap();
    assert_eq!(header, Header::new());
    assert_eq!(
        events.last().unwrap().payload,
        serde_json::json!({"Ended": "won"})
    );
    let entries: Vec<Entry> = events[1..events.len() - 1]
        .iter()
        .map(|event| serde_json::from_value(event.payload.clone()).unwrap())
        .collect();
    assert!(
        matches!(entries[0], Entry::Start { .. }),
        "{:?}",
        entries[0]
    );
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            Entry::End {
                by: Victory::Named,
                ..
            }
        )),
        "{stderr}"
    );
}

#[test]
fn a_model_that_does_not_play_is_refused_before_anything_is_printed() {
    let config = configured("refusing", &refusing(&[MODEL]));
    let stderr = refused(&["play", &config]);
    assert!(
        stderr.contains(&format!("{MODEL} did not play Twenty Questions")),
        "{stderr}"
    );
}

#[test]
fn a_file_that_is_missing_or_malformed_is_an_invalid_configuration_file_naming_it() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-file.toml");
    let missing = missing.to_str().unwrap();
    let stderr = refused(&["play", missing]);
    assert!(
        stderr.contains(&format!("invalid configuration file {missing}")),
        "{stderr}"
    );
    // A file that is missing says how to write out the examples.
    assert!(
        stderr.starts_with(&format!(
            "Error: no configuration file {missing}; `twenty-questions configs --write .` \
             writes the example configurations into the current directory"
        )),
        "{stderr}"
    );
    let config = written(
        "malformed",
        "[players]\ncount = 13\n[host]\nsystem = \"Host.\"\nobject = \"a cat\"\n",
    );
    let stderr = refused(&["play", &config]);
    assert!(
        stderr.contains(&format!("invalid configuration file {config}")),
        "{stderr}"
    );
    assert!(stderr.contains("[players] count is 13"), "{stderr}");
    assert!(!stderr.contains("configs --write"), "{stderr}");
}

/// Every event a log holds, in order.
fn events(log: &[u8]) -> Vec<Event<Value>> {
    std::str::from_utf8(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn episodes_play_one_after_another_each_logged_between_its_own_header_and_end() {
    let config = configured("episodes", &playing(&[MODEL]));
    let output = succeeded(&["play", &config, "--episodes", "3"]);
    let events = events(&output.stderr);
    let mut episodes: Vec<_> = events.iter().map(|event| event.episode).collect();
    episodes.dedup();
    assert_eq!(episodes.len(), 3, "{episodes:?}");
    assert_eq!(HashSet::<_>::from_iter(&episodes).len(), 3, "{episodes:?}");
    for episode in episodes {
        let logged: Vec<&Value> = events
            .iter()
            .filter(|event| event.episode == episode)
            .map(|event| &event.payload)
            .collect();
        assert!(logged[0].get("Header").is_some(), "{logged:?}");
        assert_eq!(
            *logged.last().unwrap(),
            &serde_json::json!({"Ended": "won"})
        );
        // The host picks an object of its own for each episode.
        let chosen = logged
            .iter()
            .filter(|entry| entry.get("Chosen").is_some())
            .count();
        assert_eq!(chosen, 1, "{logged:?}");
    }
    // Each episode is told in turn, set off from the one before by a blank
    // line.
    let told = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = told.lines().collect();
    let seated = |at: usize| lines[at].contains(" plays ");
    let opening: Vec<usize> = (0..lines.len())
        .filter(|at| seated(*at) && (*at == 0 || !seated(at - 1)))
        .collect();
    assert_eq!(opening.len(), 3, "{told}");
    assert_eq!(opening[0], 0, "{told}");
    assert!(
        opening[1..].iter().all(|at| lines[at - 1].is_empty()),
        "{told}"
    );
}

#[test]
fn no_episodes_is_a_usage_error() {
    let output = run(&["play", "game.toml", "--episodes", "0"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--episodes"), "{stderr}");
}

#[test]
fn no_log_leaves_standard_error_empty_and_the_game_is_still_told() {
    let config = configured("no-log", &playing(&[MODEL]));
    let output = succeeded(&["play", &config, "--no-log"]);
    assert!(output.stderr.is_empty(), "{output:?}");
    let told = String::from_utf8(output.stdout).unwrap();
    assert!(told.contains("The players named a teapot"), "{told}");
}

/// The legend the `--check` listing ends with.
const LEGEND: &str = "* marks a model that played when asked, ~ one that played when asked to call \
                      a tool but not made to, and ! one that did not.";

#[test]
fn the_models_command_lists_the_provider_s_models_sorted_and_plain() {
    let base_url = serving(&["zephyr-7b", MODEL, "llama-3.1-8b-instruct"]);
    let output = succeeded(&["models", &base_url]);
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "llama-3.1-8b-instruct\nqwen2.5-7b-instruct\nzephyr-7b\n"
    );
}

#[test]
fn the_models_command_with_check_asks_each_model_to_play_and_marks_its_answer() {
    let base_url = playing_only(&["zephyr-7b", MODEL], &[MODEL]);
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
    let refused = format!(
        "POST {base_url}/chat/completions was answered 400 Bad Request: {}",
        refusal("zephyr-7b")
    );
    assert_eq!(
        reasons,
        [
            "! zephyr-7b".to_string(),
            "  zephyr-7b did not play Twenty Questions".to_string(),
            "  did not answer as a player".to_string(),
            format!("  made to call a tool: {refused}"),
            format!("  asked to call one: {refused}"),
        ],
        "{stderr}"
    );
}

#[test]
fn the_models_command_with_check_stops_at_a_model_that_could_not_be_asked() {
    // A provider that wants a key answers nothing about any model.
    let base_url = wanting_a_key(&["zephyr-7b", MODEL]);
    let stderr = refused(&["models", &base_url, "--check"]);
    assert!(
        stderr.starts_with(&format!("Error: {MODEL} could not be asked to play")),
        "{stderr}"
    );
    assert!(stderr.contains("401 Unauthorized"), "{stderr}");
    assert!(!stderr.contains("zephyr-7b"), "{stderr}");
}

#[test]
fn the_models_command_with_ids_to_check_asks_only_those_and_needs_each_to_be_listed() {
    let base_url = refusing(&["zephyr-7b", MODEL, "llama-3.1-8b-instruct"]);
    let output = succeeded(&["models", &base_url, "--check", MODEL]);
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
    assert!(stderr.starts_with(&format!("! {MODEL}\n  ")), "{stderr}");
    assert!(stderr.contains(&refusal(MODEL)), "{stderr}");
    // The ids end with -- when the base URL follows them.
    let output = succeeded(&["models", "--check", MODEL, "--", &base_url]);
    assert_eq!(String::from_utf8(output.stdout).unwrap(), printed);
    let stderr = refused(&["models", &base_url, "--check", MODEL, "gpt-0"]);
    assert!(stderr.contains("gpt-0"), "{stderr}");
    assert!(stderr.contains(&base_url), "{stderr}");
}

#[test]
fn the_models_command_names_a_provider_it_cannot_reach_and_its_help_ends_with_the_providers_known()
{
    let help = printed(&["models", "--help"]);
    assert!(help.contains("BASE_URL"), "{help}");
    assert!(help.contains("--api-key-env"), "{help}");
    assert!(help.contains("OPENAI_API_KEY"), "{help}");
    let base_url = unreachable();
    let stderr = refused(&["models", &base_url]);
    assert!(stderr.contains(&base_url), "{stderr}");
}

#[test]
fn the_models_command_refuses_a_key_variable_that_is_not_set_before_asking_anyone() {
    let base_url = unreachable();
    let name = "TWENTY_QUESTIONS_NO_SUCH_KEY";
    let stderr = refused(&["models", &base_url, "--api-key-env", name]);
    assert!(stderr.contains(name), "{stderr}");
    assert!(!stderr.contains(&base_url), "{stderr}");
}

/// An empty directory of its own for the test `test`.
fn emptied(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn configs_lists_every_example_configuration_with_its_description() {
    let listing = printed(&["configs"]);
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs");
    let mut expected: Vec<String> = std::fs::read_dir(examples)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    expected.sort();
    let paths: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(paths, expected, "{listing}");
    assert!(
        listing
            .contains("Three players and a host, all one model, who picks its own secret object."),
        "{listing}"
    );
}

#[test]
fn configs_writes_the_examples_and_overwrites_them_only_when_forced() {
    let dir = emptied("configs-write");
    let written = |args: &[&str]| run_in(&dir, &[&["configs", "--write", "."], args].concat());
    let output = written(&[]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/configs/prompted.toml");
    assert_eq!(
        std::fs::read(dir.join("prompted.toml")).unwrap(),
        std::fs::read(example).unwrap()
    );
    let output = written(&[]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("already exists; --force overwrites it"),
        "{stderr}"
    );
    assert!(written(&["--force"]).status.success());
}
