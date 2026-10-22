//! The desktop app's settings, [`AppSettings`]: the models, and the
//! game's table, host, strategies and prompt parts. The app owns them. It
//! stores them in `settings.toml` with [`save`] and reads them back with
//! [`load`], [`validate`]s them field by field, and plays a game from the
//! configuration [`text`] they make, which is what `twenty-questions play`
//! reads.
//!
//! The game's settings are its configuration less its models. Its
//! configuration is made of them with the models the host and its
//! strategies name.

pub use twenty_questions_gui_types::settings::*;

use crate::config::{self, Format, GUESSES, HostConfig, NAMES, SHORTEST};
use free_agent_game::strategy::Prompted;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// The file the settings are stored in, in the app's config directory.
pub const FILE: &str = "settings.toml";

/// The example configuration the game's settings start from.
const EXAMPLE: &str = include_str!("../configs/prompted.toml");

/// A configuration file as the settings read and write it: every key the
/// file may hold, with its values as written.
#[derive(Debug, Serialize, Deserialize)]
struct Written {
    #[serde(default)]
    format: Format,
    #[serde(default = "guesses")]
    guesses: u64,
    #[serde(default = "time_limit")]
    time_limit: String,
    #[serde(default)]
    players: WrittenPlayers,
    host: HostConfig,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    models: BTreeMap<String, ModelSettings>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    prompt_parts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    strategies: BTreeMap<String, Prompted>,
}

/// The default `guesses`, for `serde`.
fn guesses() -> u64 {
    GUESSES
}

/// The default `time_limit`, as written, for `serde`.
fn time_limit() -> String {
    humantime::format_duration(config::TIME_LIMIT).to_string()
}

/// The `[players]` table as written.
#[derive(Debug, Serialize, Deserialize)]
struct WrittenPlayers {
    #[serde(default = "players")]
    count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strategy: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    strategies: BTreeMap<String, usize>,
}

impl Default for WrittenPlayers {
    fn default() -> Self {
        WrittenPlayers {
            count: players(),
            strategy: None,
            strategies: BTreeMap::new(),
        }
    }
}

/// The default `count`, for `serde`.
fn players() -> usize {
    config::PLAYERS
}

/// The configuration a game is played under, as written: the game's
/// settings in `app`, with the models the host and its strategies name.
fn written(app: &AppSettings) -> Written {
    let game = &app.game;
    let models = game
        .models_named()
        .into_iter()
        .filter_map(|name| Some((name.to_string(), app.models.get(name)?.clone())))
        .collect();
    let host = &game.host;
    let given = |text: &str| (!text.trim().is_empty()).then(|| text.to_string());
    Written {
        format: Format::NEWEST,
        guesses: game.guesses,
        time_limit: game.time_limit.trim().to_string(),
        players: WrittenPlayers {
            count: game.players,
            strategy: given(&game.strategy),
            strategies: game.allotments.clone(),
        },
        host: HostConfig {
            model: given(&host.model),
            object: given(&host.object).map(|object| object.trim().to_string()),
            temperature: host.temperature,
            categories: host.categories.clone(),
            pick: given(&host.pick),
            system: host.system.clone(),
        },
        models,
        prompt_parts: game.prompt_parts.clone(),
        strategies: game
            .strategies
            .iter()
            .map(|(name, strategy)| {
                let prompted = Prompted {
                    system: strategy.system.clone(),
                    model: strategy.model.clone(),
                };
                (name.clone(), prompted)
            })
            .collect(),
    }
}

/// The configuration file a game is played from: the game's settings in
/// `app`, with the models the host and its strategies name as its
/// `[models]`. `twenty-questions play` plays it as the app does.
pub fn text(app: &AppSettings) -> String {
    toml::to_string_pretty(&written(app)).expect("every setting is TOML")
}

/// The settings of the example configuration, with the model the host
/// and each strategy ask named, and its models.
fn example() -> (GameSettings, BTreeMap<String, ModelSettings>) {
    let settled = config::parse(EXAMPLE).expect("the example settles");
    let written: Written = toml::from_str(EXAMPLE).expect("the example parses");
    let strategy = written.players.strategy.unwrap_or_else(|| {
        let mut allotted = settled.allotments.keys();
        allotted
            .next()
            .expect("the example seats a strategy")
            .clone()
    });
    let host = settled.host;
    let game = GameSettings {
        players: written.players.count,
        guesses: written.guesses,
        time_limit: written.time_limit,
        strategy,
        allotments: written.players.strategies,
        host: HostSettings {
            model: host.model.unwrap_or_default(),
            object: host.object.unwrap_or_default(),
            temperature: host.temperature,
            categories: host.categories,
            pick: host.pick.unwrap_or_default(),
            system: host.system,
        },
        strategies: settled
            .strategies
            .into_iter()
            .map(|(name, prompted)| {
                let strategy = StrategySettings {
                    policy: Policy::Prompted,
                    model: prompted.model,
                    system: prompted.system,
                };
                (name, strategy)
            })
            .collect(),
        prompt_parts: written.prompt_parts,
    };
    (game, written.models)
}

/// The settings the game starts from, which **Restore defaults** brings
/// back: the example's, without its models.
pub fn game_defaults() -> GameSettings {
    example().0
}

/// The settings the app starts from: the example's, with its models.
pub fn defaults() -> AppSettings {
    let (game, models) = example();
    AppSettings {
        models,
        game,
        ..AppSettings::default()
    }
}

/// The `format` key of [`FILE`], the one key read first.
#[derive(Deserialize)]
struct Stored {
    #[serde(default)]
    #[expect(dead_code, reason = "a format is checked as it is read")]
    format: Format,
}

/// The settings as [`save`] writes them: the format first.
#[derive(Serialize)]
struct Saved<'a> {
    format: Format,
    #[serde(flatten)]
    app: &'a AppSettings,
}

/// The settings stored in the config directory `dir`, or, the first time,
/// the [`defaults`].
///
/// # Errors
///
/// When the stored settings do not parse, as when they are of a newer
/// [`Format`], placed at its line and column.
pub fn load(dir: &Path) -> anyhow::Result<AppSettings> {
    let Ok(text) = std::fs::read_to_string(dir.join(FILE)) else {
        return Ok(defaults());
    };
    toml::from_str::<Stored>(&text)?;
    Ok(toml::from_str(&text)?)
}

/// Store `app` in the config directory `dir`, made if it is not there:
/// written whole to a file beside [`FILE`], which then takes its place.
///
/// # Errors
///
/// When the directory or the file cannot be written.
pub fn save(app: &AppSettings, dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let saved = Saved {
        format: Format::NEWEST,
        app,
    };
    let text = toml::to_string_pretty(&saved).expect("every setting is TOML");
    let written = dir.join(format!("{FILE}.new"));
    std::fs::write(&written, text)?;
    std::fs::rename(written, dir.join(FILE))
}

/// What is wrong with each field of `app`. Valid settings have no error.
///
/// A model needs a base URL and an id, a context window of at least a
/// token, and a request timeout that is a duration. The table seats one to
/// twelve players, who have one to twenty guesses, and a game lasts a
/// duration of at least 30 seconds. The host and every strategy name a
/// model that exists, the strategies the seats play exist, and the host
/// has an object or a prompt to pick one with, at a temperature from 0 to
/// 2. Settings with none of those wrong are settled as a configuration
/// file is, every prompt rendered, and what that finds is at the prompt it
/// is about, or at the game.
pub fn validate(app: &AppSettings) -> Vec<FieldError> {
    let mut errors: Vec<FieldError> = app
        .models
        .iter()
        .flat_map(|(name, model)| model_errors(name, model))
        .collect();
    let named = app.game.models_named();
    let models_fine = !errors
        .iter()
        .any(|error| named.contains(error.path[1].as_str()));
    let game = game_errors(app);
    let settles = game.is_empty() && models_fine;
    errors.extend(game);
    if settles && let Err(settling) = config::parse(&text(app)).and_then(|s| s.prompts()) {
        errors.push(placed(&settling));
    }
    errors
}

/// What is wrong with the model `name`, `model`.
fn model_errors(name: &str, model: &ModelSettings) -> Vec<FieldError> {
    let at = |field: &str, message: String| FieldError::new(&["models", name, field], message);
    let mut errors = Vec::new();
    if model.base_url.trim().is_empty() {
        errors.push(at("base_url", format!("model '{name}' needs a base URL")));
    }
    if model.id.trim().is_empty() {
        errors.push(at("id", format!("model '{name}' needs a model id")));
    }
    if model.context_window == Some(0) {
        errors.push(at(
            "context_window",
            format!("model '{name}' needs a context window of at least 1 token"),
        ));
    }
    if let Some(timeout) = &model.request_timeout
        && humantime::parse_duration(timeout.trim()).is_err()
    {
        errors.push(at(
            "request_timeout",
            format!("model '{name}' has a request timeout of '{timeout}', which isn't a duration"),
        ));
    }
    errors
}

/// What is wrong with the game's settings in `app`, field by field.
fn game_errors(app: &AppSettings) -> Vec<FieldError> {
    let game = &app.game;
    let mut errors = Vec::new();
    let mut error = |path: &[&str], message: String| {
        errors.push(FieldError::new(&[&[GAME], path].concat(), message));
    };
    if !(1..=NAMES.len()).contains(&game.players) {
        let message = format!("the table seats 1 to {} players", NAMES.len());
        error(&["players"], message);
    }
    if !(1..=GUESSES).contains(&game.guesses) {
        error(
            &["guesses"],
            format!("the players have 1 to {GUESSES} guesses"),
        );
    }
    match humantime::parse_duration(game.time_limit.trim()) {
        Err(_) => error(
            &["time_limit"],
            format!("'{}' isn't a duration, such as 10m", game.time_limit),
        ),
        Ok(limit) if limit < SHORTEST => error(
            &["time_limit"],
            format!(
                "a game lasts at least {}",
                humantime::format_duration(SHORTEST)
            ),
        ),
        Ok(_) => {}
    }
    let host = &game.host;
    if let Some(message) = model_error("the host", &host.model, app) {
        error(&["host", "model"], message);
    }
    if host.object.trim().is_empty() && host.pick.trim().is_empty() {
        let message = "the host needs a secret object, or a prompt to pick one".to_string();
        error(&["host", "object"], message);
    }
    if !(0.0..=2.0).contains(&host.temperature) {
        let message = "a temperature is from 0 to 2".to_string();
        error(&["host", "temperature"], message);
    }
    for (name, strategy) in &game.strategies {
        if let Some(message) = model_error(&format!("strategy '{name}'"), &strategy.model, app) {
            error(&["strategies", name, "model"], message);
        }
    }
    let seated = game.allotments.values().sum::<usize>() < game.players;
    if seated && !game.strategies.contains_key(&game.strategy) {
        let message = format!(
            "the strategy everyone plays, '{}', doesn't exist",
            game.strategy
        );
        error(&["strategy"], message);
    }
    for name in game.allotments.keys() {
        if !game.strategies.contains_key(name) {
            let message = format!("seats are allotted '{name}', which isn't a strategy");
            error(&["allotments", name], message);
        }
    }
    errors
}

/// What is wrong with the model `named` that `user` asks in `app`, if
/// anything: that it names none, or one that doesn't exist.
fn model_error(user: &str, named: &str, app: &AppSettings) -> Option<String> {
    if named.is_empty() {
        Some(format!("{user} names no model"))
    } else if !app.models.contains_key(named) {
        Some(format!("{user} names model '{named}', which doesn't exist"))
    } else {
        None
    }
}

/// `settling`, a configuration's error, at the field it is about: a
/// strategy's prompt, the host's, the pick, the prompt parts, or the game.
fn placed(settling: &anyhow::Error) -> FieldError {
    let message = format!("{settling:#}");
    let about = settling.to_string();
    let strategy = about
        .strip_prefix("rendering the prompt of strategy ")
        .and_then(|rest| rest.split_once(" for "))
        .map(|(name, _)| name);
    let path: Vec<&str> = match (strategy, about.as_str()) {
        (Some(name), _) => vec!["strategies", name, "system"],
        (None, "rendering the host's prompt") => vec!["host", "system"],
        (None, "rendering the host's pick") => vec!["host", "pick"],
        (None, about) if about.starts_with("[prompt_parts]") => vec!["prompt_parts"],
        (None, _) => vec![],
    };
    FieldError::new(&[&[GAME], &path[..]].concat(), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned;
    use crate::run::prepare;
    use free_agent_model::spec::Environment;
    use std::path::PathBuf;

    /// A directory of its own for the test `name`, empty and not yet made.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("twenty-questions-settings-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The only error `app` has, with its path.
    fn only_error(app: &AppSettings) -> (Vec<String>, String) {
        let errors = validate(app);
        assert_eq!(errors.len(), 1, "{errors:?}");
        (errors[0].path.clone(), errors[0].message.clone())
    }

    /// `path` as owned strings.
    fn path(path: &[&str]) -> Vec<String> {
        path.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_defaults_are_the_prompted_example_and_have_no_error() {
        let app = defaults();
        assert_eq!(app.models.keys().collect::<Vec<_>>(), ["qwen"]);
        assert_eq!(app.game, game_defaults());
        assert_eq!(app.game.players, 4);
        assert_eq!(app.game.time_limit, "10m");
        assert_eq!(app.game.strategy, "everyone");
        assert_eq!(app.game.host.model, "qwen");
        assert_eq!(app.game.host.object, "");
        assert_eq!(app.game.host.categories.len(), 8);
        assert_eq!(app.game.strategies["everyone"].model, "qwen");
        assert_eq!(app.game.prompt_parts.keys().collect::<Vec<_>>(), ["rules"]);
        assert_eq!(
            app.summary(),
            "4 players · qwen2.5-7b-instruct-1m · host picks"
        );
        assert!(validate(&app).is_empty(), "{:?}", validate(&app));
    }

    #[test]
    fn the_settings_round_trip_through_their_file() {
        let dir = scratch("round-trip");
        let mut app = defaults();
        app.general.appearance = Appearance::Dark;
        app.game.host.object = "a teapot".to_string();
        app.game.allotments.insert("everyone".to_string(), 1);
        save(&app, &dir).unwrap();
        let stored = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(stored.starts_with("format = 1\n"), "{stored}");
        assert_eq!(load(&dir).unwrap(), app);
        assert!(!dir.join(format!("{FILE}.new")).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_first_launch_starts_from_the_defaults() {
        let dir = scratch("first-launch");
        assert_eq!(load(&dir).unwrap(), defaults());
    }

    #[test]
    fn settings_that_cannot_be_read_are_an_error_and_cannot_be_saved_over_a_file() {
        let dir = scratch("unreadable");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), "general = 7\n").unwrap();
        assert!(load(&dir).is_err());
        assert!(save(&defaults(), &dir.join(FILE)).is_err());
        std::fs::write(dir.join(FILE), "format = 2\n[general]\n").unwrap();
        let shown = format!("{:#}", load(&dir).unwrap_err());
        assert!(shown.contains("configuration format 2"), "{shown}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_defaults_make_a_configuration_that_parses_and_prepares() {
        let mut app = defaults();
        for model in app.models.values_mut() {
            model.base_url = canned::playing(&[&model.id]);
        }
        let text = text(&app);
        let settled = config::parse(&text).unwrap_or_else(|e| panic!("{e:#}\n{text}"));
        assert_eq!(settled.seats, ["Alice", "Bob", "Carol", "Dave"]);
        assert_eq!(settled.host.object, None);
        prepare(&text, &Environment).await.unwrap();
    }

    #[test]
    fn the_configuration_names_only_the_models_in_use_and_the_secret_trimmed() {
        let mut app = defaults();
        app.models
            .insert("spare".to_string(), app.models["qwen"].clone());
        app.game.host.object = " a teapot ".to_string();
        app.game.strategy.clear();
        let text = text(&app);
        assert!(text.starts_with("format = 1\n"), "{text}");
        let settled = config::parse(&text).unwrap();
        assert_eq!(settled.models.keys().collect::<Vec<_>>(), ["qwen"]);
        assert_eq!(settled.host.object.as_deref(), Some("a teapot"));
        assert_eq!(settled.allotments["everyone"], 4);
    }

    #[test]
    fn a_model_needs_a_base_url_an_id_a_window_and_a_timeout_that_are_valid() {
        let mut app = defaults();
        let qwen = app.models.get_mut("qwen").unwrap();
        qwen.base_url = " ".to_string();
        qwen.id = String::new();
        qwen.context_window = Some(0);
        qwen.request_timeout = Some("soon".to_string());
        let errors = validate(&app);
        let paths: Vec<&str> = errors.iter().map(|e| e.path[2].as_str()).collect();
        assert_eq!(
            paths,
            ["base_url", "id", "context_window", "request_timeout"]
        );
        assert_eq!(
            errors[3].message,
            "model 'qwen' has a request timeout of 'soon', which isn't a duration"
        );
    }

    #[test]
    fn the_table_the_guesses_and_the_time_limit_are_in_range() {
        let mut app = defaults();
        app.game.players = 13;
        assert_eq!(
            only_error(&app),
            (
                path(&["game", "players"]),
                "the table seats 1 to 12 players".to_string()
            )
        );
        app.game.players = 4;
        app.game.guesses = 0;
        assert_eq!(only_error(&app).0, path(&["game", "guesses"]));
        app.game.guesses = 20;
        app.game.time_limit = "soon".to_string();
        assert_eq!(
            only_error(&app),
            (
                path(&["game", "time_limit"]),
                "'soon' isn't a duration, such as 10m".to_string()
            )
        );
        app.game.time_limit = "10s".to_string();
        assert_eq!(only_error(&app).1, "a game lasts at least 30s");
    }

    #[test]
    fn the_host_needs_a_model_an_object_or_a_pick_and_a_temperature_in_range() {
        let mut app = defaults();
        app.game.host.model.clear();
        assert_eq!(
            only_error(&app),
            (
                path(&["game", "host", "model"]),
                "the host names no model".to_string()
            )
        );
        app.game.host.model = "ghost".to_string();
        assert_eq!(
            only_error(&app).1,
            "the host names model 'ghost', which doesn't exist"
        );
        app.game.host.model = "qwen".to_string();
        app.game.host.pick = " ".to_string();
        assert_eq!(only_error(&app).0, path(&["game", "host", "object"]));
        app.game.host.object = "a teapot".to_string();
        assert!(validate(&app).is_empty());
        app.game.host.temperature = 2.5;
        assert_eq!(only_error(&app).0, path(&["game", "host", "temperature"]));
    }

    #[test]
    fn the_strategies_name_models_that_exist_and_the_seats_play_strategies_that_do() {
        let mut app = defaults();
        app.game.strategies.get_mut("everyone").unwrap().model = "ghost".to_string();
        assert_eq!(
            only_error(&app),
            (
                path(&["game", "strategies", "everyone", "model"]),
                "strategy 'everyone' names model 'ghost', which doesn't exist".to_string()
            )
        );
        let mut app = defaults();
        app.game.strategy = "missing".to_string();
        assert_eq!(
            only_error(&app),
            (
                path(&["game", "strategy"]),
                "the strategy everyone plays, 'missing', doesn't exist".to_string()
            )
        );
        app.game.allotments.insert("missing".to_string(), 4);
        assert_eq!(
            only_error(&app),
            (
                path(&["game", "allotments", "missing"]),
                "seats are allotted 'missing', which isn't a strategy".to_string()
            )
        );
    }

    #[test]
    fn a_game_whose_model_is_wrong_is_not_settled() {
        let mut app = defaults();
        app.models.get_mut("qwen").unwrap().id.clear();
        app.game.strategies.get_mut("everyone").unwrap().system = "{{ nothing }}".to_string();
        let errors = validate(&app);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].path, path(&["models", "qwen", "id"]));
    }

    #[test]
    fn a_prompt_that_does_not_render_is_an_error_at_that_prompt() {
        let broken = |edit: fn(&mut GameSettings)| {
            let mut app = defaults();
            edit(&mut app.game);
            only_error(&app)
        };
        let (at, message) = broken(|game| {
            game.strategies.get_mut("everyone").unwrap().system = "{{ nothing }}".to_string();
        });
        assert_eq!(at, path(&["game", "strategies", "everyone", "system"]));
        assert!(
            message.starts_with("rendering the prompt of strategy everyone for Alice"),
            "{message}"
        );
        let (at, _) = broken(|game| game.host.system = "{{ nothing }}".to_string());
        assert_eq!(at, path(&["game", "host", "system"]));
        let (at, _) = broken(|game| game.host.pick = "{{ nothing }}".to_string());
        assert_eq!(at, path(&["game", "host", "pick"]));
        let (at, message) = broken(|game| {
            game.prompt_parts.insert("name".to_string(), String::new());
        });
        assert_eq!(at, path(&["game", "prompt_parts"]));
        assert!(message.contains("template variable"), "{message}");
    }

    #[test]
    fn what_settling_finds_about_no_one_prompt_is_at_the_game() {
        let error = anyhow::anyhow!("something else");
        assert_eq!(placed(&error), FieldError::new(&["game"], "something else"));
    }
}
