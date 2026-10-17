//! The desktop app's settings, [`AppSettings`]: the models both games
//! share, and each game's table, phases, strategies and prompt parts. The
//! app owns
//! them. It stores them in `settings.toml` with [`save`] and reads them
//! back with [`load`], [`validate`]s them field by field, and plays a game
//! from the configuration [`text`] they make, which is what `social-deduction
//! play` reads. A configuration file comes in with [`Imported::parse`] and
//! [`import`], and goes out as [`text`].
//!
//! A game's settings are its configuration less its models. Its
//! configuration is made of them with the shared models its strategies
//! name, and [`settle`] settles it as a file would be, without writing it
//! out.

pub use social_deduction_stage::settings::*;

use crate::avalon::{self, Avalon};
use crate::config::{self, Settled};
use crate::game::Game;
use crate::strategy::Strategy;
use crate::werewolf::{self, Werewolf};
use minijinja::value::merge_maps;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use serde::{Deserialize, Serialize};
use social_deduction_stage::GameName;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::time::Duration;

/// The file the settings are stored in, in the app's config directory.
pub const FILE: &str = "settings.toml";

/// The example configuration each game's settings start from.
const EXAMPLES: [(GameName, &str); 2] = [
    (
        GameName::Avalon,
        include_str!("../configs/avalon/prompted.toml"),
    ),
    (
        GameName::Werewolf,
        include_str!("../configs/werewolf/prompted.toml"),
    ),
];

/// What the settings know of a game besides what every game has.
trait Tabled: Game {
    /// The game.
    const GAME: GameName;
    /// Its `[phases]` keys, in the order the phases come.
    const PHASES: &'static [&'static str];
    /// Its roles, in the order they are dealt.
    fn roles() -> Vec<Self::Role>;
    /// The settings a configuration `table` settles to.
    fn settle(table: toml::Table) -> anyhow::Result<config::Settings<Self>>;
    /// What is wrong with `limits` together, each by its key.
    fn limits_error(limits: &BTreeMap<&str, Duration>) -> Option<(&'static str, String)>;
}

impl Tabled for Werewolf {
    const GAME: GameName = GameName::Werewolf;
    const PHASES: &'static [&'static str] = &["night_limit", "day_limit", "ratify_limit"];

    fn roles() -> Vec<werewolf::Role> {
        werewolf::Table::default()
            .counts()
            .map(|(role, _)| role)
            .to_vec()
    }

    fn settle(table: toml::Table) -> anyhow::Result<werewolf::config::Settings> {
        werewolf::config::from_table(table)?.settle()
    }

    fn limits_error(limits: &BTreeMap<&str, Duration>) -> Option<(&'static str, String)> {
        let (day, ratify) = (limits.get("day_limit")?, limits.get("ratify_limit")?);
        let error = werewolf::config::within_the_day(*day, *ratify).err()?;
        Some(("ratify_limit", error.to_string()))
    }
}

impl Tabled for Avalon {
    const GAME: GameName = GameName::Avalon;
    const PHASES: &'static [&'static str] = &[
        "propose_limit",
        "vote_limit",
        "quest_limit",
        "assassinate_limit",
    ];

    fn roles() -> Vec<avalon::Role> {
        avalon::Table::default()
            .counts()
            .map(|(role, _)| role)
            .to_vec()
    }

    fn settle(table: toml::Table) -> anyhow::Result<avalon::config::Settings> {
        avalon::config::from_table(table)?.settle()
    }

    fn limits_error(_: &BTreeMap<&str, Duration>) -> Option<(&'static str, String)> {
        None
    }
}

/// A role's key: its name in lower case, `werewolf`, `merlin`.
fn role_key(role: impl ToString) -> String {
    role.to_string().to_lowercase()
}

/// A configuration file as the settings read and write it: every key a
/// game's file may hold, with its values as written.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Written {
    game: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strategy: Option<String>,
    #[serde(default)]
    phases: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    models: BTreeMap<String, ModelSettings>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    prompt_parts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    strategies: BTreeMap<String, WrittenStrategy>,
    #[serde(default)]
    roles: BTreeMap<String, WrittenRole>,
}

/// A `[strategies.<name>]` table as written.
#[derive(Debug, Default, Serialize, Deserialize)]
struct WrittenStrategy {
    policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

/// A `[roles.<role>]` table as written.
#[derive(Debug, Default, Serialize, Deserialize)]
struct WrittenRole {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strategies: Option<BTreeMap<String, usize>>,
}

/// The configuration a game of `game` is played under, as written: its
/// settings in `app`, with the shared models its strategies name.
fn written(app: &AppSettings, game: GameName) -> Written {
    let settings = app.game(game);
    let models = settings
        .models_named()
        .into_iter()
        .filter_map(|name| Some((name.to_string(), app.models.get(name)?.clone())))
        .collect();
    let strategies = settings
        .strategies
        .iter()
        .map(|(name, strategy)| {
            let written = match strategy.policy {
                Policy::Random => WrittenStrategy {
                    policy: "random".to_string(),
                    ..WrittenStrategy::default()
                },
                Policy::Prompted => WrittenStrategy {
                    policy: "prompted".to_string(),
                    model: Some(strategy.model.clone()),
                    system: Some(strategy.system.clone()),
                },
            };
            (name.clone(), written)
        })
        .collect();
    let roles = settings
        .roles
        .iter()
        .map(|role| {
            let written = WrittenRole {
                count: Some(role.count),
                strategy: role.strategy.clone(),
                strategies: (!role.strategies.is_empty()).then(|| role.strategies.clone()),
            };
            (role.key.clone(), written)
        })
        .collect();
    Written {
        game: game_key(game).to_string(),
        strategy: Some(settings.strategy.clone()),
        phases: settings
            .phases
            .iter()
            .map(|phase| (phase.key.clone(), phase.limit.clone()))
            .collect(),
        models,
        prompt_parts: settings.prompt_parts.clone(),
        strategies,
        roles,
    }
}

/// The configuration file a game of `game` is played from: its settings
/// in `app`, with the shared models its strategies name as its
/// `[models]`. `social-deduction play` plays it as the app does.
pub fn text(app: &AppSettings, game: GameName) -> String {
    toml::to_string_pretty(&written(app, game)).expect("every setting is TOML")
}

/// The settings a game of `game` takes effect under, settled from `app`
/// as [`config::parse`] settles the configuration [`text`] makes, without
/// writing it out.
///
/// # Errors
///
/// What settling the configuration finds wrong.
pub fn settle(app: &AppSettings, game: GameName) -> anyhow::Result<Settled> {
    let table = toml::Table::try_from(written(app, game))?;
    Ok(match game {
        GameName::Werewolf => Settled::Werewolf(Box::new(Werewolf::settle(table)?)),
        GameName::Avalon => Settled::Avalon(Box::new(Avalon::settle(table)?)),
    })
}

/// A configuration file read in: the game it names, its settings, and
/// its models.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The game the file names.
    pub game: GameName,
    /// Its settings.
    pub settings: GameSettings,
    /// Its models, by name.
    pub models: BTreeMap<String, ModelSettings>,
}

impl Imported {
    /// The configuration file `text`, parsed as [`config::parse`] parses
    /// it, so that its errors come back placed at their line and column.
    /// Each role has the seats the file settles to, each phase the limit
    /// the file writes or the game's own, and each prompted strategy the
    /// model it asks.
    ///
    /// # Errors
    ///
    /// When `text` is not a configuration file that settles.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let settled = config::parse(text)?;
        let written: Written = toml::from_str(text)?;
        Ok(match settled {
            Settled::Werewolf(settings) => imported(&settings, written),
            Settled::Avalon(settings) => imported(&settings, written),
        })
    }
}

/// The settings `written` makes, as `settled` settled them.
fn imported<G: Tabled>(settled: &config::Settings<G>, written: Written) -> Imported {
    let counts = settled.lineup.role_counts();
    let limits = serde_json::to_value(&settled.limits).expect("limits are JSON");
    let mut roles = written.roles;
    let roles = G::roles()
        .into_iter()
        .map(|role| {
            let key = role_key(role);
            let file = roles.remove(&key).unwrap_or_default();
            RoleSettings {
                count: counts[&role],
                strategy: file.strategy,
                strategies: file.strategies.unwrap_or_default(),
                key,
            }
        })
        .collect();
    let phases = G::PHASES
        .iter()
        .map(|key| PhaseSettings {
            key: key.to_string(),
            limit: written.phases.get(*key).cloned().unwrap_or_else(|| {
                limits[*key]
                    .as_str()
                    .expect("a limit is a string")
                    .to_string()
            }),
        })
        .collect();
    let strategies = settled
        .lineup
        .strategies
        .iter()
        .filter(|(name, _)| *name != RANDOM)
        .map(|(name, strategy)| {
            let settings = match strategy {
                Strategy::Random(_) => StrategySettings {
                    policy: Policy::Random,
                    ..StrategySettings::default()
                },
                Strategy::Prompted(prompted) => StrategySettings {
                    policy: Policy::Prompted,
                    model: prompted.model.clone(),
                    system: prompted.system.clone(),
                },
            };
            (name.clone(), settings)
        })
        .collect();
    Imported {
        game: G::GAME,
        settings: GameSettings {
            strategy: written.strategy.unwrap_or_else(|| RANDOM.to_string()),
            roles,
            phases,
            strategies,
            prompt_parts: written.prompt_parts,
        },
        models: written.models,
    }
}

/// Bring `imported` into `app`: its settings replace its game's, which
/// is chosen to play, and its models join the shared ones. A model whose
/// name is taken by another is added as `<name>-2`, or the next number
/// free, and the strategies that ask it are changed to match; one the
/// same as the model of its name is that model.
pub fn import(app: &mut AppSettings, imported: Imported) {
    let Imported {
        game,
        mut settings,
        models,
    } = imported;
    for (name, model) in models {
        let taken = unique(&name, |taken| {
            app.models.get(taken).is_some_and(|other| *other != model)
        });
        for strategy in settings.strategies.values_mut() {
            if strategy.policy == Policy::Prompted && strategy.model == name {
                strategy.model.clone_from(&taken);
            }
        }
        app.models.insert(taken, model);
    }
    *app.game_mut(game) = settings;
}

/// The settings of the example configuration of `game`, which **Restore
/// Defaults** brings back, and its models.
fn example(game: GameName) -> Imported {
    let (_, text) = EXAMPLES
        .into_iter()
        .find(|(example, _)| *example == game)
        .expect("every game has an example");
    Imported::parse(text).expect("every example settles")
}

/// The settings `game` starts from: its example's, without its models.
pub fn game_defaults(game: GameName) -> GameSettings {
    example(game).settings
}

/// The settings the app starts from: each game's example, with their
/// models.
pub fn defaults() -> AppSettings {
    let mut app = AppSettings::default();
    for (game, _) in EXAMPLES {
        import(&mut app, example(game));
    }
    app
}

/// The settings stored in the config directory `dir`, or, the first time,
/// the [`defaults`]. Settings that cannot be read are the defaults.
pub fn load(dir: &Path) -> AppSettings {
    match std::fs::read_to_string(dir.join(FILE)) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!("{FILE} could not be read, so the settings are the defaults: {error}");
            defaults()
        }),
        Err(_) => defaults(),
    }
}

/// Store `app` in the config directory `dir`, made if it is not there:
/// written whole to a file beside [`FILE`], which then takes its place.
///
/// # Errors
///
/// When the directory or the file cannot be written.
pub fn save(app: &AppSettings, dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = toml::to_string_pretty(app).expect("every setting is TOML");
    let written = dir.join(format!("{FILE}.new"));
    std::fs::write(&written, text)?;
    std::fs::rename(written, dir.join(FILE))
}

/// What is wrong with each field of `app`. Valid settings have no error.
///
/// A model needs a base URL and an id, a context window of at least a
/// token, and a request timeout that is a duration. A game's limits must
/// be durations that the game's rules allow together. A strategy may not
/// be named [`RANDOM`], must name a model that exists when it is prompted,
/// and its prompt must render for every role at the table, naming only
/// variables and prompt parts. The strategy everyone plays and each role's
/// must exist, and a prompt part is named by words of letters, digits and
/// underscores, and pulled in neither like a variable nor like another
/// part. Settings
/// with none of those wrong settle as the game's rules settle a table,
/// whose error is the roles'.
pub fn validate(app: &AppSettings) -> Vec<FieldError> {
    let mut errors = Vec::new();
    for (name, model) in &app.models {
        errors.extend(model_errors(name, model));
    }
    let models: Vec<FieldError> = errors.clone();
    errors.extend(game_errors::<Werewolf>(app, &models));
    errors.extend(game_errors::<Avalon>(app, &models));
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

/// What is wrong with the settings of `G` in `app`, where `models` is
/// what is wrong with the shared models.
fn game_errors<G: Tabled>(app: &AppSettings, models: &[FieldError]) -> Vec<FieldError> {
    let key = game_key(G::GAME);
    let settings = app.game(G::GAME);
    let mut errors = Vec::new();
    let mut error = |path: &[&str], message: String| {
        errors.push(FieldError::new(&[&[key], path].concat(), message));
    };
    let mut limits = BTreeMap::new();
    let mut durations = true;
    for phase in &settings.phases {
        match humantime::parse_duration(phase.limit.trim()) {
            Ok(limit) => {
                limits.insert(phase.key.as_str(), limit);
            }
            Err(_) => {
                durations = false;
                let message = format!("'{}' isn't a duration, such as 45s or 2m", phase.limit);
                error(&["phases", &phase.key], message);
            }
        }
    }
    if durations && let Some((phase, message)) = G::limits_error(&limits) {
        error(&["phases", phase], message);
    }
    let defined = |name: &str| name == RANDOM || settings.strategies.contains_key(name);
    let seated: Vec<G::Role> = G::roles()
        .into_iter()
        .filter(|role| {
            let key = role_key(role);
            settings.roles.iter().any(|r| r.key == key && r.count > 0)
        })
        .collect();
    let known = variables::<G>(&settings.prompt_parts);
    for (name, strategy) in &settings.strategies {
        if name == RANDOM {
            let message =
                format!("the strategy '{RANDOM}' is built in, so no other takes its name");
            error(&["strategies", name], message);
        }
        if strategy.policy == Policy::Random {
            continue;
        }
        let model = &strategy.model;
        if model.is_empty() {
            error(
                &["strategies", name, "model"],
                format!("strategy '{name}' names no model"),
            );
        } else if !app.models.contains_key(model) {
            let message = format!("strategy '{name}' names model '{model}', which doesn't exist");
            error(&["strategies", name, "model"], message);
        }
        if let Some(message) = prompt_error::<G>(
            name,
            &strategy.system,
            &settings.prompt_parts,
            &known,
            &seated,
        ) {
            error(&["strategies", name, "system"], message);
        }
    }
    if !defined(&settings.strategy) {
        let message = format!(
            "the strategy everyone plays, '{}', doesn't exist",
            settings.strategy
        );
        error(&["strategy"], message);
    }
    for role in &settings.roles {
        let title = label(&role.key);
        if let Some(strategy) = &role.strategy
            && !defined(strategy)
        {
            let message = format!("the {title}'s strategy, '{strategy}', doesn't exist");
            error(&["roles", &role.key, "strategy"], message);
        }
        for strategy in role.strategies.keys().filter(|name| !defined(name)) {
            let message = format!("the {title} is allotted '{strategy}', which isn't a strategy");
            error(&["roles", &role.key, "strategies", strategy], message);
        }
    }
    let variables = variables::<G>(&BTreeMap::new());
    let mut pulled = BTreeMap::new();
    for part in settings.prompt_parts.keys() {
        let variable = part_variable(part);
        let message = if !is_part_name(part) {
            format!(
                "the prompt part '{part}' needs a name of letters, digits, underscores and spaces"
            )
        } else if variables.contains(&variable) {
            format!("the prompt part '{part}' is named like a variable, which it can't be")
        } else if let Some(other) = pulled.insert(variable.clone(), part) {
            format!(
                "the prompt parts '{other}' and '{part}' are both pulled in with {{{{ {variable} }}}}"
            )
        } else {
            continue;
        };
        error(&["prompt_parts", part], message);
    }
    let named = settings.models_named();
    let models_fine = !models
        .iter()
        .any(|error| named.contains(error.path[1].as_str()));
    if errors.is_empty()
        && models_fine
        && let Err(settling) = settle(app, G::GAME)
    {
        errors.push(FieldError::new(&[key, "roles"], format!("{settling:#}")));
    }
    errors
}

/// What a prompt of a game of `G` is rendered with for `role`: the seat's
/// name, the role's variables, and each of `prompt_parts`, by its
/// [`part_variable`].
fn told<G: Tabled>(prompt_parts: &BTreeMap<String, String>, role: G::Role) -> Value {
    let parts: BTreeMap<String, String> = prompt_parts
        .iter()
        .map(|(name, text)| (part_variable(name), text.clone()))
        .collect();
    merge_maps([
        context! { name => "Player 1" },
        G::variables(role),
        Value::from(parts),
    ])
}

/// Every name a prompt of a game of `G` may use, for any role, with the
/// prompt parts `prompt_parts`.
fn variables<G: Tabled>(prompt_parts: &BTreeMap<String, String>) -> BTreeSet<String> {
    G::roles()
        .into_iter()
        .flat_map(|role| {
            told::<G>(prompt_parts, role)
                .try_iter()
                .into_iter()
                .flatten()
        })
        .filter_map(|key| key.as_str().map(String::from))
        .collect()
}

/// What is wrong with the prompt template `system` of the strategy `name`
/// in a game of `G` with the prompt parts `prompt_parts`, rendered for
/// each of `roles`, if anything is. A name the template uses that is none
/// of `known`, the variables and prompt parts, nor a global, is named.
fn prompt_error<G: Tabled>(
    name: &str,
    system: &str,
    prompt_parts: &BTreeMap<String, String>,
    known: &BTreeSet<String>,
    roles: &[G::Role],
) -> Option<String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    let template = match env.template_from_str(system) {
        Ok(template) => template,
        Err(error) => {
            return Some(format!(
                "the prompt of strategy '{name}' doesn't compile: {error}"
            ));
        }
    };
    let globals: BTreeSet<&str> = env.globals().map(|(global, _)| global).collect();
    let unknown = template
        .undeclared_variables(false)
        .into_iter()
        .filter(|variable| !known.contains(variable) && !globals.contains(variable.as_str()))
        .min();
    if let Some(unknown) = unknown {
        return Some(format!(
            "strategy '{name}' names '{unknown}', which is neither a prompt part nor a variable"
        ));
    }
    roles.iter().find_map(|role| {
        let error = template.render(told::<G>(prompt_parts, *role)).err()?;
        Some(format!(
            "the prompt of strategy '{name}' doesn't render: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned;
    use crate::model::spec::Environment;
    use crate::run::prepare;
    use std::path::PathBuf;

    /// A directory of its own for the test `name`, empty and not yet made.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("settings-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The error at `path` among `errors`, if there is one.
    fn at<'a>(errors: &'a [FieldError], path: &[&str]) -> Option<&'a str> {
        let path: Vec<String> = path.iter().map(ToString::to_string).collect();
        errors
            .iter()
            .find(|error| error.is_at(&path))
            .map(|error| error.message.as_str())
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
    fn the_defaults_are_each_game_s_prompted_example_and_have_no_error() {
        let app = defaults();
        assert_eq!(app.models.keys().collect::<Vec<_>>(), ["qwen"]);
        assert_eq!(app.werewolf, game_defaults(GameName::Werewolf));
        assert_eq!(app.avalon, game_defaults(GameName::Avalon));
        assert_eq!(
            app.summary(GameName::Werewolf),
            "7 players · 1 strategy · qwen2.5-7b-instruct-1m"
        );
        assert_eq!(
            app.summary(GameName::Avalon),
            "5 players · 1 strategy · qwen2.5-7b-instruct-1m"
        );
        let phases: Vec<(&str, &str)> = app
            .werewolf
            .phases
            .iter()
            .map(|phase| (phase.key.as_str(), phase.limit.as_str()))
            .collect();
        assert_eq!(
            phases,
            [
                ("night_limit", "1m"),
                ("day_limit", "1m"),
                ("ratify_limit", "20s")
            ]
        );
        assert!(validate(&app).is_empty(), "{:?}", validate(&app));
    }

    #[test]
    fn the_settings_round_trip_through_their_file() {
        let dir = scratch("round-trip");
        let mut app = defaults();
        app.general.appearance = Appearance::Dark;
        app.werewolf.roles[0]
            .strategies
            .insert("everyone".to_string(), 1);
        save(&app, &dir).unwrap();
        assert_eq!(load(&dir), app);
        assert!(!dir.join(format!("{FILE}.new")).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prompt_parts_stored_under_their_old_name_text_are_read() {
        let dir = scratch("text");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(FILE),
            "[werewolf.text]\nrules = \"Werewolves hunt.\"\n",
        )
        .unwrap();
        assert_eq!(
            load(&dir).werewolf.prompt_parts["rules"],
            "Werewolves hunt."
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_stored_with_a_logs_folder_are_read_without_it() {
        let dir = scratch("logs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(FILE),
            "[general]\nlogs = \"/tmp/logs\"\nappearance = \"dark\"\n",
        )
        .unwrap();
        assert_eq!(load(&dir).general.appearance, Appearance::Dark);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_that_cannot_be_read_are_the_defaults_and_cannot_be_saved_over_a_file() {
        let dir = scratch("unreadable");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), "general = 7\n").unwrap();
        assert_eq!(load(&dir), defaults());
        let file = dir.join(FILE);
        assert!(save(&defaults(), &file).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_first_launch_starts_from_the_defaults_whatever_else_is_there() {
        let dir = scratch("first-launch");
        assert_eq!(load(&dir), defaults());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("last.toml"),
            include_str!("../configs/avalon/random.toml"),
        )
        .unwrap();
        assert_eq!(load(&dir), defaults());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `app` with its models served by a canned provider that plays.
    fn served(mut app: AppSettings) -> AppSettings {
        for model in app.models.values_mut() {
            model.base_url = canned::playing(&[&model.id]);
        }
        app
    }

    #[tokio::test]
    async fn each_game_s_defaults_make_a_configuration_that_parses_and_prepares() {
        let app = served(defaults());
        for game in GameName::ALL {
            let text = text(&app, game);
            config::parse(&text).unwrap_or_else(|e| panic!("{e:#}\n{text}"));
            let prepared = prepare(&text, &Environment).await.unwrap();
            let prepared = match prepared.game() {
                config::Name::Werewolf => GameName::Werewolf,
                config::Name::Avalon => GameName::Avalon,
            };
            assert_eq!(prepared, game);
        }
    }

    #[test]
    fn a_game_s_settings_and_the_configuration_they_make_settle_the_same() {
        let mut app = defaults();
        app.werewolf.roles[1]
            .strategies
            .insert("everyone".to_string(), 2);
        app.werewolf.roles[1].strategy = Some(RANDOM.to_string());
        app.avalon.strategies.insert(
            "dice".to_string(),
            StrategySettings {
                policy: Policy::Random,
                ..StrategySettings::default()
            },
        );
        app.avalon.roles[2].strategy = Some("dice".to_string());
        app.models.get_mut("qwen").unwrap().request_timeout = Some("2m".to_string());
        for game in GameName::ALL {
            let structured = format!("{:?}", settle(&app, game).unwrap());
            let parsed = format!("{:?}", config::parse(&text(&app, game)).unwrap());
            assert_eq!(structured, parsed);
        }
    }

    #[test]
    fn exporting_then_importing_gives_back_the_same_settings() {
        let mut app = defaults();
        app.werewolf.phases[1].limit = "90s".to_string();
        app.werewolf.roles[0]
            .strategies
            .insert(RANDOM.to_string(), 1);
        app.werewolf.roles[0].strategy = Some("everyone".to_string());
        for game in GameName::ALL {
            let mut back = app.clone();
            import(&mut back, Imported::parse(&text(&app, game)).unwrap());
            assert_eq!(back, app, "{game:?}");
        }
    }

    #[test]
    fn an_imported_model_whose_name_is_taken_is_renamed_and_so_are_its_strategies_references() {
        let mut app = defaults();
        let mut file = text(&app, GameName::Avalon).replace("qwen2.5-7b-instruct-1m", "llama");
        file.push_str("[strategies.dice]\npolicy = \"random\"\n");
        import(&mut app, Imported::parse(&file).unwrap());
        assert_eq!(app.models["qwen"].id, "qwen2.5-7b-instruct-1m");
        assert_eq!(app.models["qwen-2"].id, "llama");
        assert_eq!(app.avalon.strategies["everyone"].model, "qwen-2");
        assert_eq!(app.werewolf.strategies["everyone"].model, "qwen");
        // Again, the next number; the same model again is that one.
        let third = file.replace("\"llama\"", "\"mistral\"");
        import(&mut app, Imported::parse(&third).unwrap());
        assert_eq!(app.models["qwen-3"].id, "mistral");
        import(&mut app, Imported::parse(&file).unwrap());
        assert_eq!(app.avalon.strategies["everyone"].model, "qwen-2");
        assert_eq!(app.models.len(), 3);
    }

    #[test]
    fn a_file_that_does_not_parse_is_not_imported_and_its_error_is_placed() {
        let error = Imported::parse("game = \"werewolf\"\n[roles\n").unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("line 2"), "{shown}");
    }

    #[test]
    fn a_model_needs_a_base_url_an_id_a_window_and_a_timeout_that_are_valid() {
        let mut app = defaults();
        app.models.insert(
            "blank".to_string(),
            ModelSettings {
                context_window: Some(0),
                request_timeout: Some("soon".to_string()),
                ..ModelSettings::default()
            },
        );
        let errors = validate(&app);
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert_eq!(
            at(&errors, &["models", "blank", "base_url"]),
            Some("model 'blank' needs a base URL")
        );
        assert_eq!(
            at(&errors, &["models", "blank", "id"]),
            Some("model 'blank' needs a model id")
        );
        assert_eq!(
            at(&errors, &["models", "blank", "context_window"]),
            Some("model 'blank' needs a context window of at least 1 token")
        );
        assert_eq!(
            at(&errors, &["models", "blank", "request_timeout"]),
            Some("model 'blank' has a request timeout of 'soon', which isn't a duration")
        );
    }

    #[test]
    fn a_limit_that_is_not_a_duration_is_an_error_at_its_phase() {
        let mut app = defaults();
        app.werewolf.phases[1].limit = "a while".to_string();
        assert_eq!(
            only_error(&app),
            (
                path(&["werewolf", "phases", "day_limit"]),
                "'a while' isn't a duration, such as 45s or 2m".to_string()
            )
        );
    }

    #[test]
    fn werewolf_s_ratify_limit_longer_than_its_day_is_an_error_at_the_ratify_limit() {
        let mut app = defaults();
        app.werewolf.phases[2].limit = "2m".to_string();
        assert_eq!(
            only_error(&app),
            (
                path(&["werewolf", "phases", "ratify_limit"]),
                "the ratify limit (2m) is longer than the day limit (1m)".to_string()
            )
        );
    }

    #[test]
    fn an_invalid_table_is_an_error_at_the_roles_by_the_game_s_own_rules() {
        let mut app = defaults();
        app.avalon.roles[0].count = 0;
        app.avalon.roles[2].count = 3;
        let (at, message) = only_error(&app);
        assert_eq!(at, path(&["avalon", "roles"]));
        assert!(
            message.contains("Merlin exactly when it has an Assassin"),
            "{message}"
        );
        // A table that allots more than the count.
        let mut app = defaults();
        app.werewolf.roles[0]
            .strategies
            .insert("everyone".to_string(), 3);
        assert_eq!(
            only_error(&app),
            (
                path(&["werewolf", "roles"]),
                "[roles.werewolf] count is 2, but [roles.werewolf.strategies] allots 3".to_string()
            )
        );
    }

    #[test]
    fn a_strategy_naming_a_model_that_does_not_exist_or_none_is_an_error_at_its_model() {
        let mut app = defaults();
        app.models.clear();
        let errors = validate(&app);
        assert_eq!(
            at(&errors, &["werewolf", "strategies", "everyone", "model"]),
            Some("strategy 'everyone' names model 'qwen', which doesn't exist")
        );
        let mut app = defaults();
        app.avalon
            .strategies
            .get_mut("everyone")
            .unwrap()
            .model
            .clear();
        assert_eq!(
            only_error(&app),
            (
                path(&["avalon", "strategies", "everyone", "model"]),
                "strategy 'everyone' names no model".to_string()
            )
        );
    }

    #[test]
    fn a_game_whose_model_is_wrong_is_not_settled() {
        let mut app = defaults();
        app.models.get_mut("qwen").unwrap().context_window = Some(0);
        let errors = validate(&app);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(app.errors_of(GameName::Werewolf, &errors).count(), 1);
    }

    #[test]
    fn a_prompt_naming_a_missing_prompt_part_or_that_does_not_render_is_an_error_at_its_prompt() {
        let system = |app: &mut AppSettings, text: &str| {
            app.werewolf.strategies.get_mut("everyone").unwrap().system = text.to_string();
        };
        let at_system = path(&["werewolf", "strategies", "everyone", "system"]);
        let mut app = defaults();
        system(&mut app, "{{ rulez }} {{ range(3) | length }}");
        assert_eq!(
            only_error(&app),
            (
                at_system.clone(),
                "strategy 'everyone' names 'rulez', which is neither a prompt part nor a variable"
                    .to_string()
            )
        );
        system(&mut app, "{% if %}");
        let (at, message) = only_error(&app);
        assert_eq!(at, at_system);
        assert!(
            message.starts_with("the prompt of strategy 'everyone' doesn't compile"),
            "{message}"
        );
        system(&mut app, "{{ rules.missing.deeper }}");
        let (at, message) = only_error(&app);
        assert_eq!(at, at_system);
        assert!(
            message.starts_with("the prompt of strategy 'everyone' doesn't render"),
            "{message}"
        );
    }

    #[test]
    fn strategies_that_do_not_exist_or_take_random_s_name_are_errors_where_they_are_named() {
        let mut app = defaults();
        app.werewolf.strategy = "nobody".to_string();
        app.werewolf.roles[3].strategy = Some("ghost".to_string());
        app.werewolf.roles[2]
            .strategies
            .insert("phantom".to_string(), 1);
        app.werewolf.strategies.insert(
            RANDOM.to_string(),
            StrategySettings {
                policy: Policy::Random,
                ..StrategySettings::default()
            },
        );
        let errors = validate(&app);
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert_eq!(
            at(&errors, &["werewolf", "strategy"]),
            Some("the strategy everyone plays, 'nobody', doesn't exist")
        );
        assert_eq!(
            at(&errors, &["werewolf", "roles", "seer", "strategy"]),
            Some("the Seer's strategy, 'ghost', doesn't exist")
        );
        assert_eq!(
            at(
                &errors,
                &["werewolf", "roles", "doctor", "strategies", "phantom"]
            ),
            Some("the Doctor is allotted 'phantom', which isn't a strategy")
        );
        assert_eq!(
            at(&errors, &["werewolf", "strategies", "random"]),
            Some("the strategy 'random' is built in, so no other takes its name")
        );
    }

    #[test]
    fn a_prompt_part_named_like_a_variable_is_an_error_at_the_part() {
        let mut app = defaults();
        app.avalon
            .prompt_parts
            .insert("team".to_string(), "Taken.".to_string());
        assert_eq!(
            only_error(&app),
            (
                path(&["avalon", "prompt_parts", "team"]),
                "the prompt part 'team' is named like a variable, which it can't be".to_string()
            )
        );
    }

    #[test]
    fn a_prompt_part_named_but_by_letters_digits_underscores_and_spaces_is_an_error_at_the_part() {
        let mut app = defaults();
        app.avalon
            .prompt_parts
            .insert("the-tone".to_string(), "Calm.".to_string());
        assert_eq!(
            only_error(&app),
            (
                path(&["avalon", "prompt_parts", "the-tone"]),
                "the prompt part 'the-tone' needs a name of letters, digits, underscores and spaces"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_prompt_part_with_spaces_is_pulled_in_with_underscores() {
        let mut app = defaults();
        app.werewolf
            .prompt_parts
            .insert("Seer only".to_string(), "Look.".to_string());
        app.werewolf
            .strategies
            .get_mut("everyone")
            .unwrap()
            .system
            .push_str("{{ Seer_only }}");
        assert!(validate(&app).is_empty(), "{:?}", validate(&app));
        app.werewolf
            .prompt_parts
            .insert("Seer_only".to_string(), "Look.".to_string());
        assert_eq!(
            only_error(&app),
            (
                path(&["werewolf", "prompt_parts", "Seer_only"]),
                "the prompt parts 'Seer only' and 'Seer_only' are both pulled in with \
                 {{ Seer_only }}"
                    .to_string()
            )
        );
    }
}
