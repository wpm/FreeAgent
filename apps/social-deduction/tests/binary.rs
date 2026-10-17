//! The binary names a game and its configuration file, plays one game or
//! as many episodes as it is asked, tells them on standard output, and logs
//! them to standard error as JSON lines.

use free_agent::Event;
use serde::de::DeserializeOwned;
use social_deduction::avalon::{self, Avalon};
use social_deduction::config::Configuration;
use social_deduction::game::{self, Game};
use social_deduction::model::ToolChoice;
use social_deduction::model::canned::{
    day_refusal, playing, playing_only, playing_within, recording, refusal, refusing,
    refusing_days, refusing_offers_of, refusing_required, reported, retracting, serving,
    unreachable, wanting_a_key,
};
use social_deduction::model::window::{ContextWindow, Source};
use social_deduction::seat::PlayerId;
use social_deduction::werewolf::{Ending, Entry, Limits, Message, Move, Outcome, Role, Werewolf};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

fn run(args: &[&str]) -> Output {
    run_in(Path::new("."), args)
}

/// Run the command in `dir`.
fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_social-deduction"))
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

/// Play a game with `args` after `play`: the events on standard error,
/// parsed as entries of `E`, and what was printed.
fn played<E: DeserializeOwned>(args: &[&str]) -> (Vec<Event<E>>, String) {
    let output = succeeded(&[&["play"], args].concat());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, String::from_utf8(output.stdout).unwrap())
}

/// Assert that `stderr` calls the configuration file at `path` invalid.
fn assert_invalid(stderr: &str, path: &str) {
    assert!(
        stderr.contains(&format!("invalid configuration file {path}")),
        "{stderr}"
    );
}

/// The deal the log opens with: every player's role and strategy.
fn start<G: Game>(
    events: &[Event<game::Entry<G>>],
) -> (&HashMap<PlayerId, G::Role>, &HashMap<PlayerId, String>) {
    match events.first().map(|event| &event.payload) {
        Some(game::Entry::Start {
            roles, strategies, ..
        }) => (roles, strategies),
        first => panic!("{first:?}"),
    }
}

/// Assert that the command refuses its arguments, with the exit code `clap`
/// reserves for a usage error; what it said on standard error.
fn assert_usage_error(args: &[&str]) -> String {
    let output = run(args);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    String::from_utf8(output.stderr).unwrap()
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
    let Some(Entry::Event(Outcome::End { winner, .. })) = events.last().map(|event| &event.payload)
    else {
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
            seat.starts_with("Player ") && seat.ends_with(')'),
            "{printed}"
        );
    }
    (events, printed)
}

/// The limits the deal the log opens with gives each phase.
fn started<G: Game>(events: &[Event<game::Entry<G>>]) -> &G::Limits {
    match events.first().map(|event| &event.payload) {
        Some(game::Entry::Start { limits, .. }) => limits,
        first => panic!("{first:?}"),
    }
}

/// The configuration the log of `events` has right after the deal.
fn configuration<G: Game>(events: &[Event<game::Entry<G>>]) -> &Configuration<G> {
    match events.get(1).map(|event| &event.payload) {
        Some(game::Entry::Configuration(configuration)) => configuration,
        second => panic!("{second:?}"),
    }
}

#[test]
fn the_random_config_plays_every_player_random_and_is_logged_from_the_deal_to_the_result() {
    let (events, _) = assert_plays_the_default_table(&[&example("werewolf", "random.toml")], &[]);
    let (_, strategies) = start(&events);
    assert!(strategies.values().all(|strategy| strategy == "random"));
    // The configuration is logged, with no models.
    let configuration = configuration(&events);
    assert!(configuration.models.is_empty(), "{configuration:?}");
    assert!(configuration.prompts.is_empty(), "{configuration:?}");
}

#[test]
fn an_unratified_day_ends_once_everyone_has_voted_and_is_logged_as_such() {
    let (events, printed) = played(&[&example("werewolf", "random.toml")]);
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
fn a_command_must_be_named() {
    assert_usage_error(&[]);
}

#[test]
fn play_without_a_configuration_file_is_a_usage_error_naming_it() {
    let stderr = assert_usage_error(&["play"]);
    assert!(stderr.contains("<CONFIG>"), "{stderr}");
}

#[test]
fn the_games_are_unrecognized_subcommands() {
    for game in ["werewolf", "avalon"] {
        let stderr = assert_usage_error(&[game, &example("werewolf", "random.toml")]);
        assert!(
            stderr.contains(&format!("unrecognized subcommand '{game}'")),
            "{stderr}"
        );
    }
}

#[test]
fn the_settings_the_file_holds_are_unknown_arguments() {
    let config = example("werewolf", "random.toml");
    for flag in [
        "--werewolves",
        "--vote-limit",
        "--model-base-url",
        "--model-id",
        "--context-window",
    ] {
        let stderr = assert_usage_error(&["play", &config, flag, "1"]);
        assert!(
            stderr.contains(&format!("unexpected argument '{flag}'")),
            "{stderr}"
        );
    }
}

#[test]
fn help_lists_play_and_models_and_play_the_file_the_episodes_and_the_log_flags_alone() {
    let help = printed(&["--help"]);
    let commands: Vec<&str> = help
        .split("Commands:")
        .nth(1)
        .unwrap()
        .split("Options:")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(commands, ["play", "models", "configs", "help"], "{help}");
    let help = printed(&["play", "--help"]);
    assert!(help.contains("<CONFIG>"), "{help}");
    let flags: Vec<&str> = help
        .split_whitespace()
        .filter(|word| word.starts_with("--"))
        .collect();
    assert_eq!(
        flags,
        ["--episodes", "--log", "--no-log", "--help"],
        "{help}"
    );
}

#[test]
fn a_malformed_file_is_an_invalid_configuration_before_any_game_output() {
    for (test, file, said) in [
        ("no-game", "strategy = \"random\"\n", "missing field `game`"),
        (
            "chess",
            "game = \"chess\"\n",
            "unknown variant `chess`, expected `werewolf` or `avalon`",
        ),
        (
            "werewolf-merlin",
            "game = \"werewolf\"\n[roles.merlin]\ncount = 1\n",
            "unknown field `merlin`",
        ),
    ] {
        let config = written(test, file);
        let stderr = refused(&["play", &config]);
        assert_invalid(&stderr, &config);
        assert!(stderr.contains(said), "{stderr}");
        assert!(
            stderr.contains("line ") && stderr.contains("column "),
            "{stderr}"
        );
    }
}

/// The model the tests' configuration files name.
const EXAMPLE_MODEL: &str = "qwen2.5-7b-instruct";

/// A configuration file of `game`, by its name in the game's configs
/// directory.
fn example(game: &str, name: &str) -> String {
    format!("{}/configs/{game}/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A game as these tests play it.
trait Tested: Game {
    /// The game's name, as a configuration file's `game` key gives it.
    const KEY: &str;
    /// How many players the default table seats.
    const SEATS: usize;
    /// The `[phases]` a prompted game is played under: short ones, so
    /// that a phase nobody acts in ends soon.
    const PHASES: &[&str];
    /// How many requests the probe makes of a model.
    const PROBED: usize;
    /// What a model that does not play is said to have failed.
    const REFUSED: &str;
}

impl Tested for Werewolf {
    const KEY: &str = "werewolf";
    const SEATS: usize = 7;
    const PHASES: &[&str] = &QUICK;
    // One request as by night, and one as by day.
    const PROBED: usize = 2;
    const REFUSED: &str = "did not play Werewolf\n    1: did not answer a night's request";
}

impl Tested for Avalon {
    const KEY: &str = "avalon";
    const SEATS: usize = 5;
    // Avalon's phases end once the players they wait on have acted, which
    // the models these tests play do at once.
    const PHASES: &[&str] = &[];
    // One request for each kind of phase.
    const PROBED: usize = 4;
    const REFUSED: &str = "did not play Avalon\n    1: did not answer as the leader";
}

/// The TOML table `lines` make.
fn table(lines: &[&str]) -> toml::Table {
    lines.join("\n").parse().unwrap()
}

/// Rewrite the configuration file at `path` as `edit` changes it.
fn edited(path: &str, edit: impl FnOnce(&mut toml::Table)) {
    let mut file: toml::Table = std::fs::read_to_string(path).unwrap().parse().unwrap();
    edit(&mut file);
    std::fs::write(path, toml::to_string(&file).unwrap()).unwrap();
}

/// Rewrite the configuration file at `path` so that its `[phases]` table
/// holds `phases` and nothing else.
fn rephased(path: &str, phases: &[&str]) {
    edited(path, |file| {
        file.insert("phases".to_string(), table(phases).into());
    });
}

/// A configuration file written for this test: the prompted example of
/// `G` with the `[models.qwen]` table's provider replaced by one at
/// `base_url`, its model by `id`, and `lines` added to it, and the game's
/// [`Tested::PHASES`], in the target directory, at a path of its own so the
/// tests do not collide.
fn configured<G: Tested>(test: &str, base_url: &str, id: &str, lines: &[&str]) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::copy(example(G::KEY, "prompted.toml"), &path).unwrap();
    edited(&path, |file| {
        let qwen = file["models"]["qwen"].as_table_mut().unwrap();
        for key in ["base_url", "id"] {
            assert!(
                qwen.contains_key(key),
                "the example's [models.qwen] has no {key}"
            );
        }
        qwen.insert("base_url".to_string(), base_url.into());
        qwen.insert("id".to_string(), id.into());
        qwen.extend(table(lines));
        file.insert("phases".to_string(), table(G::PHASES).into());
    });
    path
}

/// How each day in `events` ended, in order.
fn endings(events: &[Event<Entry>]) -> Vec<Ending> {
    let endings: Vec<Ending> = events
        .iter()
        .filter_map(|event| match &event.payload {
            Entry::Event(Outcome::Voted { by, .. }) => Some(*by),
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
const QUICK: [&str; 3] = [
    "night_limit = \"1s\"",
    "day_limit = \"2s\"",
    "ratify_limit = \"1s\"",
];

#[test]
fn the_context_window_the_provider_reports_is_used_unless_the_file_gives_one() {
    let base_url = playing_within(&[EXAMPLE_MODEL], 32768);
    let config = configured::<Werewolf>("window", &base_url, EXAMPLE_MODEL, &[]);
    let args = [config.as_str()];
    let told = "qwen's context window is 32768 tokens, as the provider reports.";
    let (events, _) = assert_plays_the_default_table(&args, &[told]);
    assert_eq!(
        configuration(&events).models["qwen"].context_window,
        ContextWindow {
            tokens: 32768,
            source: Source::Provider,
        }
    );
    let config = configured::<Werewolf>(
        "window-file",
        &base_url,
        EXAMPLE_MODEL,
        &["context_window = 8192"],
    );
    let args = [config.as_str()];
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
    let config = configured::<Werewolf>("forgetting", &base_url, EXAMPLE_MODEL, &[&window]);
    let args = [config.as_str()];
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
    let config = configured::<Werewolf>(
        "window-small",
        &base_url,
        EXAMPLE_MODEL,
        &["context_window = 1500"],
    );
    let stderr = refused(&["play", &config]);
    assert!(
        stderr.contains("[models.qwen] context_window is 1500 tokens, smaller than the"),
        "{stderr}"
    );
}

#[test]
fn a_context_window_of_0_is_refused_before_anything_is_printed() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured::<Werewolf>(
        "window-0",
        &base_url,
        EXAMPLE_MODEL,
        &["context_window = 0"],
    );
    let stderr = refused(&["play", &config]);
    assert!(
        stderr
            .contains("[models.qwen] context_window is 0; a context window holds at least 1 token"),
        "{stderr}"
    );
}

#[test]
fn a_game_of_model_players_is_logged_from_the_configuration_to_the_result() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured::<Werewolf>("plays", &base_url, EXAMPLE_MODEL, &[]);
    let args = [config.as_str()];
    let (events, printed) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW]);
    let (roles, strategies) = start(&events);
    assert!(strategies.values().all(|strategy| strategy == "everyone"));
    assert_eq!(started(&events).ratify_limit, Duration::from_secs(1));
    let configuration = configuration(&events);
    let model = &configuration.models["qwen"];
    assert_eq!(model.base_url, base_url);
    assert_eq!(model.id, EXAMPLE_MODEL);
    assert_eq!(model.tool_choice, ToolChoice::Required);
    assert_eq!(configuration.limits.night_limit, Duration::from_secs(1));
    assert_eq!(configuration.limits.ratify_limit, Duration::from_secs(1));
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
            "game = \"werewolf\"
[phases]
{}
[models.qwen]
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
",
            QUICK.join("\n")
        ),
    )
    .unwrap();
    let args = [path.as_str()];
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
    let config = configured::<Werewolf>("retracts", &base_url, EXAMPLE_MODEL, &[]);
    // Each change of the tentative decision early in a day restarts the
    // ratify timer, and on a slow machine the model calls behind them take
    // a while, so the day limit leaves the votes ample time to stand.
    rephased(
        &config,
        &[
            "night_limit = \"1s\"",
            "day_limit = \"10s\"",
            "ratify_limit = \"1s\"",
        ],
    );
    let args = [config.as_str()];
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
                message: Message::Move(Move::Retract { .. }),
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
    let config = configured::<Werewolf>("auto", &base_url, EXAMPLE_MODEL, &[]);
    let args = [config.as_str()];
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
        "game = \"werewolf\"\nstrategy = \"plain\"\n[models.m]\nbase_url = \"http://localhost:1234/v1\"\nid = \"m\"\n\
         [strategies.plain]\npolicy = \"prompted\"\nsystem = \"{{ rulez }}\"\n",
    )
    .unwrap();
    let stderr = refused(&["play", &path]);
    for expected in ["plain", "Player 1", "werewolf", "rulez"] {
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn a_file_without_a_ratify_limit_plays_days_that_are_not_ratified() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured::<Werewolf>("unratified", &base_url, EXAMPLE_MODEL, &[]);
    rephased(&config, &["night_limit = \"1s\"", "day_limit = \"2s\""]);
    let args = [config.as_str()];
    let (events, printed) = assert_plays_the_default_table(&args, &[DEFAULT_WINDOW]);
    assert_eq!(started(&events).ratify_limit, Duration::ZERO);
    assert_eq!(configuration(&events).limits.ratify_limit, Duration::ZERO);
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
    let config = configured::<Werewolf>("ratify", &base_url, EXAMPLE_MODEL, &[]);
    rephased(&config, &["day_limit = \"10s\"", "ratify_limit = \"20s\""]);
    let stderr = refused(&["play", &config]);
    assert_invalid(&stderr, &config);
    assert!(
        stderr.contains("the ratify limit (20s) is longer than the day limit (10s)"),
        "{stderr}"
    );
}

#[test]
fn a_configuration_file_that_cannot_be_read_is_an_error_naming_it() {
    let stderr = refused(&["play", "no-such-file.toml"]);
    assert_invalid(&stderr, "no-such-file.toml");
    assert!(
        stderr.contains("social-deduction configs --write ."),
        "{stderr}"
    );
}

/// Run the command in `dir`, which must exit well; what it printed.
fn printed_in(dir: &Path, args: &[&str]) -> String {
    let output = run_in(dir, args);
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
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
    let paths: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        paths,
        [
            "avalon/prompted.toml",
            "avalon/random.toml",
            "werewolf/prompted.toml",
            "werewolf/random.toml",
        ],
        "{listing}"
    );
    assert!(
        listing.contains("The least a model-played game needs"),
        "{listing}"
    );
}

#[test]
fn configs_writes_the_examples_where_they_play_with_nothing_but_the_binary() {
    let dir = emptied("configs-write");
    assert_eq!(printed_in(&dir, &["configs", "--write", "."]), "");
    for (game, name) in [("werewolf", "random.toml"), ("avalon", "prompted.toml")] {
        assert_eq!(
            std::fs::read(dir.join(game).join(name)).unwrap(),
            std::fs::read(example(game, name)).unwrap(),
            "{game}/{name}"
        );
    }
    let told = printed_in(&dir, &["play", "werewolf/random.toml", "--no-log"]);
    assert!(told.contains(" win after "), "{told}");
}

#[test]
fn a_model_that_plays_by_night_alone_is_refused_in_the_provider_s_words() {
    let base_url = refusing_days(&[EXAMPLE_MODEL]);
    let config = configured::<Werewolf>("nights-only", &base_url, EXAMPLE_MODEL, &[]);
    let stderr = refused(&["play", &config]);
    assert!(
        stderr.starts_with("Error: setting up the model qwen"),
        "{stderr}"
    );
    for said in [
        format!("{EXAMPLE_MODEL} did not play Werewolf"),
        "did not answer a day's request".to_string(),
        format!("POST {base_url}/chat/completions was answered 400"),
        day_refusal(EXAMPLE_MODEL),
    ] {
        assert!(stderr.contains(&said), "{stderr}");
    }
}

#[test]
fn a_model_its_provider_does_not_serve_is_refused() {
    let base_url = serving(&["llama-3.1-8b-instruct"]);
    let config = configured::<Werewolf>("unserved", &base_url, EXAMPLE_MODEL, &[]);
    let stderr = refused(&["play", &config]);
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
    let config = configured::<Werewolf>(test, qwen_url, qwen_id, &[]);
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
    let args = [config.as_str()];
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
    let stderr = refused(&["play", &config]);
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
        "POST {base_url}/chat/completions was answered 400 Bad Request: {}",
        refusal("zephyr-7b")
    );
    assert_eq!(
        reasons,
        [
            "! zephyr-7b".to_string(),
            "  zephyr-7b did not play Werewolf".to_string(),
            "  did not answer a night's request".to_string(),
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
            "  qwen2.5-7b-instruct did not play Werewolf",
            "  did not answer a day's request",
            &format!(
                "  POST {base_url}/chat/completions was answered 400 Bad Request: {}",
                day_refusal(EXAMPLE_MODEL)
            ),
        ],
        "{stderr}"
    );
}

#[test]
fn the_models_command_with_check_marks_a_model_that_plays_werewolf_but_not_avalon_as_not_playing() {
    let base_url = refusing_offers_of("vote", &[EXAMPLE_MODEL]);
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
            "  qwen2.5-7b-instruct did not play Avalon",
            "  did not answer as a voter",
            &format!(
                "  POST {base_url}/chat/completions was answered 400 Bad Request: {}",
                refusal(EXAMPLE_MODEL)
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

/// Every role the deal Avalon's log opens with seats, in seat order, each
/// played by the random strategy.
fn avalon_roles(events: &[Event<avalon::Entry>]) -> Vec<avalon::Role> {
    let (roles, strategies) = start(events);
    assert!(strategies.values().all(|strategy| strategy == "random"));
    let mut roles: Vec<avalon::Role> = roles.values().copied().collect();
    roles.sort_unstable();
    roles
}

#[test]
fn avalon_plays_the_default_table_to_the_end_and_tells_it() {
    let (events, printed) = played::<avalon::Entry>(&[&example("avalon", "random.toml")]);
    let episode = events[0].episode;
    assert!(events.iter().all(|event| event.episode == episode));
    use avalon::Role::{Assassin, Merlin, Minion, Servant};
    assert_eq!(
        avalon_roles(&events),
        [Merlin, Servant, Servant, Assassin, Minion]
    );
    let Some(avalon::Entry::Event(avalon::Outcome::End { winner, .. })) =
        events.last().map(|event| &event.payload)
    else {
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
        lines[8].starts_with("Quest 1, proposal 1 of 5. Player 1 ("),
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
fn avalon_plays_any_valid_table_under_the_limits_the_file_gives() {
    let config = written(
        "avalon-table",
        "game = \"avalon\"\n[phases]\nvote_limit = \"90s\"\n[roles.servant]\ncount = 3\n[roles.minion]\ncount = 2\n\
         [roles.percival]\ncount = 1\n[roles.morgana]\ncount = 1\n",
    );
    let (events, printed) = played::<avalon::Entry>(&[&config]);
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
fn avalon_refuses_an_invalid_table_saying_what_is_wrong() {
    let config = written(
        "avalon-no-merlin",
        "game = \"avalon\"\n[roles.merlin]\ncount = 0\n[roles.servant]\ncount = 3\n",
    );
    let stderr = refused(&["play", &config]);
    assert_invalid(&stderr, &config);
    assert!(
        stderr.contains("a table has Merlin exactly when it has an Assassin"),
        "{stderr}"
    );
    let config = written(
        "avalon-too-few",
        "game = \"avalon\"\n[roles.servant]\ncount = 0\n",
    );
    let stderr = refused(&["play", &config]);
    assert!(stderr.contains("5 to 10 players"), "{stderr}");
    assert_usage_error(&["play", "game.toml", "other.toml"]);
}

#[test]
fn avalon_logs_the_configuration_of_the_random_config_s_players() {
    let (events, printed) = played::<avalon::Entry>(&[&example("avalon", "random.toml")]);
    use avalon::Role::{Assassin, Merlin, Minion, Servant};
    assert_eq!(
        avalon_roles(&events),
        [Merlin, Servant, Servant, Assassin, Minion]
    );
    let configuration = configuration(&events);
    assert!(configuration.models.is_empty(), "{configuration:?}");
    assert!(configuration.prompts.is_empty(), "{configuration:?}");
    assert_eq!(configuration.role_counts[&Servant], 2);
    assert!(
        matches!(
            events.last().map(|event| &event.payload),
            Some(avalon::Entry::Event(avalon::Outcome::End { .. }))
        ),
        "{events:?}"
    );
    assert!(printed.starts_with("5 players:\n"), "{printed}");
}

/// A configuration file written for this test, holding `file`, in the
/// target directory, at a path of its own so the tests do not collide.
fn written(test: &str, file: &str) -> String {
    let path = format!("{}/{test}.toml", env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(&path, file).unwrap();
    path
}

#[test]
fn avalon_plays_a_table_of_random_evil_and_prompted_good_to_the_end() {
    let base_url = playing(&[EXAMPLE_MODEL]);
    let config = configured::<Avalon>("avalon-mixed", &base_url, EXAMPLE_MODEL, &[]);
    edited(&config, |file| {
        let roles = [
            "assassin.strategy = \"random\"",
            "minion.strategy = \"random\"",
        ];
        file.insert("roles".to_string(), table(&roles).into());
    });
    let (events, printed) = played::<avalon::Entry>(&[&config]);
    let (roles, strategies) = start(&events);
    for (seat, role) in roles {
        let expected = match role.team() {
            avalon::Team::Good => "everyone",
            avalon::Team::Evil => "random",
        };
        assert_eq!(strategies[seat], expected, "{seat}");
    }
    // Only the good players are told prompts, each its own.
    let configuration = configuration(&events);
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
        events.iter().any(|event| matches!(
            event.payload,
            avalon::Entry::Event(avalon::Outcome::End { .. })
        )),
        "{events:?}"
    );
    // The prompted players talk, and the account tells it.
    assert!(printed.contains(DEFAULT_WINDOW), "{printed}");
    assert!(
        printed.contains("says, \"I have nothing to add.\""),
        "{printed}"
    );
}

/// Play a game of `G` whose players all ask one model, and check that
/// every call a player makes logs the usage the provider reported, and
/// that the probe's requests log none. A call still underway when the game
/// ends logs nothing either.
fn assert_every_call_logs_its_usage_and_the_probe_none<G: Tested>() {
    let (base_url, requests) = recording(&[EXAMPLE_MODEL]);
    let test = format!("{}-usage", G::KEY);
    let config = configured::<G>(&test, &base_url, EXAMPLE_MODEL, &[]);
    let (events, _) = played::<game::Entry<G>>(&[&config]);
    let (roles, _) = start(&events);
    let usages: Vec<(&PlayerId, &serde_json::Value)> = events
        .iter()
        .filter_map(|event| match &event.payload {
            game::Entry::Usage { player, usage, .. } => Some((player, usage)),
            _ => None,
        })
        .collect();
    assert!(!usages.is_empty(), "{events:?}");
    for (player, usage) in &usages {
        assert!(roles.contains_key(*player), "{player}");
        assert_eq!(*usage, &reported());
    }
    assert!(usages.len() <= requests.lock().unwrap().len() - G::PROBED);
}

#[test]
fn every_call_a_player_makes_logs_its_usage_and_the_probe_logs_none_in_werewolf() {
    assert_every_call_logs_its_usage_and_the_probe_none::<Werewolf>();
}

#[test]
fn every_call_a_player_makes_logs_its_usage_and_the_probe_logs_none_in_avalon() {
    assert_every_call_logs_its_usage_and_the_probe_none::<Avalon>();
}

/// Play the random example of `G` with `--no-log`, and check that standard
/// error is left empty and the game is still told; and that `--log`, given
/// after it, puts the log back.
fn assert_no_log_leaves_standard_error_empty<G: Tested>() {
    let random = example(G::KEY, "random.toml");
    let output = succeeded(&["play", &random, "--no-log"]);
    assert!(output.stderr.is_empty(), "{output:?}");
    let printed = String::from_utf8(output.stdout).unwrap();
    let seated = format!("{} players:\n", G::SEATS);
    assert!(printed.starts_with(&seated), "{printed}");
    let (events, _) = played::<game::Entry<G>>(&[&random, "--no-log", "--log"]);
    assert_eq!(start(&events).0.len(), G::SEATS);
}

#[test]
fn no_log_leaves_standard_error_empty_and_the_game_is_still_told_in_werewolf() {
    assert_no_log_leaves_standard_error_empty::<Werewolf>();
}

#[test]
fn no_log_leaves_standard_error_empty_and_the_game_is_still_told_in_avalon() {
    assert_no_log_leaves_standard_error_empty::<Avalon>();
}

/// Each (role, strategy) pair a deal of `roles` and `strategies` deals, in
/// order.
fn pairs<R: ToString>(
    roles: &HashMap<PlayerId, R>,
    strategies: &HashMap<PlayerId, String>,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = roles
        .iter()
        .map(|(id, role)| (role.to_string(), strategies[id].clone()))
        .collect();
    pairs.sort();
    pairs
}

/// Play the random example of `G` for three episodes, and check that they
/// are logged one after another, each under an episode id of its own and
/// opening with its deal, that every deal gives the same pairs to the same
/// players, and that the episodes are told in turn, set off by a blank
/// line.
fn assert_episodes_play_in_turn<G: Tested>() {
    let random = example(G::KEY, "random.toml");
    let (events, printed) = played::<game::Entry<G>>(&[&random, "--episodes", "3"]);
    let mut episodes: Vec<_> = events.iter().map(|event| event.episode).collect();
    episodes.dedup();
    assert_eq!(episodes.len(), 3, "{episodes:?}");
    assert_eq!(HashSet::<_>::from_iter(&episodes).len(), 3, "{episodes:?}");
    let mut deals = Vec::new();
    for episode in episodes {
        let logged: Vec<_> = events
            .iter()
            .filter(|event| event.episode == episode)
            .cloned()
            .collect();
        let (roles, strategies) = start(&logged);
        let mut players: Vec<PlayerId> = roles.keys().cloned().collect();
        players.sort();
        deals.push((players, pairs(roles, strategies)));
    }
    assert!(deals.iter().all(|deal| *deal == deals[0]), "{deals:?}");
    assert_eq!(deals[0].0.len(), G::SEATS);
    let seated = format!("{} players:\n", G::SEATS);
    assert!(printed.starts_with(&seated), "{printed}");
    assert_eq!(printed.matches(&seated).count(), 3, "{printed}");
    assert_eq!(
        printed.matches(&format!("\n\n{seated}")).count(),
        2,
        "{printed}"
    );
}

#[test]
fn episodes_play_one_after_another_with_the_same_players_and_pairs_in_werewolf() {
    assert_episodes_play_in_turn::<Werewolf>();
}

#[test]
fn episodes_play_one_after_another_with_the_same_players_and_pairs_in_avalon() {
    assert_episodes_play_in_turn::<Avalon>();
}

#[test]
fn no_episodes_is_a_usage_error() {
    let random = example("werewolf", "random.toml");
    let stderr = assert_usage_error(&["play", &random, "--episodes", "0"]);
    assert!(stderr.contains("--episodes"), "{stderr}");
}

/// Check that a game of `G` whose model does not play when asked is
/// refused before anything is printed, naming the model, what it failed,
/// the request and the provider's words.
fn assert_refuses_a_model_that_does_not_play<G: Tested>() {
    let base_url = refusing(&[EXAMPLE_MODEL]);
    let test = format!("{}-refused", G::KEY);
    let config = configured::<G>(&test, &base_url, EXAMPLE_MODEL, &[]);
    let stderr = refused(&["play", &config]);
    assert!(
        stderr.starts_with("Error: setting up the model qwen"),
        "{stderr}"
    );
    for said in [
        format!("{EXAMPLE_MODEL} {}", G::REFUSED),
        format!("POST {base_url}/chat/completions was answered 400"),
        refusal(EXAMPLE_MODEL),
    ] {
        assert!(stderr.contains(&said), "{stderr}");
    }
}

#[test]
fn a_model_that_does_not_play_when_asked_is_refused_in_the_provider_s_words_in_werewolf() {
    assert_refuses_a_model_that_does_not_play::<Werewolf>();
}

#[test]
fn a_model_that_does_not_play_when_asked_is_refused_in_the_provider_s_words_in_avalon() {
    assert_refuses_a_model_that_does_not_play::<Avalon>();
}

/// Play a game of `G` from a random file holding `file` after its `game`
/// key, and check that it is dealt `counts` and logs `limits`, in its
/// start and in its configuration, and that the account tells the table
/// and `told`.
fn assert_takes_the_role_counts_and_the_limits_from_the_file<G: Tested>(
    file: &str,
    counts: &[(G::Role, usize)],
    limits: G::Limits,
    told: &str,
) {
    let test = format!("{}-counted", G::KEY);
    let config = written(&test, &format!("game = \"{}\"\n{file}", G::KEY));
    let (events, printed) = played::<game::Entry<G>>(&[&config]);
    let (roles, _) = start(&events);
    for (role, count) in counts {
        let dealt = roles.values().filter(|dealt| *dealt == role).count();
        assert_eq!(dealt, *count, "{role}");
    }
    let seats: usize = counts.iter().map(|(_, count)| count).sum();
    assert_eq!(roles.len(), seats);
    assert_eq!(*started(&events), limits);
    let configuration = configuration(&events);
    assert_eq!(configuration.limits, limits);
    for (role, count) in counts {
        assert_eq!(configuration.role_counts[role], *count, "{role}");
    }
    assert!(
        printed.starts_with(&format!("{seats} players:\n")),
        "{printed}"
    );
    assert!(printed.contains(told), "{printed}");
}

#[test]
fn werewolf_takes_the_role_counts_and_the_limits_from_the_file() {
    // The day is not ratified unless something says so.
    assert_takes_the_role_counts_and_the_limits_from_the_file::<Werewolf>(
        "[phases]\nnight_limit = \"5s\"\nday_limit = \"1m\"\n[roles.werewolf]\ncount = 1\n\
         [roles.villager]\ncount = 2\n[roles.doctor]\ncount = 0\n[roles.seer]\ncount = 0\n",
        &[
            (Role::Werewolf, 1),
            (Role::Villager, 2),
            (Role::Doctor, 0),
            (Role::Seer, 0),
        ],
        Limits {
            night_limit: Duration::from_secs(5),
            day_limit: Duration::from_secs(60),
            ratify_limit: Duration::ZERO,
        },
        "A night waits 5s and a day 1m.",
    );
}

#[test]
fn avalon_takes_the_role_counts_and_the_limits_from_the_file() {
    use avalon::Role::{Assassin, Merlin, Minion, Servant};
    assert_takes_the_role_counts_and_the_limits_from_the_file::<Avalon>(
        "[phases]\nvote_limit = \"2m\"\nquest_limit = \"3m\"\n[roles.servant]\ncount = 3\n\
         [roles.minion]\ncount = 2\n",
        &[(Merlin, 1), (Servant, 3), (Assassin, 1), (Minion, 2)],
        avalon::Rules {
            vote_limit: Duration::from_secs(120),
            quest_limit: Duration::from_secs(180),
            ..avalon::Rules::default()
        },
        "A proposal waits 1m, a vote 2m, a quest 3m",
    );
}
