//! What every app's settings share: the look, the models and the providers
//! they are reached through, the strategies that ask the models, and the
//! errors a field can have. An app's settings are [`Settings`]. The
//! backend stores them and checks them, and the Settings window edits
//! them, so they cross between the two as they are, in a [`Snapshot`].
//! The models each provider lists, what probing a model found and where
//! an API key is cross the same way. None of them ever carries a key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The strategy, where a strategy may be random, that is always defined,
/// with the random policy.
pub const RANDOM: &str = "random";

/// What every app's settings hold: the look, the models, by name, and the
/// providers added. The app says which of its strategies ask which model;
/// adding, renaming and removing models and providers is the same in
/// every app.
pub trait Settings {
    /// What belongs to no game.
    fn general(&self) -> &General;

    /// What belongs to no game, to change.
    fn general_mut(&mut self) -> &mut General;

    /// The models, by name, which the strategies choose from.
    fn models(&self) -> &BTreeMap<String, ModelSettings>;

    /// The models, to change.
    fn models_mut(&mut self) -> &mut BTreeMap<String, ModelSettings>;

    /// The providers added besides those known and found, in the order
    /// they were added.
    fn providers(&self) -> &[Provider];

    /// The providers added, to change.
    fn providers_mut(&mut self) -> &mut Vec<Provider>;

    /// Each strategy, or anything else, that asks a model, as the Models
    /// page names it, such as `Werewolf “everyone”` or `The host`, with
    /// the name of the model it asks, in the order the app shows them.
    fn model_users(&self) -> Vec<(String, &str)>;

    /// Make every strategy, or anything else, that names the model `old`
    /// name `new`.
    fn rename_model_users(&mut self, old: &str, new: &str);

    /// Add `model` as `name`, trimmed, unless that cannot name it.
    ///
    /// # Errors
    ///
    /// When the name is empty or another model has it.
    fn add_model(&mut self, name: &str, model: ModelSettings) -> Result<(), String> {
        let name = name.trim();
        name_problem(self.models(), name, "")?;
        self.models_mut().insert(name.to_string(), model);
        Ok(())
    }

    /// Rename the model `old` to `new`, trimmed, and every strategy that
    /// asks it with it, unless `new` cannot name it.
    ///
    /// # Errors
    ///
    /// When the name is empty or another model has it.
    fn rename_model(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        name_problem(self.models(), new, old)?;
        let Some(model) = self.models_mut().remove(old) else {
            return Ok(());
        };
        self.models_mut().insert(new.to_string(), model);
        self.rename_model_users(old, new);
        Ok(())
    }

    /// The strategies that ask the model `name`, as the Models page names
    /// them.
    fn users_of_model(&self, name: &str) -> Vec<String> {
        self.model_users()
            .into_iter()
            .filter(|(_, model)| *model == name)
            .map(|(strategy, _)| strategy)
            .collect()
    }

    /// Add `provider`, its name and base URL trimmed, the base URL's
    /// final slash too, unless it lacks a name or a whole base URL: an
    /// `http` or `https` URL ending in `/v1`.
    ///
    /// # Errors
    ///
    /// When it lacks a name or a whole base URL.
    fn add_provider(&mut self, provider: Provider) -> Result<(), String> {
        let name = provider.name.trim();
        let base_url = root(&provider.base_url);
        if name.is_empty() || !whole(base_url) {
            return Err("Give it a name and a base URL ending in /v1.".to_string());
        }
        let added = Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: provider.api_key_env,
        };
        self.providers_mut().push(added);
        Ok(())
    }

    /// The providers the Models page shows: those `found`, known and
    /// local, then those added, then the provider of each model's base URL
    /// that is none of them, as a local server when it is one on this
    /// machine, whether or not it is up, and otherwise named for its host.
    fn providers_shown(&self, found: &[Provider]) -> Vec<Provider> {
        let mut shown = found.to_vec();
        for provider in self.providers() {
            if provider_at(&shown, &provider.base_url).is_none() {
                shown.push(provider.clone());
            }
        }
        for model in self.models().values() {
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
    fn remove_provider(&mut self, base_url: &str) {
        let at = |url: &str| root(url) == root(base_url);
        self.providers_mut()
            .retain(|provider| !at(&provider.base_url));
        self.models_mut().retain(|_, model| !at(&model.base_url));
    }
}

/// Settings whose strategies ask the models, and whose system prompts
/// pull in prompt parts: each game's, where an app has several.
pub trait Strategies: Settings {
    /// Which strategies and prompt parts: a game's.
    type Of: Copy + Send + Sync + 'static;

    /// Whether a strategy may choose at random instead of asking a model.
    const MAY_BE_RANDOM: bool;

    /// Where the fields of `of` are: the path their paths start with,
    /// such as `["werewolf"]` or `["game"]`.
    fn at(of: Self::Of) -> Vec<String>;

    /// The strategies of `of`, by name.
    fn strategies(&self, of: Self::Of) -> &BTreeMap<String, StrategySettings>;

    /// The strategies of `of`, to change.
    fn strategies_mut(&mut self, of: Self::Of) -> &mut BTreeMap<String, StrategySettings>;

    /// The prompt parts of `of`: named pieces of text the system prompts
    /// pull in with their [`part_variable`].
    fn prompt_parts(&self, of: Self::Of) -> &BTreeMap<String, String>;

    /// The prompt parts of `of`, to change.
    fn prompt_parts_mut(&mut self, of: Self::Of) -> &mut BTreeMap<String, String>;

    /// What pulls a prompt part in, one and many: `strategy` and
    /// `strategies`.
    const PART_USERS: (&'static str, &'static str) = ("strategy", "strategies");

    /// Each prompt part of `of`, with the names of what pulls it in.
    fn part_users(&self, of: Self::Of) -> BTreeMap<String, Vec<String>>;
}

/// Why `name` cannot name one of `models` other than `except`: it is
/// empty, or another model has it.
fn name_problem(
    models: &BTreeMap<String, ModelSettings>,
    name: &str,
    except: &str,
) -> Result<(), String> {
    if name.is_empty() {
        return Err("Give it a name.".to_string());
    }
    if name != except && models.contains_key(name) {
        return Err("Another model already has that name.".to_string());
    }
    Ok(())
}

/// The settings as the backend holds them, and what is wrong with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, bound(deserialize = "S: Deserialize<'de> + Default"))]
pub struct Snapshot<S> {
    /// The settings.
    pub settings: S,
    /// Every field's error.
    pub errors: Vec<FieldError>,
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

/// A key as words, the first capitalized: `time_limit` as `Time limit`,
/// `host` as `Host`.
pub fn label(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Whether `name` can name a prompt part: words of letters, digits and
/// underscores with one space between them, not starting with a digit, so
/// that a template can pull it in with its [`part_variable`].
pub fn is_part_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_')
        && name.split(' ').all(|word| {
            !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The variable a template pulls the prompt part `name` in with: its name
/// with each space an underscore, so `House rules` is `{{ House_rules }}`.
pub fn part_variable(name: &str) -> String {
    name.replace(' ', "_")
}

/// What belongs to no game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// How the app looks.
    pub appearance: Appearance,
}

/// How the app looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the system looks, and as the game shows itself where it has a
    /// look of its own.
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

/// What is wrong with one field of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// Where the field is: `["models", "gpt", "id"]`.
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
pub fn host(base_url: &str) -> &str {
    let base_url = base_url.trim();
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

/// `base_url` as base URLs are compared: without spaces around it or a
/// final slash.
pub fn root(base_url: &str) -> &str {
    base_url.trim().trim_end_matches('/')
}

/// Whether `base_url` is whole: an `http` or `https` URL ending in `/v1`.
pub fn whole(base_url: &str) -> bool {
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
    /// How many models there are to probe: those not probed once the
    /// account failed are left for the next round.
    pub total: usize,
    /// Why the last of its models that could not be asked could not be,
    /// if one could not be.
    #[serde(default)]
    pub unchecked: Option<Unchecked>,
}

/// Why a model could not be asked to play, which says nothing about the
/// model, and what the provider said, or the error when it said nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unchecked {
    /// What the trouble was.
    pub trouble: Trouble,
    /// What the provider said, or the error.
    pub said: String,
}

/// What kept a model from being asked to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trouble {
    /// The account has no credits left.
    NoCredits,
    /// The provider did not accept the API key.
    KeyRefused,
    /// The API key is not allowed to use the provider's models.
    KeyForbidden,
    /// Anything else, such as a provider that could not be reached.
    Failed,
}

impl Unchecked {
    /// Whether every model of the provider would fail to be asked the
    /// same way, the account being the trouble.
    pub fn account_wide(&self) -> bool {
        self.trouble != Trouble::Failed
    }
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

    /// Why the model did not play, if it did not: the cause beneath the
    /// request it did not answer, as its probe words the reason, or all
    /// of the reason.
    pub fn cause(&self) -> Option<&str> {
        let Check::DidNotPlay(reason) = self else {
            return None;
        };
        let cause = reason
            .split_once(": did not answer ")
            .and_then(|(_, rest)| rest.split_once(": "))
            .map_or(reason.as_str(), |(_, cause)| cause);
        Some(cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app's settings whose strategies each ask a model by name, or,
    /// when random, name one without asking it.
    #[derive(Default)]
    struct Table {
        general: General,
        models: BTreeMap<String, ModelSettings>,
        providers: Vec<Provider>,
        strategies: BTreeMap<String, StrategySettings>,
    }

    impl Settings for Table {
        fn general(&self) -> &General {
            &self.general
        }

        fn general_mut(&mut self) -> &mut General {
            &mut self.general
        }

        fn models(&self) -> &BTreeMap<String, ModelSettings> {
            &self.models
        }

        fn models_mut(&mut self) -> &mut BTreeMap<String, ModelSettings> {
            &mut self.models
        }

        fn providers(&self) -> &[Provider] {
            &self.providers
        }

        fn providers_mut(&mut self) -> &mut Vec<Provider> {
            &mut self.providers
        }

        fn model_users(&self) -> Vec<(String, &str)> {
            self.strategies
                .iter()
                .filter(|(_, strategy)| strategy.policy == Policy::Prompted)
                .map(|(name, strategy)| (format!("“{name}”"), strategy.model.as_str()))
                .collect()
        }

        fn rename_model_users(&mut self, old: &str, new: &str) {
            for strategy in self.strategies.values_mut() {
                if strategy.model == old {
                    strategy.model = new.to_string();
                }
            }
        }
    }

    /// A strategy of `policy` naming `model`.
    fn strategy(policy: Policy, model: &str) -> StrategySettings {
        StrategySettings {
            policy,
            model: model.to_string(),
            system: String::new(),
        }
    }

    /// A model `id`.
    fn model(id: &str) -> ModelSettings {
        ModelSettings {
            id: id.to_string(),
            ..ModelSettings::default()
        }
    }

    /// Settings with the models `gpt` and `local`, asked by `smart` and
    /// `everyone`, and named by the random `dice`.
    fn two_models() -> Table {
        Table {
            models: BTreeMap::from([
                ("gpt".to_string(), model("gpt-4.1-mini")),
                ("local".to_string(), model("qwen")),
            ]),
            strategies: BTreeMap::from([
                ("everyone".to_string(), strategy(Policy::Prompted, "local")),
                ("smart".to_string(), strategy(Policy::Prompted, "gpt")),
                ("dice".to_string(), strategy(Policy::Random, "gpt")),
            ]),
            ..Table::default()
        }
    }

    #[test]
    fn a_model_is_renamed_and_the_strategies_that_name_it_follow() {
        let mut settings = two_models();
        settings.general_mut().appearance = Appearance::Dark;
        assert_eq!(settings.general().appearance, Appearance::Dark);
        assert_eq!(settings.rename_model("gpt", " opus "), Ok(()));
        assert!(settings.models.contains_key("opus") && !settings.models.contains_key("gpt"));
        assert_eq!(settings.strategies["smart"].model, "opus");
        assert_eq!(settings.strategies["dice"].model, "opus");
        assert_eq!(settings.strategies["everyone"].model, "local");
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
        let mut settings = two_models();
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
    fn a_model_s_users_are_the_strategies_that_ask_it() {
        let settings = two_models();
        assert_eq!(settings.users_of_model("gpt"), ["“smart”"]);
        assert_eq!(settings.users_of_model("local"), ["“everyone”"]);
        assert!(settings.users_of_model("nobody").is_empty());
    }

    #[test]
    fn a_provider_added_needs_a_name_and_a_whole_base_url_and_takes_its_models_when_removed() {
        let mut settings = two_models();
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
            settings.providers(),
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
        let settings = Table {
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
            ..Table::default()
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
    }

    #[test]
    fn a_snapshot_left_out_is_its_default() {
        let snapshot: Snapshot<General> = serde_json::from_str("{}").unwrap();
        assert_eq!(snapshot, Snapshot::default());
        let strategy: StrategySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(strategy.policy, Policy::Prompted);
    }

    #[test]
    fn the_look_holds_the_theme_or_leaves_it_to_the_system() {
        assert_eq!(Appearance::System.theme(), None);
        assert_eq!(Appearance::Light.theme(), Some("light"));
        assert_eq!(Appearance::Dark.theme(), Some("dark"));
        assert_eq!(General::default().appearance, Appearance::System);
    }

    #[test]
    fn a_field_error_is_at_its_path() {
        let error = FieldError::new(&["models", "gpt", "id"], "no id");
        assert_eq!(error.message, "no id");
        assert!(error.is_at(&["models", "gpt", "id"].map(String::from)));
        assert!(!error.is_at(&["models".to_string()]));
    }

    #[test]
    fn a_provider_is_found_by_its_base_url_with_or_without_a_final_slash() {
        let openai = Provider {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        };
        let providers = std::slice::from_ref(&openai);
        assert_eq!(
            provider_at(providers, " https://api.openai.com/v1/").map(|p| p.name.as_str()),
            Some("OpenAI")
        );
        assert_eq!(provider_at(providers, "https://api.example.com/v1"), None);
        assert_eq!(
            root(" https://api.openai.com/v1/ "),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn a_host_is_the_base_url_s_between_its_scheme_and_its_path() {
        assert_eq!(host(" https://api.example.com/v1/ "), "api.example.com");
        assert_eq!(host("localhost:1234/v1"), "localhost:1234");
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
        assert_eq!(no.cause(), Some("no tools"));
        assert_eq!(Check::Plays.cause(), None);
        let reason = "qwen did not play Chess: did not answer as a player: POST x: no: tools";
        assert_eq!(
            Check::DidNotPlay(reason.to_string()).cause(),
            Some("POST x: no: tools")
        );
    }

    #[test]
    fn only_a_failure_of_the_account_stops_a_provider_s_checks() {
        let unchecked = |trouble| Unchecked {
            trouble,
            said: String::new(),
        };
        for trouble in [
            Trouble::NoCredits,
            Trouble::KeyRefused,
            Trouble::KeyForbidden,
        ] {
            assert!(unchecked(trouble).account_wide(), "{trouble:?}");
        }
        assert!(!unchecked(Trouble::Failed).account_wide());
    }

    #[test]
    fn a_prompt_part_s_name_is_words_of_letters_digits_and_underscores() {
        for name in [
            "rules",
            "tone_2",
            "_aside",
            "Rules",
            "House rules",
            "the 2nd rule",
        ] {
            assert!(is_part_name(name), "{name}");
        }
        for name in [
            "",
            "2nd",
            " rules",
            "rules ",
            "the  rules",
            "tone-2",
            "règles",
        ] {
            assert!(!is_part_name(name), "{name}");
        }
    }

    #[test]
    fn a_prompt_part_is_pulled_in_by_its_name_with_underscores_for_spaces() {
        assert_eq!(part_variable("rules"), "rules");
        assert_eq!(part_variable("House rules"), "House_rules");
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
