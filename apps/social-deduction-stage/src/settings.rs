//! The desktop app's settings: the folder logs go in, the look, the models
//! both games share, and each game's table, phases, strategies and
//! prompt parts.
//! The backend stores them and checks them, and the Settings window edits
//! them, so they cross between the two as they are. The providers a model
//! is chosen from, the models each one lists, what probing a model found
//! and where an API key is cross the same way. None of them ever carries a
//! key.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::GameName;

/// The strategy that is always defined, with the random policy.
pub const RANDOM: &str = "random";

/// Everything the app's settings hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// What belongs to neither game.
    pub general: General,
    /// The models, by name, which both games' strategies choose from.
    pub models: BTreeMap<String, ModelSettings>,
    /// Werewolf's settings.
    pub werewolf: GameSettings,
    /// Avalon's settings.
    pub avalon: GameSettings,
    /// The providers added besides those known and found, in the order
    /// they were added.
    pub providers: Vec<Provider>,
}

impl AppSettings {
    /// The settings of `game`.
    pub fn game(&self, game: GameName) -> &GameSettings {
        match game {
            GameName::Werewolf => &self.werewolf,
            GameName::Avalon => &self.avalon,
        }
    }

    /// The settings of `game`, to change.
    pub fn game_mut(&mut self, game: GameName) -> &mut GameSettings {
        match game {
            GameName::Werewolf => &mut self.werewolf,
            GameName::Avalon => &mut self.avalon,
        }
    }

    /// One line saying what a game of `game` is: `7 players · 1 strategy
    /// · gpt-4.1-mini`, with the ids of the models in play, if any.
    pub fn summary(&self, game: GameName) -> String {
        let settings = self.game(game);
        let players = settings.players();
        let at_the_table = settings.at_the_table();
        let mut parts = vec![
            plural(players, "player", "players"),
            plural(at_the_table.len(), "strategy", "strategies"),
        ];
        let ids: BTreeSet<&str> = at_the_table
            .iter()
            .filter_map(|name| settings.model_of(name))
            .map(|name| self.models.get(name).map_or(name, |model| &model.id))
            .collect();
        if !ids.is_empty() {
            parts.push(ids.into_iter().collect::<Vec<_>>().join(", "));
        }
        parts.join(" · ")
    }

    /// The errors among `errors` that stop a game of `game`: its own, and
    /// those of the models its strategies name.
    pub fn errors_of<'a>(
        &self,
        game: GameName,
        errors: &'a [FieldError],
    ) -> impl Iterator<Item = &'a FieldError> + use<'a> {
        let key = game_key(game);
        let models: BTreeSet<String> = self
            .game(game)
            .models_named()
            .into_iter()
            .map(String::from)
            .collect();
        errors.iter().filter(move |error| match &error.path[..] {
            [first, ..] if first == key => true,
            [first, name, ..] if first == "models" => models.contains(name),
            _ => false,
        })
    }

    /// Why `name` cannot name a model other than `except`: it is empty, or
    /// another model has it.
    fn name_problem(&self, name: &str, except: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("Give it a name.".to_string());
        }
        if name != except && self.models.contains_key(name) {
            return Err("Another model already has that name.".to_string());
        }
        Ok(())
    }

    /// Add `model` as `name`, trimmed, unless that cannot name it.
    ///
    /// # Errors
    ///
    /// When the name is empty or another model has it.
    pub fn add_model(&mut self, name: &str, model: ModelSettings) -> Result<(), String> {
        let name = name.trim();
        self.name_problem(name, "")?;
        self.models.insert(name.to_string(), model);
        Ok(())
    }

    /// Rename the model `old` to `new`, trimmed, and every strategy of
    /// either game that asks it with it, unless `new` cannot name it.
    ///
    /// # Errors
    ///
    /// When the name is empty or another model has it.
    pub fn rename_model(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        self.name_problem(new, old)?;
        let Some(model) = self.models.remove(old) else {
            return Ok(());
        };
        self.models.insert(new.to_string(), model);
        for game in [&mut self.werewolf, &mut self.avalon] {
            for strategy in game.strategies.values_mut() {
                if strategy.model == old {
                    strategy.model = new.to_string();
                }
            }
        }
        Ok(())
    }

    /// The prompted strategies that ask the model `name`, each as its game
    /// and its name: `Werewolf “everyone”`.
    pub fn users_of_model(&self, name: &str) -> Vec<String> {
        [GameName::Werewolf, GameName::Avalon]
            .into_iter()
            .flat_map(|game| {
                let settings = self.game(game);
                settings
                    .strategies
                    .keys()
                    .filter(move |strategy| settings.model_of(strategy) == Some(name))
                    .map(move |strategy| format!("{} “{strategy}”", game_title(game)))
            })
            .collect()
    }

    /// Add `provider`, its name and base URL trimmed, the base URL's
    /// final slash too, unless it lacks a name or a whole base URL: an
    /// `http` or `https` URL ending in `/v1`.
    ///
    /// # Errors
    ///
    /// When it lacks a name or a whole base URL.
    pub fn add_provider(&mut self, provider: Provider) -> Result<(), String> {
        let name = provider.name.trim();
        let base_url = root(&provider.base_url);
        if name.is_empty() || !whole(base_url) {
            return Err("Give it a name and a base URL ending in /v1.".to_string());
        }
        self.providers.push(Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: provider.api_key_env,
        });
        Ok(())
    }

    /// The providers the Models tab shows: those `found`, known and local,
    /// then those added, then the provider of each model's base URL that
    /// is none of them, as a local server when it is one on this machine,
    /// whether or not it is up, and otherwise named for its host.
    pub fn providers_shown(&self, found: &[Provider]) -> Vec<Provider> {
        let mut shown = found.to_vec();
        for provider in &self.providers {
            if provider_at(&shown, &provider.base_url).is_none() {
                shown.push(provider.clone());
            }
        }
        for model in self.models.values() {
            if model.base_url.trim().is_empty() || provider_at(&shown, &model.base_url).is_some() {
                continue;
            }
            shown.push(match Provider::local_port(&model.base_url) {
                Some(port) => Provider::local(port),
                None => Provider {
                    name: host(&model.base_url).to_string(),
                    base_url: root(&model.base_url).to_string(),
                    api_key_env: model.api_key_env.clone(),
                },
            });
        }
        shown
    }

    /// Remove the provider added at `base_url`, and every model reached
    /// through it.
    pub fn remove_provider(&mut self, base_url: &str) {
        let at = |url: &str| root(url) == root(base_url);
        self.providers.retain(|provider| !at(&provider.base_url));
        self.models.retain(|_, model| !at(&model.base_url));
    }

    /// The first error among `errors` that stops a game of `game`, with
    /// the game's title before it, and where it is.
    pub fn first_error(&self, game: GameName, errors: &[FieldError]) -> Option<FieldError> {
        let error = self.errors_of(game, errors).next()?;
        Some(FieldError {
            path: error.path.clone(),
            message: format!("{}: {}", game_title(game), error.message),
        })
    }
}

/// `wanted`, or, when `taken` says it is taken, `wanted-2` or the next
/// number not taken.
pub fn unique(wanted: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = wanted.to_string();
    let mut number = 1;
    while taken(&name) {
        number += 1;
        name = format!("{wanted}-{number}");
    }
    name
}

/// A key as words, the first capitalized: `day_limit` as `Day limit`,
/// `merlin` as `Merlin`.
pub fn label(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Whether `name` can name a prompt part: letters, digits and
/// underscores, not starting with a digit, so that a template can pull it
/// in with `{{ name }}`.
pub fn is_part_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `count` and the noun for it, `one` or `many`.
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The key a game's settings are under, and its errors' paths start
/// with: `werewolf` or `avalon`.
pub fn game_key(game: GameName) -> &'static str {
    match game {
        GameName::Werewolf => "werewolf",
        GameName::Avalon => "avalon",
    }
}

/// A game's name as the app shows it: `Werewolf` or `Avalon`.
pub fn game_title(game: GameName) -> &'static str {
    match game {
        GameName::Werewolf => "Werewolf",
        GameName::Avalon => "Avalon",
    }
}

/// What belongs to neither game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// The folder game logs are saved in, when it is not the app's own.
    pub logs: Option<String>,
    /// How the app looks.
    pub appearance: Appearance,
    /// The game chosen to play.
    pub game: GameName,
}

impl Default for General {
    /// Logs in the app's folder, the system's look, and Werewolf.
    fn default() -> Self {
        Self {
            logs: None,
            appearance: Appearance::System,
            game: GameName::Werewolf,
        }
    }
}

/// How the app looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the system looks, and Werewolf's game dark by night and light by
    /// day.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Appearance {
    /// The page's `data-theme`, which the look holds to whatever the game
    /// shows, or `None` for the system's.
    pub fn theme(self) -> Option<&'static str> {
        match self {
            Appearance::System => None,
            Appearance::Light => Some("light"),
            Appearance::Dark => Some("dark"),
        }
    }
}

/// One model from one provider, and how to reach it: a configuration
/// file's `[models.<name>]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettings {
    /// The root of an OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The model's id, exactly as the provider lists it.
    pub id: String,
    /// The variable holding the API key, over the one known for the
    /// provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// The model's context window, in tokens, over what the provider
    /// reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// How long a request may take, such as `60s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<String>,
}

/// One game's settings: everything its configuration holds but the
/// models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// The strategy every role plays that is not allotted otherwise.
    pub strategy: String,
    /// The game's roles, in the order they are dealt.
    pub roles: Vec<RoleSettings>,
    /// The game's phase limits, in the order the phases come.
    pub phases: Vec<PhaseSettings>,
    /// The strategies, by name, besides [`RANDOM`].
    pub strategies: BTreeMap<String, StrategySettings>,
    /// Named pieces of text the system prompts pull in with `{{ name }}`,
    /// read also from settings stored when they were called `text`.
    #[serde(alias = "text")]
    pub prompt_parts: BTreeMap<String, String>,
}

impl Default for GameSettings {
    /// No roles, no phases, and everyone random.
    fn default() -> Self {
        Self {
            strategy: RANDOM.to_string(),
            roles: Vec::new(),
            phases: Vec::new(),
            strategies: BTreeMap::new(),
            prompt_parts: BTreeMap::new(),
        }
    }
}

impl GameSettings {
    /// How many players sit at the table.
    pub fn players(&self) -> usize {
        self.roles.iter().map(|role| role.count).sum()
    }

    /// The names of the strategies allotted a seat, as a game allots them:
    /// a role's `strategies` first, then its own strategy for the seats
    /// left, and a role with neither plays the game's strategy.
    pub fn at_the_table(&self) -> BTreeSet<&str> {
        let mut names = BTreeSet::new();
        for role in self.roles.iter().filter(|role| role.count > 0) {
            let allotted: usize = role.strategies.values().sum();
            names.extend(
                role.strategies
                    .iter()
                    .filter(|(_, seats)| **seats > 0)
                    .map(|(name, _)| name.as_str()),
            );
            match &role.strategy {
                Some(strategy) if role.count > allotted => {
                    names.insert(strategy.as_str());
                }
                None if role.strategies.is_empty() => {
                    names.insert(self.strategy.as_str());
                }
                _ => {}
            }
        }
        names
    }

    /// The names of the models the prompted strategies name.
    pub fn models_named(&self) -> BTreeSet<&str> {
        self.strategies
            .keys()
            .filter_map(|name| self.model_of(name))
            .collect()
    }

    /// Each prompt part, with the names of the prompted strategies whose
    /// system prompts use it. A system prompt that doesn't compile uses
    /// none.
    pub fn part_users(&self) -> BTreeMap<&str, Vec<&str>> {
        let mut users: BTreeMap<&str, Vec<&str>> = self
            .prompt_parts
            .keys()
            .map(|part| (part.as_str(), Vec::new()))
            .collect();
        let env = minijinja::Environment::new();
        for (name, strategy) in &self.strategies {
            if strategy.policy != Policy::Prompted {
                continue;
            }
            let Ok(template) = env.template_from_str(&strategy.system) else {
                continue;
            };
            for variable in template.undeclared_variables(false) {
                if let Some(users) = users.get_mut(variable.as_str()) {
                    users.push(name);
                }
            }
        }
        users
    }

    /// The name of the model the strategy `name` asks, if it is defined
    /// and prompted.
    pub fn model_of(&self, name: &str) -> Option<&str> {
        let strategy = self.strategies.get(name)?;
        (strategy.policy == Policy::Prompted).then_some(strategy.model.as_str())
    }
}

/// One role's seats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSettings {
    /// The role's key: `werewolf`, `merlin`.
    pub key: String,
    /// How many of the role sit at the table.
    pub count: usize,
    /// The strategy the role's seats play that `strategies` does not
    /// allot, over the game's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    /// How many seats of the role play each strategy, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub strategies: BTreeMap<String, usize>,
}

/// One phase's limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseSettings {
    /// The phase's key: `day_limit`.
    pub key: String,
    /// How long the phase waits, as written: `45s`, `2m`.
    pub limit: String,
}

/// A strategy: a policy and its parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategySettings {
    /// What the strategy's players do.
    pub policy: Policy,
    /// The name of the model a prompted strategy asks.
    pub model: String,
    /// A prompted strategy's system prompt template.
    pub system: String,
}

/// What a strategy's players do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Ask a language model.
    #[default]
    Prompted,
    /// Choose at random.
    Random,
}

/// What is wrong with one field of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// Where the field is: `["werewolf", "phases", "day_limit"]`,
    /// `["models", "gpt", "id"]`.
    pub path: Vec<String>,
    /// What is wrong, in a sentence that stands alone.
    pub message: String,
}

impl FieldError {
    /// The error `message` at `path`.
    pub fn new(path: &[&str], message: impl Into<String>) -> Self {
        Self {
            path: path.iter().map(ToString::to_string).collect(),
            message: message.into(),
        }
    }

    /// Whether the error is at `path`.
    pub fn is_at(&self, path: &[String]) -> bool {
        self.path == path
    }
}

/// The settings as the backend holds them, and what is wrong with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The settings.
    pub settings: AppSettings,
    /// Every field's error.
    pub errors: Vec<FieldError>,
    /// The folder game logs are saved in.
    pub logs: String,
}

/// A provider a model may be reached through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// What the app calls it: `OpenAI`, `Local (localhost:1234)`.
    pub name: String,
    /// The root of its OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The variable that conventionally holds its API key, if it wants one.
    pub api_key_env: Option<String>,
}

impl Provider {
    /// The local server on `port`: `Local (localhost:1234)`, wanting no
    /// key.
    pub fn local(port: u16) -> Self {
        Self {
            name: format!("Local (localhost:{port})"),
            base_url: format!("http://localhost:{port}/v1"),
            api_key_env: None,
        }
    }

    /// The port of the local server at `base_url`, if it is one.
    pub fn local_port(base_url: &str) -> Option<u16> {
        let rest = base_url.trim().strip_prefix("http://localhost:")?;
        rest.trim_end_matches('/').strip_suffix("/v1")?.parse().ok()
    }
}

/// The provider of `providers` at `base_url`, with or without a final
/// slash, if there is one.
pub fn provider_at<'a>(providers: &'a [Provider], base_url: &str) -> Option<&'a Provider> {
    providers
        .iter()
        .find(|provider| root(&provider.base_url) == root(base_url))
}

/// The host of `base_url`: `api.example.com` of
/// `https://api.example.com/v1`.
fn host(base_url: &str) -> &str {
    let base_url = base_url.trim();
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

/// `base_url` as base URLs are compared: without spaces around it or a
/// final slash.
fn root(base_url: &str) -> &str {
    base_url.trim().trim_end_matches('/')
}

/// Whether `base_url` is whole: an `http` or `https` URL ending in `/v1`.
fn whole(base_url: &str) -> bool {
    let base_url = root(base_url);
    (base_url.starts_with("http://") || base_url.starts_with("https://"))
        && base_url.ends_with("/v1")
}

/// Whether the provider at `base_url` is remote: reached over `https`.
pub fn is_remote(base_url: &str) -> bool {
    base_url.trim_start().starts_with("https://")
}

/// The models a provider lists, or why it could not list them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Listing {
    /// Each model the provider serves, in the order it lists them.
    pub models: Vec<Listed>,
    /// Why the provider could not list its models.
    pub error: Option<String>,
    /// What probing found of each model, by id.
    pub checks: BTreeMap<String, Check>,
}

/// One model a provider lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// Its id.
    pub id: String,
    /// The context window a game gives it unless its settings give one, in
    /// tokens: the one the provider reports, or the provider's default.
    pub context_window: u64,
    /// Whether its id says it is not a model that talks, such as an
    /// embedding model, which is never probed.
    pub hidden: bool,
}

/// What probing one model found, or `None` for a probe that could not be
/// run, sent as each probe ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// The provider's base URL.
    pub base_url: String,
    /// The model's id.
    pub id: String,
    /// What the probe found.
    pub check: Option<Check>,
}

/// How far probing a provider's models has got, sent as each probe ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The provider's base URL.
    pub base_url: String,
    /// How many models have been probed.
    pub checked: usize,
    /// How many models there are to probe.
    pub total: usize,
}

/// Where a key variable's key is, never the key itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyStatus {
    /// In the OS keychain.
    InKeychain,
    /// In the environment variable of its name.
    InEnvironment,
    /// Nowhere.
    Missing,
    /// The keychain could not be asked, and why.
    Unavailable(String),
}

impl KeyStatus {
    /// What a provider says of its key.
    pub fn told(&self) -> String {
        match self {
            KeyStatus::InKeychain => "in the keychain".to_string(),
            KeyStatus::InEnvironment => "from the environment".to_string(),
            KeyStatus::Missing => "not set".to_string(),
            KeyStatus::Unavailable(why) => why.clone(),
        }
    }

    /// Whether there is a key to send.
    pub fn is_set(&self) -> bool {
        matches!(self, KeyStatus::InKeychain | KeyStatus::InEnvironment)
    }
}

/// What asking a model to play found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    /// It plays.
    Plays,
    /// It plays only when not made to call a tool.
    PlaysUnforced,
    /// It did not play, and why.
    DidNotPlay(String),
}

impl Check {
    /// Whether the model plays, made to call a tool or not.
    pub fn plays(&self) -> bool {
        !matches!(self, Check::DidNotPlay(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A role `key` of `count` seats, playing `strategy` when it names one.
    fn role(key: &str, count: usize, strategy: Option<&str>) -> RoleSettings {
        RoleSettings {
            key: key.to_string(),
            count,
            strategy: strategy.map(String::from),
            strategies: BTreeMap::new(),
        }
    }

    /// A prompted strategy asking `model`.
    fn prompted(model: &str) -> StrategySettings {
        StrategySettings {
            policy: Policy::Prompted,
            model: model.to_string(),
            system: "Play.".to_string(),
        }
    }

    /// Settings whose Werewolf seats five players, two playing `smart`
    /// and three `everyone`, both prompted, asking `gpt` and `local`.
    fn two_strategies() -> AppSettings {
        let model = |id: &str| ModelSettings {
            id: id.to_string(),
            ..ModelSettings::default()
        };
        AppSettings {
            models: BTreeMap::from([
                ("gpt".to_string(), model("gpt-4.1-mini")),
                ("local".to_string(), model("qwen")),
            ]),
            werewolf: GameSettings {
                strategy: "everyone".to_string(),
                roles: vec![
                    role("werewolf", 2, Some("smart")),
                    role("villager", 3, None),
                ],
                strategies: BTreeMap::from([
                    ("everyone".to_string(), prompted("local")),
                    ("smart".to_string(), prompted("gpt")),
                    ("idle".to_string(), prompted("missing")),
                ]),
                ..GameSettings::default()
            },
            ..AppSettings::default()
        }
    }

    #[test]
    fn a_game_s_summary_counts_its_players_and_strategies_and_names_its_models() {
        let settings = two_strategies();
        assert_eq!(
            settings.summary(GameName::Werewolf),
            "5 players · 2 strategies · gpt-4.1-mini, qwen"
        );
        let mut settings = AppSettings::default();
        settings.avalon.roles = vec![role("merlin", 1, None)];
        assert_eq!(settings.summary(GameName::Avalon), "1 player · 1 strategy");
        // A model that is not defined is named as the strategy names it.
        settings.avalon.strategy = "lost".to_string();
        settings
            .avalon
            .strategies
            .insert("lost".to_string(), prompted("ghost"));
        assert_eq!(
            settings.summary(GameName::Avalon),
            "1 player · 1 strategy · ghost"
        );
    }

    #[test]
    fn a_role_s_strategies_come_first_and_its_own_strategy_fills_the_rest() {
        let mut settings = GameSettings::default();
        let mut servant = role("servant", 3, Some("filler"));
        servant.strategies = BTreeMap::from([("table".to_string(), 2), ("none".to_string(), 0)]);
        let mut minion = role("minion", 1, Some("unused"));
        minion.strategies = BTreeMap::from([("table".to_string(), 1)]);
        let mut oberon = role("oberon", 1, None);
        oberon.strategies = BTreeMap::from([("only".to_string(), 1)]);
        settings.roles = vec![servant, minion, oberon, role("morgana", 0, Some("absent"))];
        let names: Vec<&str> = settings.at_the_table().into_iter().collect();
        assert_eq!(names, ["filler", "only", "table"]);
    }

    #[test]
    fn a_game_s_errors_are_its_own_and_those_of_the_models_it_names() {
        let settings = two_strategies();
        let errors = [
            FieldError::new(&["werewolf", "phases", "day_limit"], "a"),
            FieldError::new(&["avalon", "phases", "vote_limit"], "b"),
            FieldError::new(&["models", "gpt", "id"], "c"),
            FieldError::new(&["models", "missing", "id"], "d"),
            FieldError::new(&["models", "other", "id"], "e"),
            FieldError::new(&["general"], "f"),
        ];
        let messages = |game| -> Vec<&str> {
            settings
                .errors_of(game, &errors)
                .map(|error| error.message.as_str())
                .collect()
        };
        assert_eq!(messages(GameName::Werewolf), ["a", "c", "d"]);
        assert_eq!(messages(GameName::Avalon), ["b"]);
        assert!(errors[0].is_at(&["werewolf", "phases", "day_limit"].map(String::from)));
        assert!(!errors[0].is_at(&["werewolf".to_string()]));
    }

    #[test]
    fn a_random_strategy_asks_no_model() {
        let mut settings = GameSettings::default();
        settings.strategies.insert(
            "dice".to_string(),
            StrategySettings {
                policy: Policy::Random,
                ..StrategySettings::default()
            },
        );
        assert_eq!(settings.model_of("dice"), None);
        assert_eq!(settings.model_of("missing"), None);
    }

    #[test]
    fn games_are_keyed_and_titled() {
        assert_eq!(game_key(GameName::Werewolf), "werewolf");
        assert_eq!(game_key(GameName::Avalon), "avalon");
        assert_eq!(game_title(GameName::Werewolf), "Werewolf");
        assert_eq!(game_title(GameName::Avalon), "Avalon");
        let mut settings = AppSettings::default();
        settings.game_mut(GameName::Avalon).strategy = "a".to_string();
        settings.game_mut(GameName::Werewolf).strategy = "w".to_string();
        assert_eq!(settings.game(GameName::Avalon).strategy, "a");
        assert_eq!(settings.game(GameName::Werewolf).strategy, "w");
    }

    #[test]
    fn the_look_holds_the_theme_or_leaves_it_to_the_system() {
        assert_eq!(Appearance::System.theme(), None);
        assert_eq!(Appearance::Light.theme(), Some("light"));
        assert_eq!(Appearance::Dark.theme(), Some("dark"));
        assert_eq!(General::default().appearance, Appearance::System);
    }

    #[test]
    fn settings_left_out_are_their_defaults() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AppSettings::default());
        assert_eq!(settings.werewolf.strategy, RANDOM);
        assert_eq!(settings.general.game, GameName::Werewolf);
    }

    #[test]
    fn a_model_is_renamed_and_the_strategies_that_ask_it_follow() {
        let mut settings = two_strategies();
        settings
            .avalon
            .strategies
            .insert("all".to_string(), prompted("gpt"));
        assert_eq!(settings.rename_model("gpt", " opus "), Ok(()));
        assert!(settings.models.contains_key("opus") && !settings.models.contains_key("gpt"));
        assert_eq!(settings.werewolf.strategies["smart"].model, "opus");
        assert_eq!(settings.avalon.strategies["all"].model, "opus");
        assert_eq!(settings.werewolf.strategies["everyone"].model, "local");
        assert_eq!(settings.rename_model("opus", "opus"), Ok(()));
        assert_eq!(
            settings.rename_model("opus", " "),
            Err("Give it a name.".to_string())
        );
        assert_eq!(
            settings.rename_model("opus", "local"),
            Err("Another model already has that name.".to_string())
        );
        assert_eq!(settings.rename_model("missing", "found"), Ok(()));
        assert!(!settings.models.contains_key("found"));
    }

    #[test]
    fn a_model_is_added_under_a_name_no_other_has() {
        let mut settings = two_strategies();
        let opus = ModelSettings {
            base_url: "https://api.anthropic.com/v1".to_string(),
            id: "claude-opus-5-5".to_string(),
            ..ModelSettings::default()
        };
        assert_eq!(
            settings.add_model("gpt", opus.clone()),
            Err("Another model already has that name.".to_string())
        );
        assert_eq!(
            settings.add_model("", opus.clone()),
            Err("Give it a name.".to_string())
        );
        assert_eq!(settings.add_model(" opus ", opus.clone()), Ok(()));
        assert_eq!(settings.models["opus"], opus);
    }

    #[test]
    fn a_model_s_users_are_the_prompted_strategies_that_ask_it() {
        let mut settings = two_strategies();
        settings
            .avalon
            .strategies
            .insert("all".to_string(), prompted("local"));
        assert_eq!(
            settings.users_of_model("local"),
            ["Werewolf “everyone”", "Avalon “all”"]
        );
        assert_eq!(settings.users_of_model("gpt"), ["Werewolf “smart”"]);
        assert!(settings.users_of_model("nobody").is_empty());
    }

    #[test]
    fn a_provider_added_needs_a_name_and_a_whole_base_url_and_takes_its_models_when_removed() {
        let mut settings = two_strategies();
        let example = |name: &str, base_url: &str| Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: Some("EXAMPLE_API_KEY".to_string()),
        };
        let wanted = "Give it a name and a base URL ending in /v1.";
        for (name, base_url) in [
            (" ", "https://api.example.com/v1"),
            ("Example", "api.example.com/v1"),
            ("Example", "https://api.example.com"),
        ] {
            assert_eq!(
                settings.add_provider(example(name, base_url)),
                Err(wanted.to_string())
            );
        }
        assert_eq!(
            settings.add_provider(example(" Example ", " https://api.example.com/v1/ ")),
            Ok(())
        );
        assert_eq!(
            settings.providers,
            [example("Example", "https://api.example.com/v1")]
        );
        settings.models.get_mut("gpt").unwrap().base_url =
            "https://api.example.com/v1/".to_string();
        settings.remove_provider("https://api.example.com/v1");
        assert!(settings.providers.is_empty());
        assert_eq!(settings.models.keys().collect::<Vec<_>>(), ["local"]);
    }

    #[test]
    fn the_providers_shown_are_those_found_then_added_then_each_other_a_model_names() {
        let openai = Provider {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        };
        assert_eq!(
            provider_at(std::slice::from_ref(&openai), " https://api.openai.com/v1/")
                .map(|p| p.name.as_str()),
            Some("OpenAI")
        );
        let added = Provider {
            name: "Example".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key_env: None,
        };
        let model = |base_url: &str, api_key_env: Option<&str>| ModelSettings {
            base_url: base_url.to_string(),
            api_key_env: api_key_env.map(String::from),
            ..ModelSettings::default()
        };
        let settings = AppSettings {
            providers: vec![added.clone(), openai.clone()],
            models: BTreeMap::from([
                ("a".to_string(), model("https://api.openai.com/v1/", None)),
                ("b".to_string(), model("http://localhost:9999/v1", None)),
                (
                    "c".to_string(),
                    model("https://other.example.org/v1/", Some("OTHER_KEY")),
                ),
                ("d".to_string(), model("", None)),
                ("e".to_string(), model("https://api.example.com/v1", None)),
            ]),
            ..AppSettings::default()
        };
        assert_eq!(
            settings.providers_shown(std::slice::from_ref(&openai)),
            [
                openai,
                added,
                Provider::local(9999),
                Provider {
                    name: "other.example.org".to_string(),
                    base_url: "https://other.example.org/v1".to_string(),
                    api_key_env: Some("OTHER_KEY".to_string()),
                },
            ]
        );
        assert_eq!(host("localhost"), "localhost");
    }

    #[test]
    fn a_base_url_is_whole_when_it_is_http_or_https_and_ends_in_v1() {
        assert!(whole("https://api.openai.com/v1"));
        assert!(whole(" http://localhost:1234/v1/ "));
        assert!(!whole("http://localhost:1234/"));
        assert!(!whole("localhost:1234/v1"));
        assert!(!whole(""));
    }

    #[test]
    fn a_key_s_status_says_where_it_is() {
        assert_eq!(KeyStatus::InKeychain.told(), "in the keychain");
        assert_eq!(KeyStatus::InEnvironment.told(), "from the environment");
        assert_eq!(KeyStatus::Missing.told(), "not set");
        assert_eq!(
            KeyStatus::Unavailable("no store".to_string()).told(),
            "no store"
        );
        assert!(KeyStatus::InKeychain.is_set() && KeyStatus::InEnvironment.is_set());
        assert!(!KeyStatus::Missing.is_set());
    }

    #[test]
    fn a_model_plays_whether_or_not_it_is_made_to_call_a_tool() {
        let no = Check::DidNotPlay("no tools".to_string());
        assert!(Check::Plays.plays() && Check::PlaysUnforced.plays() && !no.plays());
    }

    #[test]
    fn a_game_s_first_error_is_titled_with_the_game() {
        let settings = two_strategies();
        let errors = [FieldError::new(&["models", "gpt", "id"], "no id")];
        assert_eq!(
            settings.first_error(GameName::Werewolf, &errors),
            Some(FieldError::new(&["models", "gpt", "id"], "Werewolf: no id"))
        );
        assert_eq!(settings.first_error(GameName::Avalon, &errors), None);
    }

    #[test]
    fn a_prompt_part_s_name_is_letters_digits_and_underscores() {
        for name in ["rules", "tone_2", "_aside", "Rules"] {
            assert!(is_part_name(name), "{name}");
        }
        for name in ["", "2nd", "the rules", "tone-2", "règles"] {
            assert!(!is_part_name(name), "{name}");
        }
    }

    #[test]
    fn a_prompt_part_is_used_by_the_prompted_strategies_that_pull_it_in() {
        let mut settings = two_strategies().werewolf;
        let system = |settings: &mut GameSettings, name: &str, system: &str| {
            settings.strategies.get_mut(name).unwrap().system = system.to_string();
        };
        for part in ["rules", "ruleset", "tone", "rule", "aside"] {
            settings
                .prompt_parts
                .insert(part.to_string(), String::new());
        }
        system(&mut settings, "everyone", "{{ rules }}\nYou are {{name}}.");
        system(
            &mut settings,
            "smart",
            "{{rules|upper}} {% if tone %}{{ ruleset }}{% endif %}",
        );
        system(&mut settings, "idle", "{{ aside");
        settings.strategies.insert(
            "dumb".to_string(),
            StrategySettings {
                policy: Policy::Random,
                system: "{{ rules }}".to_string(),
                ..StrategySettings::default()
            },
        );
        let users = settings.part_users();
        let users: Vec<(&str, &[&str])> = users.iter().map(|(p, u)| (*p, &u[..])).collect();
        assert_eq!(
            users,
            [
                ("aside", &[][..]),
                ("rule", &[]),
                ("rules", &["everyone", "smart"]),
                ("ruleset", &["smart"]),
                ("tone", &["smart"]),
            ]
        );
    }

    #[test]
    fn a_name_taken_is_numbered() {
        let taken = ["gpt", "gpt-2"];
        assert_eq!(unique("gpt", |name| taken.contains(&name)), "gpt-3");
        assert_eq!(unique("claude", |name| taken.contains(&name)), "claude");
    }

    #[test]
    fn a_key_is_labeled_as_words() {
        assert_eq!(label("day_limit"), "Day limit");
        assert_eq!(label("merlin"), "Merlin");
        assert_eq!(label(""), "");
    }

    #[test]
    fn a_local_server_is_named_for_its_port_and_known_by_its_base_url() {
        let local = Provider::local(1234);
        assert_eq!(local.name, "Local (localhost:1234)");
        assert_eq!(local.base_url, "http://localhost:1234/v1");
        assert_eq!(local.api_key_env, None);
        assert_eq!(
            Provider::local_port("http://localhost:1234/v1/"),
            Some(1234)
        );
        assert_eq!(Provider::local_port("http://localhost:x/v1"), None);
        assert_eq!(Provider::local_port("http://localhost:1234"), None);
        assert_eq!(Provider::local_port("https://api.openai.com/v1"), None);
        assert!(is_remote(" https://api.openai.com/v1"));
        assert!(!is_remote("http://localhost:1234/v1"));
    }
}
