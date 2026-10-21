//! The binary plays a configuration file's game, tells it on standard
//! output, and logs it to standard error as JSON Lines.

use free_agent::Event;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use twenty_questions::log::{Entry, Header, Victory};
use twenty_questions::model::canned::{playing, refusing};

/// The model the tests' configuration files name.
const MODEL: &str = "qwen2.5-7b-instruct";

/// Run the command.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_twenty-questions"))
        .args(args)
        .output()
        .unwrap()
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
    assert_eq!(
        lines[..3],
        [
            "Alice, Bob, Carol and Dave play everyone.",
            "They have 20 guesses and 1m.",
            "qwen's context window is 3072 tokens, as the configuration file says.",
        ]
    );
    // The category is drawn at random.
    assert!(
        lines[3].starts_with("Asked for ")
            && lines[3].ends_with(", the host is thinking of a teapot."),
        "{told}"
    );
    assert!(lines.contains(&"Alice: Let's ask."), "{told}");
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
}
