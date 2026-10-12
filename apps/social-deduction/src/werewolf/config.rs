//! The configuration of a game of Werewolf: the [`Config`] a TOML file is
//! parsed into, the [`Overrides`] the command line may give, and the
//! [`Settings`] that take effect once the two are combined with the
//! defaults in the code. A setting comes from the command line if it is
//! given there, otherwise from the file, otherwise from the code. With no
//! file, every player is random.
//!
//! The file defines named [`Model`]s and named [`Strategy`]s, each a policy
//! with its parameters, and allots each role's seats among the strategies.
//! The strategy [`RANDOM`] is always defined. [`Settings::deal`] deals the
//! (role, strategy) pairs the allotments make to the seats at random.
//!
//! A prompted player's system prompt is a `minijinja` template from its
//! strategy, rendered for its seat and role. [`Settings::prompts`] renders
//! every [`SystemPrompt`] a game could call for, so that a broken template
//! stops the program before any game begins.
//!
//! A model is in play when a prompted strategy that names it is allotted a
//! seat. [`Settings::set_up`] sets up each model in play before any actor
//! is built: it reads the key, settles the [`ContextWindow`] and asks the
//! model to play. The API key never appears in the file. The file names
//! the environment variable that holds it, and [`Model::api_key`] reads
//! that variable into a [`SecretString`], which is redacted wherever it is
//! shown.
//!
//! The log records the [`Configuration`] a game ran under right after the
//! deal, and [`players`] builds each seat's player from it.

use super::prompted::{self, Probed};
use super::{Dealt, PhaseLimits, PlayerId, Role, RoleCounts, Rules, Seat, Table, random};
use crate::model::{
    self, CONTEXT_WINDOW, Provider, REQUEST_TIMEOUT, ToolChoice, api_key, key_variable,
};
use anyhow::{Context as _, anyhow, bail};
use clap::Args;
use humantime::format_duration;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use rand::seq::SliceRandom;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, btree_map};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The name of the strategy that is always defined, with the random policy.
pub const RANDOM: &str = "random";

/// The variables every template has besides the text blocks.
const VARIABLES: [&str; 2] = ["name", "role"];

/// What the command line may say over the file: the role counts, the
/// phase limits and the ratify limit. Each is unset unless given, so that
/// the file can be heard.
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// How many of each role sit at the table.
    #[command(flatten)]
    pub roles: RoleCounts,
    /// How long each phase waits for the players.
    #[command(flatten)]
    pub limits: PhaseLimits,
    /// How long a day's votes must stand to be ratified; zero, the
    /// default, means a day is not ratified.
    #[arg(long, value_parser = humantime::parse_duration)]
    pub ratify_limit: Option<Duration>,
}

/// The configuration file, as written. Every table may be left out, and an
/// unknown key anywhere is an error, so a misspelled setting fails loudly.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The strategy every role not allotted otherwise plays, [`RANDOM`]
    /// unless it says.
    pub strategy: Option<String>,
    /// How long each phase waits for the players.
    #[serde(default)]
    pub phases: Phases,
    /// The models, by name, and how to reach each.
    #[serde(default)]
    pub models: BTreeMap<String, Model>,
    /// Named blocks of text for the templates to use.
    #[serde(default)]
    pub text: BTreeMap<String, String>,
    /// The strategies, by name, besides [`RANDOM`].
    #[serde(default)]
    pub strategies: BTreeMap<String, Strategy>,
    /// How many of each role there are, and which strategies they play.
    #[serde(default)]
    pub roles: Roles,
}

/// A `[models.<name>]` table: one model from one provider.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    /// The root of an OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The model's id, exactly as the provider lists it.
    pub id: String,
    /// The environment variable holding the API key, over the one known
    /// for the provider.
    pub api_key_env: Option<String>,
    /// How long a request may take.
    #[serde(default = "request_timeout", with = "humantime_serde")]
    pub request_timeout: Duration,
    /// The model's context window, in tokens, over what the provider
    /// reports.
    pub context_window: Option<u64>,
}

impl Model {
    /// The environment variable holding the API key: `api_key_env` when
    /// the file names one, else the one known for the provider, if any.
    pub fn key_variable(&self) -> Option<&str> {
        key_variable(&self.base_url, self.api_key_env.as_deref())
    }

    /// The API key, read from the variable [`Model::key_variable`] names.
    /// A variable that is named but not set is an error. When none is
    /// named there is no key, which is what a local server expects.
    pub fn api_key(&self) -> anyhow::Result<Option<SecretString>> {
        self.key_variable().map(api_key).transpose()
    }

    /// The provider the model is reached through, with its key read from
    /// the environment as [`Model::api_key`] reads it.
    pub fn provider(&self) -> anyhow::Result<Provider> {
        Provider::new(&self.base_url, self.api_key()?, self.request_timeout)
    }
}

/// The default `request_timeout`, for `serde`.
fn request_timeout() -> Duration {
    REQUEST_TIMEOUT
}

/// A `[strategies.<name>]` table: a policy, named by `policy`, with its
/// parameters, and only those.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "lowercase")]
pub enum Strategy {
    /// Select uniformly at random among the candidates, at once.
    Random(Random),
    /// Ask a language model.
    Prompted(Prompted),
}

/// The random policy's parameters: none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Random {}

/// The prompted policy's parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompted {
    /// The system prompt template.
    pub system: String,
    /// The name of the model asked, which may be left out when the file
    /// defines exactly one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Prompted {
    /// The name of the model this strategy asks, of `models`: the one it
    /// names, otherwise the only one.
    pub fn model_name<'a, M>(&'a self, models: &'a BTreeMap<String, M>) -> &'a str {
        self.model
            .as_ref()
            .or(models.keys().next())
            .map_or("", String::as_str)
    }
}

/// The text of the configuration file at `path`.
fn read(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// The `[phases]` table: how long each phase waits for the players, and
/// how long a day's votes must stand, for those that say.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phases {
    /// How long the night waits for a player.
    #[serde(default, with = "humantime_serde")]
    pub night_limit: Option<Duration>,
    /// How long the day waits for a player.
    #[serde(default, with = "humantime_serde")]
    pub day_limit: Option<Duration>,
    /// How long a day's votes must stand to be ratified.
    #[serde(default, with = "humantime_serde")]
    pub ratify_limit: Option<Duration>,
}

/// The `[roles]` tables, one per role. A role the file does not mention
/// has nothing said about it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    /// `[roles.werewolf]`.
    #[serde(default)]
    pub werewolf: RoleConfig,
    /// `[roles.villager]`.
    #[serde(default)]
    pub villager: RoleConfig,
    /// `[roles.doctor]`.
    #[serde(default)]
    pub doctor: RoleConfig,
    /// `[roles.seer]`.
    #[serde(default)]
    pub seer: RoleConfig,
}

impl Roles {
    /// What the file says about `role`.
    pub fn get(&self, role: Role) -> &RoleConfig {
        match role {
            Role::Werewolf => &self.werewolf,
            Role::Villager => &self.villager,
            Role::Doctor => &self.doctor,
            Role::Seer => &self.seer,
        }
    }
}

/// What the file says about one role.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// How many of the role sit at the table.
    pub count: Option<usize>,
    /// The strategy every seat of the role plays.
    pub strategy: Option<String>,
    /// How many seats of the role play each strategy, by name.
    pub strategies: Option<BTreeMap<String, usize>>,
}

/// How many seats of each role play each strategy, by name.
pub type Allotments = BTreeMap<Role, BTreeMap<String, usize>>;

/// The settings that took effect: the file as written, and the settings
/// the command line and the code have a say in, decided.
#[derive(Debug)]
pub struct Settings {
    /// The file as written.
    pub config: Config,
    /// How many of each role sit at the table.
    pub table: Table,
    /// How long the night waits for a player.
    pub night_limit: Duration,
    /// How long the day waits for a player.
    pub day_limit: Duration,
    /// How long a day's votes must stand to be ratified.
    pub ratify_limit: Duration,
    /// Every strategy, by name: the file's, and [`RANDOM`].
    pub strategies: BTreeMap<String, Strategy>,
    /// How many seats of each role play each strategy.
    pub allotments: Allotments,
}

impl Config {
    /// Read and parse the file at `path`. Reading it says nothing about
    /// the API keys, so a file can be checked without them being set.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&read(path)?).with_context(|| format!("in {}", path.display()))
    }

    /// Parse `file`, the text of a configuration file. A `[text]` block
    /// named like one of the template variables is an error, since the
    /// templates could not tell them apart, and so is a model's context
    /// window of 0, a strategy named [`RANDOM`], and a prompted strategy
    /// naming a model the file does not define, or naming none when the
    /// file does not define exactly one.
    pub fn parse(file: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(file)?;
        if let Some(taken) = VARIABLES.iter().find(|v| config.text.contains_key(**v)) {
            bail!("[text] has a block named {taken}, which is a template variable");
        }
        if let Some((name, _)) = config
            .models
            .iter()
            .find(|(_, model)| model.context_window == Some(0))
        {
            bail!("[models.{name}] context_window is 0; a context window holds at least 1 token");
        }
        if config.strategies.contains_key(RANDOM) {
            bail!("[strategies.{RANDOM}] is built in, and may not be defined");
        }
        for (name, strategy) in &config.strategies {
            if let Strategy::Prompted(prompted) = strategy {
                match &prompted.model {
                    Some(model) if !config.models.contains_key(model) => {
                        bail!("strategy {name} names the model {model}, which is not defined")
                    }
                    None if config.models.len() != 1 => bail!(
                        "strategy {name} names no model, which it may leave out only when the file defines exactly one; it defines {}",
                        config.models.len()
                    ),
                    _ => {}
                }
            }
        }
        Ok(config)
    }

    /// The settings that take effect with `overrides` from the command
    /// line: each limit from the command line if given there, otherwise
    /// from the file, otherwise from the code; and each role's allotment.
    /// A role allotted by a `strategies` table has as many seats as the
    /// table allots, and a count in the file or on the command line that
    /// disagrees is an error naming the role and both numbers. Any other
    /// role's seats all play one strategy, its own `strategy`, otherwise
    /// the file's, otherwise [`RANDOM`], and it has the count from the
    /// command line if given there, otherwise from the file, otherwise from
    /// the code. A strategy allotted that is not defined is an error naming
    /// the role. A ratify limit longer than the day limit is an error
    /// naming both, since the day would run out before any vote stood.
    pub fn settle(self, overrides: Overrides) -> anyhow::Result<Settings> {
        let rules = Rules::default();
        let day_limit = overrides
            .limits
            .day_limit
            .or(self.phases.day_limit)
            .unwrap_or(rules.day_limit);
        let ratify_limit = overrides
            .ratify_limit
            .or(self.phases.ratify_limit)
            .unwrap_or(rules.ratify_limit);
        if ratify_limit > day_limit {
            bail!(
                "the ratify limit ({}) is longer than the day limit ({})",
                format_duration(ratify_limit),
                format_duration(day_limit)
            );
        }
        let mut strategies = self.strategies.clone();
        strategies.insert(RANDOM.to_string(), Strategy::Random(Random {}));
        let mut allotments = Allotments::new();
        for (role, default) in Table::default().counts() {
            let file = self.roles.get(role);
            let (given, option) = match role {
                Role::Werewolf => (overrides.roles.werewolves, "--werewolves"),
                Role::Villager => (overrides.roles.villagers, "--villagers"),
                Role::Doctor => (overrides.roles.doctors, "--doctors"),
                Role::Seer => (overrides.roles.seers, "--seers"),
            };
            let allotted = match &file.strategies {
                Some(table) => {
                    let allots: usize = table.values().sum();
                    let counts = [
                        (format!("[roles.{role}] count"), file.count),
                        (option.to_string(), given),
                    ];
                    for (setting, count) in counts {
                        if let Some(count) = count
                            && count != allots
                        {
                            bail!(
                                "{setting} is {count}, but [roles.{role}.strategies] allots {allots}"
                            );
                        }
                    }
                    table.clone()
                }
                None => {
                    let strategy = file
                        .strategy
                        .as_ref()
                        .or(self.strategy.as_ref())
                        .map_or(RANDOM, String::as_str);
                    let count = given.or(file.count).unwrap_or(default);
                    BTreeMap::from([(strategy.to_string(), count)])
                }
            };
            if let Some(name) = allotted.keys().find(|name| !strategies.contains_key(*name)) {
                bail!("the {role} is allotted {name}, which is not a strategy");
            }
            allotments.insert(role, allotted);
        }
        let seats = |role| allotments[&role].values().sum();
        Ok(Settings {
            table: Table {
                werewolves: seats(Role::Werewolf),
                villagers: seats(Role::Villager),
                doctors: seats(Role::Doctor),
                seers: seats(Role::Seer),
            },
            night_limit: overrides
                .limits
                .night_limit
                .or(self.phases.night_limit)
                .unwrap_or(rules.night_limit),
            day_limit,
            ratify_limit,
            strategies,
            allotments,
            config: self,
        })
    }
}

/// One player's system prompt, rendered for a seat dealt a role and a
/// prompted strategy.
#[derive(Debug)]
pub struct SystemPrompt {
    /// The seat, `player1` onward.
    pub seat: PlayerId,
    /// The role the seat was dealt.
    pub role: Role,
    /// The name of the strategy the seat was dealt.
    pub strategy: String,
    /// The rendered prompt.
    pub text: String,
}

/// The models in play as the log records them, by name.
pub type Models = BTreeMap<String, ModelInPlay>;

/// The handle each model in play is asked through, by name.
pub type Handles = BTreeMap<String, Arc<dyn model::Model>>;

impl Settings {
    /// Each (role, strategy) pair at the table, in the order the roles are
    /// dealt and then by the strategy's name, with the strategy and how
    /// many seats it is allotted.
    fn at_the_table(&self) -> impl Iterator<Item = (Role, &str, &Strategy, usize)> {
        self.allotments.iter().flat_map(move |(role, allotted)| {
            allotted
                .iter()
                .filter(|(_, seats)| **seats > 0)
                .map(move |(name, seats)| (*role, name.as_str(), &self.strategies[name], *seats))
        })
    }

    /// Deal the (role, strategy) pairs the allotments make, one per seat,
    /// at random to the seats `player1` onward.
    pub fn deal(&self) -> HashMap<PlayerId, Dealt> {
        let mut pairs: Vec<Dealt> = self
            .at_the_table()
            .flat_map(|(role, strategy, _, seats)| {
                let dealt = Dealt {
                    role,
                    strategy: strategy.to_string(),
                };
                std::iter::repeat_n(dealt, seats)
            })
            .collect();
        pairs.shuffle(&mut rand::rng());
        self.table.seats().zip(pairs).collect()
    }

    /// The name of the model the strategy `name` asks, if it is prompted.
    fn model_of(&self, name: &str) -> Option<&str> {
        match &self.strategies[name] {
            Strategy::Prompted(prompted) => Some(prompted.model_name(&self.config.models)),
            Strategy::Random(_) => None,
        }
    }

    /// The models in play, by name: those a prompted strategy at the table
    /// asks.
    pub fn models_in_play(&self) -> BTreeMap<&str, &Model> {
        self.at_the_table()
            .filter_map(|(_, strategy, _, _)| self.model_of(strategy))
            .map(|name| (name, &self.config.models[name]))
            .collect()
    }

    /// Render the system prompt of every seat for every (role, strategy)
    /// pair at the table whose strategy is prompted, since any seat may be
    /// dealt any of them, in seat order and then the order of the pairs.
    /// A prompt is rendered from its strategy's `system` template, with the
    /// variables `name` and `role` and every `[text]` block. An undefined
    /// variable is an error. A template that does not compile is an error
    /// naming the strategy; one that does not render, the strategy, the
    /// seat and the role.
    pub fn prompts(&self) -> anyhow::Result<Vec<SystemPrompt>> {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        // Each strategy's template is compiled once, and rendered once a
        // seat for each role it is allotted.
        let mut templates = BTreeMap::new();
        let mut pairs = Vec::new();
        for (role, name, strategy, _) in self.at_the_table() {
            if let Strategy::Prompted(prompted) = strategy {
                if let btree_map::Entry::Vacant(vacant) = templates.entry(name) {
                    let template = env.template_from_str(&prompted.system).with_context(|| {
                        format!("compiling the prompt template of strategy {name}")
                    })?;
                    vacant.insert(template);
                }
                pairs.push((role, name));
            }
        }
        let blocks = Value::from(&self.config.text);
        let mut prompts = Vec::new();
        for seat in self.table.seats() {
            for &(role, name) in &pairs {
                let text = templates[name]
                    .render(context! { name => &seat, role => role.to_string(), ..blocks.clone() })
                    .with_context(|| {
                        format!("rendering the prompt of strategy {name} for {seat} as the {role}")
                    })?;
                prompts.push(SystemPrompt {
                    seat: seat.clone(),
                    role,
                    strategy: name.to_string(),
                    text,
                });
            }
        }
        Ok(prompts)
    }

    /// The context window of the model `name`: the one its table gives,
    /// otherwise `reported`, the one the provider's listing reports for
    /// the model, otherwise the provider's default in `src/models.toml`,
    /// otherwise [`CONTEXT_WINDOW`]. A window smaller than a request takes
    /// before its story, by estimate, for the largest of `prompts` whose
    /// strategy asks the model, is an error naming the model and where the
    /// window came from.
    pub fn context_window(
        &self,
        name: &str,
        reported: Option<u64>,
        prompts: &[SystemPrompt],
    ) -> anyhow::Result<ContextWindow> {
        let model = &self.config.models[name];
        let given = |tokens, source| ContextWindow { tokens, source };
        let known = model::known_context_window(&model.base_url);
        let window = model
            .context_window
            .map(|tokens| given(tokens, Source::File))
            .or(reported.map(|tokens| given(tokens, Source::Provider)))
            .or(known.map(|tokens| given(tokens, Source::ProviderDefault)))
            .unwrap_or(given(CONTEXT_WINDOW, Source::Default));
        let needed = self.unstoried(name, prompts)?;
        if window.tokens < needed {
            let setting = match window.source {
                Source::File => format!("[models.{name}] context_window"),
                Source::Provider => format!("the context window the provider reports for {name}"),
                Source::ProviderDefault => {
                    format!("the provider's default context window for {name}")
                }
                Source::Default => format!("the default context window for {name}"),
            };
            let remedy = match window.source {
                Source::File => String::new(),
                _ => format!("; context_window in [models.{name}] gives the window"),
            };
            bail!(
                "{setting} is {} tokens, smaller than the {needed} a request takes before its story, by estimate{remedy}",
                window.tokens
            );
        }
        Ok(window)
    }

    /// The most a request to the model `name` takes of the context window
    /// before its story, by estimate, for any of `prompts` whose strategy
    /// asks it.
    fn unstoried(&self, name: &str, prompts: &[SystemPrompt]) -> anyhow::Result<u64> {
        let alive: HashSet<PlayerId> = self.table.seats().collect();
        let id = &self.config.models[name].id;
        prompts
            .iter()
            .filter(|prompt| self.model_of(&prompt.strategy) == Some(name))
            .try_fold(0, |most, prompt| {
                let (text, seat, role) = (&prompt.text, &prompt.seat, prompt.role);
                let needed = prompted::unstoried(id, text, seat, role, &alive, self.day_limit);
                Ok(most.max(needed?))
            })
    }

    /// Set up each model in play, one at a time, since a local server loads
    /// each model it is asked for: read its key, find it in its provider's
    /// listing, settle its context window for `prompts`, and ask it to play
    /// as by night and as by day, which settles the tool choice. Each, by
    /// name, as the log records it, and the handle its players ask it
    /// through. A model that cannot be set up is an error naming it. A
    /// model the file defines and no strategy at the table asks is not
    /// contacted.
    pub async fn set_up(&self, prompts: &[SystemPrompt]) -> anyhow::Result<(Models, Handles)> {
        let mut models = Models::new();
        let mut handles = Handles::new();
        for (name, model) in self.models_in_play() {
            let set_up = async {
                let provider = model.provider()?;
                let listed = provider.listed(&model.id).await?;
                let context_window = self.context_window(name, listed.context_window, prompts)?;
                let tool_choice = match prompted::check_plays(&provider, &model.id).await? {
                    Probed::Played(tool_choice) => tool_choice,
                    Probed::DidNotPlay(error) => return Err(error),
                };
                let played = ModelInPlay {
                    base_url: model.base_url.clone(),
                    id: model.id.clone(),
                    request_timeout: model.request_timeout,
                    api_key_env: model.key_variable().map(String::from),
                    tool_choice,
                    context_window,
                };
                anyhow::Ok((played, provider))
            };
            let (played, provider) = set_up
                .await
                .with_context(|| format!("setting up the model {name}"))?;
            models.insert(name.to_string(), played);
            handles.insert(name.to_string(), Arc::new(provider));
        }
        Ok((models, handles))
    }

    /// The configuration a game of `deal` runs under, as the log records
    /// it right after the deal: every setting as it took effect, every
    /// strategy and the allotments, `models`, the models in play as
    /// [`Settings::set_up`] set them up, each prompted player's system
    /// prompt from `prompts`, which [`Settings::prompts`] rendered, and the
    /// text blocks as written. A prompted player with no prompt rendered
    /// for its role and strategy is an error naming the seat, the role and
    /// the strategy.
    pub fn configuration(
        &self,
        deal: &HashMap<PlayerId, Dealt>,
        prompts: &[SystemPrompt],
        models: Models,
    ) -> anyhow::Result<Configuration> {
        let rendered: HashMap<(&PlayerId, Role, &str), &str> = prompts
            .iter()
            .map(|prompt| {
                let pair = (&prompt.seat, prompt.role, prompt.strategy.as_str());
                (pair, prompt.text.as_str())
            })
            .collect();
        let mut told = BTreeMap::new();
        for (seat, Dealt { role, strategy }) in deal {
            if self.model_of(strategy).is_some() {
                let text = rendered.get(&(seat, *role, strategy)).ok_or_else(|| {
                    anyhow!("no system prompt rendered for {seat} as the {role} playing {strategy}")
                })?;
                told.insert(seat.clone(), text.to_string());
            }
        }
        Ok(Configuration {
            role_counts: self.table.counts().into_iter().collect(),
            night_limit: self.night_limit,
            day_limit: self.day_limit,
            ratify_limit: self.ratify_limit,
            strategies: self.strategies.clone(),
            allotments: self.allotments.clone(),
            models,
            prompts: told,
            text: self.config.text.clone(),
        })
    }

    /// The rules a game is played under: the default rules, with the
    /// limits that took effect.
    pub fn rules(&self) -> Rules {
        Rules {
            night_limit: self.night_limit,
            day_limit: self.day_limit,
            ratify_limit: self.ratify_limit,
            ..Rules::default()
        }
    }
}

/// A model's context window, in tokens, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindow {
    /// How many tokens the model takes in at once.
    pub tokens: u64,
    /// Where the window came from.
    pub source: Source,
}

/// Where a context window came from, written as `file`, `provider`,
/// `provider default` or `default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The configuration file.
    File,
    /// The provider.
    Provider,
    /// What `src/models.toml` knows of the provider.
    #[serde(rename = "provider default")]
    ProviderDefault,
    /// The code's default.
    Default,
}

/// A model in play, as it was set up. The API key is never recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInPlay {
    /// The root of the provider's OpenAI-compatible API.
    pub base_url: String,
    /// The model's id, as the provider lists it.
    pub id: String,
    /// How long each request could take.
    #[serde(with = "humantime_serde")]
    pub request_timeout: Duration,
    /// The name of the environment variable the key came from, if any.
    pub api_key_env: Option<String>,
    /// Whether each request made the model call a tool, as the probe
    /// found the model would.
    pub tool_choice: ToolChoice,
    /// The model's context window and where it came from.
    pub context_window: ContextWindow,
}

/// The configuration a game ran under, as it took effect once the command
/// line, the file and the defaults were combined, so that the log says by
/// itself what was played and under which prompts. Each duration is
/// written as `humantime` writes it, such as `"60s"`, and each map in key
/// order. The API key is never recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Configuration {
    /// How many of each role were dealt.
    pub role_counts: BTreeMap<Role, usize>,
    /// How long the night waited for a player.
    #[serde(with = "humantime_serde")]
    pub night_limit: Duration,
    /// How long the day waited for a player.
    #[serde(with = "humantime_serde")]
    pub day_limit: Duration,
    /// How long a day's votes had to stand to be ratified.
    #[serde(with = "humantime_serde")]
    pub ratify_limit: Duration,
    /// Every strategy as written, by name, [`RANDOM`] included.
    pub strategies: BTreeMap<String, Strategy>,
    /// How many seats of each role played each strategy.
    pub allotments: Allotments,
    /// The models in play, by name.
    pub models: Models,
    /// Each prompted player's rendered system prompt.
    pub prompts: BTreeMap<PlayerId, String>,
    /// The `[text]` blocks as written.
    pub text: BTreeMap<String, String>,
}

/// Each seat's player, as `deal` dealt it a strategy and `configuration`
/// sets the strategy up: a random player, or a prompted player told its
/// system prompt and asking its strategy's model through its handle in
/// `handles`.
pub fn players(
    configuration: &Configuration,
    deal: &HashMap<PlayerId, Dealt>,
    handles: &Handles,
) -> impl FnMut(&PlayerId) -> Seat + use<> {
    let mut prompted = HashMap::new();
    for (seat, Dealt { strategy, .. }) in deal {
        if let Strategy::Prompted(strategy) = &configuration.strategies[strategy] {
            let name = strategy.model_name(&configuration.models);
            let model = &configuration.models[name];
            let player = (
                configuration.prompts[seat].clone(),
                model.id.clone(),
                model.tool_choice,
                model.context_window.tokens,
                Arc::clone(&handles[name]),
            );
            prompted.insert(seat.clone(), player);
        }
    }
    move |id: &PlayerId| match prompted.remove(id) {
        Some((prompt, model_id, tool_choice, context_window, model)) => prompted::Player::init(
            id.clone(),
            prompt,
            model_id,
            tool_choice,
            context_window,
            model,
        ),
        None => random::Player::init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned::{self, playing, playing_within, serving};
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::werewolf::report::Narrator;
    use crate::werewolf::tests::{between_the_deal_and_the_end, logged, run};
    use crate::werewolf::{Entry, Message, game};
    use secrecy::ExposeSecret;
    use serde_json::json;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    /// A file defining one model, `qwen`, and one prompted strategy for
    /// everyone, `plain`, asking it.
    const PLAIN: &str = r#"
strategy = "plain"
[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
[strategies.plain]
policy = "prompted"
system = "{{ name }} the {{ role }}"
"#;

    /// The settings `file` and `overrides` produce under the code defaults.
    fn settle(file: &str, overrides: Overrides) -> Settings {
        Config::parse(file).unwrap().settle(overrides).unwrap()
    }

    /// The error `file` and `overrides` settle to, causes included.
    fn unsettled(file: &str, overrides: Overrides) -> String {
        let error = Config::parse(file)
            .and_then(|config| config.settle(overrides))
            .unwrap_err();
        format!("{error:#}")
    }

    /// Overrides giving the role counts `roles`.
    fn counting(roles: RoleCounts) -> Overrides {
        Overrides {
            roles,
            ..Overrides::default()
        }
    }

    /// An allotment of `seats` to each named strategy.
    fn allotted(seats: &[(&str, usize)]) -> BTreeMap<String, usize> {
        seats
            .iter()
            .map(|(name, seats)| (name.to_string(), *seats))
            .collect()
    }

    #[test]
    fn the_examples_parse_and_render_and_are_the_two_documented() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/werewolf");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["prompted.toml", "random.toml"]);
        for name in names {
            let file = Path::new(dir).join(&name);
            let config = Config::load(&file).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let settings = config
                .settle(Overrides::default())
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let prompts = settings
                .prompts()
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(settings.table, Table::default(), "{name}");
            // The rules, where there are any, tell the players the ratify
            // limit, and that by day they may speak, vote, or wait, and
            // withdraw their vote.
            if let Some(rules) = settings.config.text.get("rules") {
                assert!(!prompts.is_empty(), "{name}");
                let ratify = format_duration(settings.ratify_limit).to_string();
                assert!(rules.contains(&ratify), "{name}: {rules}");
                assert!(rules.contains("speak"), "{name}: {rules}");
                assert!(rules.contains("vote, or wait"), "{name}: {rules}");
                assert!(rules.contains("withdraw"), "{name}: {rules}");
            }
        }
    }

    #[test]
    fn the_random_example_plays_the_game_of_no_file() {
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/werewolf/random.toml");
        let example = Config::load(Path::new(file))
            .unwrap()
            .settle(Overrides::default())
            .unwrap();
        let none = Config::default().settle(Overrides::default()).unwrap();
        let configured = |settings: &Settings| {
            let deal = settings.deal();
            settings.configuration(&deal, &[], Models::new()).unwrap()
        };
        assert_eq!(configured(&example), configured(&none));
        assert!(example.models_in_play().is_empty());
    }

    #[test]
    fn a_file_that_is_not_there_is_an_error_naming_it() {
        let error = Config::load(Path::new("no-such-file.toml")).unwrap_err();
        assert!(
            format!("{error:#}").contains("no-such-file.toml"),
            "{error:#}"
        );
    }

    #[test]
    fn with_no_file_every_role_of_the_default_table_is_random() {
        let settings = settle("", Overrides::default());
        assert_eq!(settings.table, Table::default());
        assert_eq!(
            settings.allotments,
            Table::default()
                .counts()
                .into_iter()
                .map(|(role, seats)| (role, allotted(&[(RANDOM, seats)])))
                .collect::<Allotments>()
        );
        assert_eq!(
            settings.strategies,
            BTreeMap::from([(RANDOM.to_string(), Strategy::Random(Random {}))])
        );
        let rules = Rules::default();
        assert_eq!(settings.night_limit, rules.night_limit);
        assert_eq!(settings.day_limit, rules.day_limit);
        assert_eq!(settings.ratify_limit, Duration::ZERO);
    }

    #[test]
    fn a_strategy_of_either_policy_parses_and_is_written_as_it_was() {
        let file = format!("{PLAIN}[strategies.dice]\npolicy = \"random\"\n");
        let config = Config::parse(&file).unwrap();
        assert_eq!(config.strategies["dice"], Strategy::Random(Random {}));
        assert_eq!(
            config.strategies["plain"],
            Strategy::Prompted(Prompted {
                system: "{{ name }} the {{ role }}".to_string(),
                model: None,
            })
        );
        assert_eq!(
            serde_json::to_value(&config.strategies).unwrap(),
            json!({
                "dice": {"policy": "random"},
                "plain": {"policy": "prompted", "system": "{{ name }} the {{ role }}"},
            })
        );
    }

    #[test]
    fn a_strategy_takes_its_policy_s_parameters_and_only_those() {
        for strategy in [
            "policy = \"random\"\nsystem = \"Hi.\"\n",
            "policy = \"random\"\nmodel = \"qwen\"\n",
            "policy = \"prompted\"\nsystem = \"Hi.\"\ntemperature = 0.5\n",
            "policy = \"prompted\"\nmodel = \"qwen\"\n",
            "policy = \"scripted\"\n",
            "system = \"Hi.\"\n",
        ] {
            let file = format!("{PLAIN}[strategies.odd]\n{strategy}");
            assert!(Config::parse(&file).is_err(), "{strategy}");
        }
        let file = format!("{PLAIN}[strategies.odd]\npolicy = \"prompted\"\n");
        let error = Config::parse(&file).unwrap_err();
        assert!(format!("{error:#}").contains("system"), "{error:#}");
    }

    #[test]
    fn a_prompted_strategy_may_leave_its_model_out_only_when_the_file_defines_one() {
        let settings = settle(PLAIN, Overrides::default());
        assert_eq!(settings.model_of("plain"), Some("qwen"));
        assert_eq!(settings.model_of(RANDOM), None);
        let two = format!(
            "{PLAIN}[models.claude]\nbase_url = \"https://api.anthropic.com/v1\"\nid = \"claude\"\n"
        );
        assert_eq!(
            unsettled(&two, Overrides::default()),
            "strategy plain names no model, which it may leave out only when the file defines exactly one; it defines 2"
        );
        let none = "[strategies.plain]\npolicy = \"prompted\"\nsystem = \"Hi.\"\n";
        let shown = unsettled(none, Overrides::default());
        assert!(shown.ends_with("it defines 0"), "{shown}");
        // Named, either may be asked.
        let named = format!("{two}[strategies.plain.model]\n");
        assert!(Config::parse(&named).is_err());
        let named = two.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"claude\"",
        );
        assert_eq!(
            settle(&named, Overrides::default()).model_of("plain"),
            Some("claude")
        );
    }

    #[test]
    fn a_model_that_is_not_defined_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"claude\"",
        );
        assert_eq!(
            unsettled(&file, Overrides::default()),
            "strategy plain names the model claude, which is not defined"
        );
    }

    #[test]
    fn the_random_strategy_is_built_in_and_may_not_be_defined() {
        let file = "[strategies.random]\npolicy = \"random\"\n";
        assert_eq!(
            unsettled(file, Overrides::default()),
            "[strategies.random] is built in, and may not be defined"
        );
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        for file in [
            format!("{PLAIN}temperature = 0.5\n"),
            format!("{PLAIN}[phases]\nnight = \"1m\"\n"),
            format!("{PLAIN}[roles.seer]\nsystem = \"\"\n"),
            format!("{PLAIN}[roles.seer]\nprompt = \"\"\n"),
            format!("{PLAIN}[prompt]\nsystem = \"\"\n"),
            format!("{PLAIN}[personas]\nplayer1 = \"\"\n"),
            format!("{PLAIN}[model]\nbase_url = \"\"\nid = \"\"\n"),
            format!("{PLAIN}[tuning]\n"),
            PLAIN.replace("id = ", "temperature = 0.5\nid = "),
        ] {
            assert!(Config::parse(&file).is_err(), "{file}");
        }
    }

    #[test]
    fn an_unknown_role_is_an_error() {
        let error = Config::parse("[roles.hunter]\ncount = 1\n").unwrap_err();
        assert!(format!("{error:#}").contains("hunter"), "{error:#}");
    }

    #[test]
    fn a_duration_the_file_cannot_parse_is_an_error() {
        let model = PLAIN.replace("id = ", "request_timeout = \"soon\"\nid = ");
        assert!(Config::parse(&model).is_err());
        let model = PLAIN.replace("id = ", "request_timeout = 10\nid = ");
        assert!(Config::parse(&model).is_err());
        assert!(Config::parse("[phases]\nnight_limit = \"a while\"\n").is_err());
    }

    #[test]
    fn the_request_timeout_comes_from_the_file_or_the_code() {
        let settings = settle(PLAIN, Overrides::default());
        assert_eq!(
            settings.config.models["qwen"].request_timeout,
            REQUEST_TIMEOUT
        );
        let file = PLAIN.replace("id = ", "request_timeout = \"10s\"\nid = ");
        assert_eq!(
            settle(&file, Overrides::default()).config.models["qwen"].request_timeout,
            Duration::from_secs(10)
        );
    }

    #[test]
    fn a_context_window_of_0_is_an_error_naming_the_model() {
        let file = PLAIN.replace("id = ", "context_window = 0\nid = ");
        assert_eq!(
            unsettled(&file, Overrides::default()),
            "[models.qwen] context_window is 0; a context window holds at least 1 token"
        );
    }

    #[test]
    fn a_text_block_named_like_a_variable_is_an_error_naming_it() {
        for variable in VARIABLES {
            let file = format!("{PLAIN}[text]\n{variable} = \"Taken.\"\n");
            let error = Config::parse(&file).unwrap_err();
            assert!(format!("{error:#}").contains(variable), "{error:#}");
        }
    }

    /// A file of three strategies, `dice`, `bold` and `plain`, of which
    /// `bold` and `plain` are prompted.
    fn three() -> String {
        format!(
            "{PLAIN}
[strategies.dice]
policy = \"random\"
[strategies.bold]
policy = \"prompted\"
system = \"Bold {{{{ name }}}}.\"
"
        )
    }

    #[test]
    fn a_role_is_allotted_by_its_strategies_its_strategy_or_the_file_s_in_that_order() {
        let file = format!(
            "{}
[roles.werewolf.strategies]
dice = 1
bold = 2
[roles.villager]
strategy = \"dice\"
[roles.doctor]
strategy = \"bold\"
[roles.doctor.strategies]
dice = 1
",
            three()
        );
        let settings = settle(&file, Overrides::default());
        assert_eq!(
            settings.allotments,
            Allotments::from([
                (Role::Werewolf, allotted(&[("bold", 2), ("dice", 1)])),
                (Role::Villager, allotted(&[("dice", 3)])),
                (Role::Doctor, allotted(&[("dice", 1)])),
                (Role::Seer, allotted(&[("plain", 1)])),
            ])
        );
        assert_eq!(
            settings.table,
            Table {
                werewolves: 3,
                ..Table::default()
            }
        );
        // A role's own strategy over the file's, and the file's over
        // random.
        let file = three().replace("strategy = \"plain\"\n", "");
        let file = format!("{file}[roles.seer]\nstrategy = \"bold\"\n");
        let settings = settle(&file, Overrides::default());
        assert_eq!(settings.allotments[&Role::Seer], allotted(&[("bold", 1)]));
        assert_eq!(
            settings.allotments[&Role::Werewolf],
            allotted(&[(RANDOM, 2)])
        );
    }

    #[test]
    fn a_strategies_table_s_seats_are_the_role_s_count_and_a_count_that_disagrees_is_an_error() {
        let file = format!(
            "{}[roles.werewolf.strategies]\ndice = 1\nbold = 2\n",
            three()
        );
        let agreeing = format!("{file}[roles.werewolf]\ncount = 3\n");
        let given = |werewolves| {
            counting(RoleCounts {
                werewolves: Some(werewolves),
                ..RoleCounts::default()
            })
        };
        assert_eq!(settle(&agreeing, given(3)).table.werewolves, 3);
        let file = file.replace(
            "[roles.werewolf.strategies]",
            "[roles.werewolf]\ncount = 2\n[roles.werewolf.strategies]",
        );
        assert_eq!(
            unsettled(&file, Overrides::default()),
            "[roles.werewolf] count is 2, but [roles.werewolf.strategies] allots 3"
        );
        assert_eq!(
            unsettled(&agreeing, given(4)),
            "--werewolves is 4, but [roles.werewolf.strategies] allots 3"
        );
    }

    #[test]
    fn a_strategy_allotted_that_is_not_defined_is_an_error_naming_the_role() {
        let files = [
            ("[roles.doctor.strategies]\nwild = 1\n", "doctor"),
            ("[roles.seer]\nstrategy = \"wild\"\n", "seer"),
        ];
        for (roles, role) in files {
            assert_eq!(
                unsettled(&format!("{PLAIN}{roles}"), Overrides::default()),
                format!("the {role} is allotted wild, which is not a strategy")
            );
        }
        let file = PLAIN.replace("strategy = \"plain\"", "strategy = \"wild\"");
        assert_eq!(
            unsettled(&file, Overrides::default()),
            "the werewolf is allotted wild, which is not a strategy"
        );
        // A strategy defined and never allotted is no error.
        settle(&three(), Overrides::default());
    }

    #[test]
    fn the_role_counts_fall_back_from_the_command_line_to_the_file_to_the_code() {
        let file = "
[roles.werewolf]
count = 5
[roles.villager]
count = 6
[roles.doctor]
count = 7
[roles.seer]
count = 8
";
        // The code alone.
        assert_eq!(settle("", Overrides::default()).table, Table::default());
        // The file over the code.
        assert_eq!(
            settle(file, Overrides::default()).table,
            Table {
                werewolves: 5,
                villagers: 6,
                doctors: 7,
                seers: 8
            }
        );
        // The command line over the file.
        let counts = RoleCounts {
            werewolves: Some(1),
            villagers: Some(2),
            doctors: Some(3),
            seers: Some(4),
        };
        assert_eq!(
            settle(file, counting(counts)).table,
            Table {
                werewolves: 1,
                villagers: 2,
                doctors: 3,
                seers: 4
            }
        );
        // A count the command line does not give still comes from the file.
        let counts = RoleCounts {
            werewolves: Some(1),
            seers: Some(4),
            ..RoleCounts::default()
        };
        assert_eq!(
            settle(file, counting(counts)).table,
            Table {
                werewolves: 1,
                villagers: 6,
                doctors: 7,
                seers: 4
            }
        );
    }

    #[test]
    fn the_phase_limits_fall_back_from_the_command_line_to_the_file_to_the_code() {
        let file = "[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"2m\"\n";
        // The file over the code.
        let settings = settle(file, Overrides::default());
        assert_eq!(settings.night_limit, Duration::from_secs(90));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
        // The command line over the file.
        let limits = |night_limit, day_limit| Overrides {
            limits: PhaseLimits {
                night_limit,
                day_limit,
            },
            ..Overrides::default()
        };
        let five = Some(Duration::from_secs(5));
        let settings = settle(file, limits(five, Some(Duration::from_secs(70))));
        assert_eq!(settings.night_limit, Duration::from_secs(5));
        assert_eq!(settings.day_limit, Duration::from_secs(70));
        // A limit the command line does not give still comes from the file.
        let settings = settle(file, limits(five, None));
        assert_eq!(settings.night_limit, Duration::from_secs(5));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
    }

    #[test]
    fn the_ratify_limit_falls_back_from_the_command_line_to_the_file_to_the_code() {
        let file = "[phases]\nratify_limit = \"15s\"\n";
        assert_eq!(
            settle(file, Overrides::default()).ratify_limit,
            Duration::from_secs(15)
        );
        let overrides = Overrides {
            ratify_limit: Some(Duration::from_secs(5)),
            ..Overrides::default()
        };
        assert_eq!(settle(file, overrides).ratify_limit, Duration::from_secs(5));
    }

    #[test]
    fn a_ratify_limit_longer_than_the_day_limit_is_an_error_naming_both() {
        let file = "[phases]\nday_limit = \"20s\"\nratify_limit = \"30s\"\n";
        assert_eq!(
            unsettled(file, Overrides::default()),
            "the ratify limit (30s) is longer than the day limit (20s)"
        );
        let limits = |day_limit: u64, ratify_limit| Overrides {
            limits: PhaseLimits {
                night_limit: None,
                day_limit: Some(Duration::from_secs(day_limit)),
            },
            ratify_limit: Some(Duration::from_secs(ratify_limit)),
            ..Overrides::default()
        };
        assert_eq!(
            unsettled("", limits(10, 20)),
            "the ratify limit (20s) is longer than the day limit (10s)"
        );
        // One as long as the day is allowed.
        assert_eq!(
            settle("", limits(20, 20)).ratify_limit,
            Duration::from_secs(20)
        );
    }

    #[test]
    fn the_rules_are_the_default_rules_with_the_limits_that_took_effect() {
        let file =
            "[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"7s\"\nratify_limit = \"3s\"\n";
        let settled = settle(file, Overrides::default()).rules();
        assert_eq!(settled.night_limit, Duration::from_secs(90));
        assert_eq!(settled.day_limit, Duration::from_secs(7));
        assert_eq!(settled.ratify_limit, Duration::from_secs(3));
    }

    /// A table of two werewolves, one random and one bold, and three
    /// villagers, two random and one plain, and nobody else.
    fn mixed() -> Settings {
        let file = format!(
            "{}
[roles.werewolf.strategies]
dice = 1
bold = 1
[roles.villager.strategies]
random = 2
plain = 1
[roles.doctor]
count = 0
[roles.seer]
count = 0
",
            three()
        );
        settle(&file, Overrides::default())
    }

    #[test]
    fn every_pair_is_dealt_as_many_times_as_allotted_to_the_seats_from_player1() {
        let settings = mixed();
        let deal = settings.deal();
        let mut seats: Vec<&PlayerId> = deal.keys().collect();
        seats.sort();
        assert_eq!(
            seats,
            ["player1", "player2", "player3", "player4", "player5"]
        );
        let mut pairs: Vec<(Role, &str)> = deal
            .values()
            .map(|dealt| (dealt.role, dealt.strategy.as_str()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            [
                (Role::Werewolf, "bold"),
                (Role::Werewolf, "dice"),
                (Role::Villager, "plain"),
                (Role::Villager, RANDOM),
                (Role::Villager, RANDOM),
            ]
        );
    }

    #[test]
    fn the_deal_is_shuffled_in_roles_and_in_strategies() {
        let settings = mixed();
        let deals: Vec<HashMap<PlayerId, Dealt>> = (0..100).map(|_| settings.deal()).collect();
        // Every seat is dealt the bold werewolf in some deal, and the dice
        // werewolf in another.
        for strategy in ["bold", "dice"] {
            let seats: HashSet<&PlayerId> = deals
                .iter()
                .flat_map(|deal| deal.iter())
                .filter(|(_, dealt)| dealt.strategy == strategy)
                .map(|(seat, _)| seat)
                .collect();
            assert_eq!(seats.len(), 5, "{strategy}: {seats:?}");
        }
    }

    /// The prompt `prompts` holds for `seat` dealt `role` and `strategy`.
    fn prompt<'a>(prompts: &'a [SystemPrompt], seat: &str, role: Role, strategy: &str) -> &'a str {
        &prompts
            .iter()
            .find(|prompt| {
                prompt.seat == seat && prompt.role == role && prompt.strategy == strategy
            })
            .unwrap_or_else(|| panic!("no prompt for {seat} as the {role} playing {strategy}"))
            .text
    }

    #[test]
    fn every_seat_is_rendered_for_every_prompted_pair_at_the_table_in_order() {
        let prompts = mixed().prompts().unwrap();
        let rendered: Vec<(&str, Role, &str)> = prompts
            .iter()
            .map(|prompt| (prompt.seat.as_str(), prompt.role, prompt.strategy.as_str()))
            .collect();
        let seats = ["player1", "player2", "player3", "player4", "player5"];
        let expected: Vec<(&str, Role, &str)> = seats
            .into_iter()
            .flat_map(|seat| {
                [
                    (seat, Role::Werewolf, "bold"),
                    (seat, Role::Villager, "plain"),
                ]
            })
            .collect();
        assert_eq!(rendered, expected);
        assert_eq!(
            prompt(&prompts, "player2", Role::Werewolf, "bold"),
            "Bold player2."
        );
        assert_eq!(
            prompt(&prompts, "player5", Role::Villager, "plain"),
            "player5 the villager"
        );
        // A table with no prompted strategy renders nothing.
        assert!(
            settle("", Overrides::default())
                .prompts()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn every_text_block_is_a_variable_and_a_persona_is_not() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{{ rules }} {{ wolf }}");
        let file = format!("{file}[text]\nrules = \"The rules.\"\nwolf = \"Howl.\"\n");
        let prompts = settle(&file, Overrides::default()).prompts().unwrap();
        assert_eq!(
            prompt(&prompts, "player1", Role::Villager, "plain"),
            "The rules. Howl."
        );
        let file = PLAIN.replace("{{ role }}", "{{ role }}[{{ persona }}]");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        assert!(format!("{error:#}").contains("persona"), "{error:#}");
    }

    #[test]
    fn a_template_that_does_not_render_is_an_error_naming_the_strategy_the_seat_and_the_role() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{{ rulez }}");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("rendering the prompt of strategy plain for player1 as the werewolf"),
            "{shown}"
        );
        assert!(shown.contains("rulez"), "{shown}");
    }

    #[test]
    fn a_template_that_does_not_compile_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{% if %}");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("compiling the prompt template of strategy plain"),
            "{shown}"
        );
        assert!(shown.contains("syntax"), "{shown}");
        // A strategy at no seat of the table is not compiled.
        let file = format!(
            "{file}[roles.werewolf]\nstrategy = \"random\"\ncount = 7\n[roles.villager]\ncount = 0\n[roles.doctor]\ncount = 0\n[roles.seer]\ncount = 0\n"
        );
        assert!(
            settle(&file, Overrides::default())
                .prompts()
                .unwrap()
                .is_empty()
        );
    }

    /// A file of two models, `qwen` and `claude`, and a strategy asking
    /// each, `plain` and `smart`, with `rest` after the strategies.
    fn two_models(rest: &str) -> String {
        format!(
            "{PLAIN}
[models.claude]
base_url = \"https://api.anthropic.com/v1\"
id = \"claude-sonnet-4-5\"
api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"
[strategies.smart]
policy = \"prompted\"
model = \"claude\"
system = \"Smart.\"
{rest}"
        )
        .replace(
            "system = \"{{ name }}",
            "model = \"qwen\"\nsystem = \"{{ name }}",
        )
    }

    #[test]
    fn a_model_is_in_play_when_a_strategy_at_the_table_asks_it() {
        let settings = settle(&two_models(""), Overrides::default());
        let in_play: Vec<&str> = settings.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["qwen"]);
        let both = settle(
            &two_models("[roles.seer]\nstrategy = \"smart\"\n"),
            Overrides::default(),
        );
        let in_play: Vec<&str> = both.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["claude", "qwen"]);
        // Allotted no seats, a strategy puts no model in play.
        let unseated = settle(
            &two_models("[roles.seer]\ncount = 0\nstrategy = \"smart\"\n"),
            Overrides::default(),
        );
        assert_eq!(unseated.models_in_play().len(), 1);
    }

    /// A file of three models: `claude` at `claude_url`, `qwen` at
    /// `qwen_url` with a context window of 8192, and `idle`, whose key is
    /// not set; the seer playing `smart`, asking `claude`, and everyone
    /// else `plain`, asking `qwen`, so that `idle` is not in play.
    fn three_models(claude_url: &str, qwen_url: &str) -> String {
        two_models(
            "[models.idle]
base_url = \"https://api.openai.com/v1\"
id = \"gpt-0\"
api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"
[strategies.unused]
policy = \"prompted\"
model = \"idle\"
system = \"Unused.\"
[roles.seer]
strategy = \"smart\"
",
        )
        .replace("https://api.anthropic.com/v1", claude_url)
        .replace(
            "api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"\n[strategies.smart]",
            "[strategies.smart]",
        )
        .replace("http://localhost:1234/v1", qwen_url)
        .replace(
            "id = \"qwen2.5-7b-instruct\"",
            "id = \"qwen2.5-7b-instruct\"\ncontext_window = 8192",
        )
    }

    #[tokio::test]
    async fn every_model_in_play_is_set_up_once_with_its_own_tool_choice_and_window() {
        let claude_url = canned::refusing_required(&["claude-sonnet-4-5"]);
        let (qwen_url, asked) = canned::recording(&["qwen2.5-7b-instruct"]);
        let settings = settle(&three_models(&claude_url, &qwen_url), Overrides::default());
        let prompts = settings.prompts().unwrap();
        let (models, handles) = settings.set_up(&prompts).await.unwrap();
        assert_eq!(
            models,
            Models::from([
                (
                    "claude".to_string(),
                    ModelInPlay {
                        base_url: claude_url,
                        id: "claude-sonnet-4-5".to_string(),
                        request_timeout: REQUEST_TIMEOUT,
                        api_key_env: None,
                        tool_choice: ToolChoice::Auto,
                        context_window: ContextWindow {
                            tokens: CONTEXT_WINDOW,
                            source: Source::Default,
                        },
                    }
                ),
                (
                    "qwen".to_string(),
                    ModelInPlay {
                        base_url: qwen_url,
                        id: "qwen2.5-7b-instruct".to_string(),
                        request_timeout: REQUEST_TIMEOUT,
                        api_key_env: None,
                        tool_choice: ToolChoice::Required,
                        context_window: ContextWindow {
                            tokens: 8192,
                            source: Source::File,
                        },
                    }
                ),
            ])
        );
        assert_eq!(handles.keys().collect::<Vec<_>>(), ["claude", "qwen"]);
        // Probed once: asked as by night and as by day, with the tools of
        // a ratified day, each a request made to call a tool.
        let asked = asked.lock().unwrap();
        let offered: Vec<usize> = asked
            .iter()
            .map(|request| request["tools"].as_array().unwrap().len())
            .collect();
        assert_eq!(offered, [1, 4]);
        assert!(
            asked
                .iter()
                .all(|request| request["tool_choice"] == "required")
        );
    }

    #[tokio::test]
    async fn the_second_model_that_cannot_be_set_up_is_an_error_naming_it() {
        let claude_url = playing(&["claude-sonnet-4-5"]);
        let qwen_url = canned::refusing(&["qwen2.5-7b-instruct"]);
        let settings = settle(&three_models(&claude_url, &qwen_url), Overrides::default());
        let error = settings
            .set_up(&settings.prompts().unwrap())
            .await
            .err()
            .unwrap();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(
                "setting up the model qwen: qwen2.5-7b-instruct did not answer a night's request"
            ),
            "{shown}"
        );
    }

    #[test]
    fn each_model_s_window_is_checked_against_the_strategies_asking_it_alone() {
        let settings = settle(
            &two_models("[roles.seer]\nstrategy = \"smart\"\n"),
            Overrides::default(),
        );
        // A long prompt for every seat as the seer, playing `smart`.
        let mut prompts = settings.prompts().unwrap();
        for prompt in &mut prompts {
            if prompt.strategy == "smart" {
                prompt.text = "x".repeat(12000);
            }
        }
        let plain = settings.unstoried("qwen", &prompts).unwrap();
        let smart = settings.unstoried("claude", &prompts).unwrap();
        assert!(smart > plain + 2900, "{smart} {plain}");
        assert_eq!(
            settings
                .context_window("qwen", Some(plain), &prompts)
                .unwrap(),
            ContextWindow {
                tokens: plain,
                source: Source::Provider,
            }
        );
        let error = settings
            .context_window("claude", Some(plain), &prompts)
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            format!(
                "the context window the provider reports for claude is {plain} tokens, smaller than the {smart} a request takes before its story, by estimate; context_window in [models.claude] gives the window"
            )
        );
    }

    #[tokio::test]
    async fn only_the_models_in_play_are_set_up_and_one_not_in_play_needs_no_key() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let file = two_models("").replace("http://localhost:1234/v1", &base_url);
        let settings = settle(&file, Overrides::default());
        let prompts = settings.prompts().unwrap();
        let (models, handles) = settings.set_up(&prompts).await.unwrap();
        assert_eq!(
            models,
            Models::from([(
                "qwen".to_string(),
                ModelInPlay {
                    base_url,
                    id: "qwen2.5-7b-instruct".to_string(),
                    request_timeout: REQUEST_TIMEOUT,
                    api_key_env: None,
                    tool_choice: ToolChoice::Required,
                    context_window: ContextWindow {
                        tokens: CONTEXT_WINDOW,
                        source: Source::Default,
                    },
                }
            )])
        );
        assert_eq!(handles.keys().collect::<Vec<_>>(), ["qwen"]);
        // A table of random players sets up nothing.
        let file = two_models("").replace("strategy = \"plain\"", "strategy = \"random\"");
        let settings = settle(&file, Overrides::default());
        let (models, handles) = settings.set_up(&[]).await.unwrap();
        assert!(models.is_empty() && handles.is_empty());
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_set_up_is_an_error_naming_it() {
        let shown = |error: anyhow::Error| format!("{error:#}");
        // Not served.
        let base_url = serving(&["llama-3.1-8b-instruct"]);
        let settings = settle(
            &PLAIN.replace("http://localhost:1234/v1", &base_url),
            Overrides::default(),
        );
        let error = settings
            .set_up(&settings.prompts().unwrap())
            .await
            .err()
            .unwrap();
        assert!(shown(error).starts_with("setting up the model qwen: "));
        // Its key not set.
        let file = PLAIN.replace(
            "id = ",
            "api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"\nid = ",
        );
        let settings = settle(&file, Overrides::default());
        let error = shown(settings.set_up(&[]).await.err().unwrap());
        assert!(error.starts_with("setting up the model qwen: "), "{error}");
        assert!(error.contains("SOCIAL_DEDUCTION_NO_SUCH_KEY"), "{error}");
        // Not playing.
        let base_url = canned::refusing(&["qwen2.5-7b-instruct"]);
        let settings = settle(
            &PLAIN.replace("http://localhost:1234/v1", &base_url),
            Overrides::default(),
        );
        let error = shown(
            settings
                .set_up(&settings.prompts().unwrap())
                .await
                .err()
                .unwrap(),
        );
        assert!(
            error.starts_with(
                "setting up the model qwen: qwen2.5-7b-instruct did not answer a night's request"
            ),
            "{error}"
        );
        // Its window too small.
        let base_url = playing_within(&["qwen2.5-7b-instruct"], 100);
        let settings = settle(
            &PLAIN.replace("http://localhost:1234/v1", &base_url),
            Overrides::default(),
        );
        let error = shown(
            settings
                .set_up(&settings.prompts().unwrap())
                .await
                .err()
                .unwrap(),
        );
        assert!(
            error.starts_with(
                "setting up the model qwen: the context window the provider reports for qwen is 100 tokens"
            ),
            "{error}"
        );
    }

    #[test]
    fn the_context_window_falls_back_from_the_file_to_the_listing_to_the_provider_to_the_code() {
        let window = |tokens, source| ContextWindow { tokens, source };
        let settled = |file: &str, reported| {
            settle(file, Overrides::default())
                .context_window("qwen", reported, &[])
                .unwrap()
        };
        let file = PLAIN.replace("id = ", "context_window = 8192\nid = ");
        assert_eq!(settled(&file, Some(32768)), window(8192, Source::File));
        assert_eq!(settled(PLAIN, Some(32768)), window(32768, Source::Provider));
        assert_eq!(
            settled(PLAIN, None),
            window(CONTEXT_WINDOW, Source::Default)
        );
        assert_eq!(CONTEXT_WINDOW, 4096);
        // A provider `src/models.toml` gives a window for.
        let openai = PLAIN.replace("http://localhost:1234/v1", "https://api.openai.com/v1");
        assert_eq!(
            settled(&openai, Some(32768)),
            window(32768, Source::Provider)
        );
        assert_eq!(
            settled(&openai, None),
            window(128000, Source::ProviderDefault)
        );
    }

    /// Every seat's prompt for the strategy `plain` as the werewolf, each
    /// `length` characters long but for one, which is `longest` long, and
    /// one prompt of the strategy `bold`, which is longer still.
    fn prompts_of_length(settings: &Settings, length: usize, longest: usize) -> Vec<SystemPrompt> {
        let mut prompts: Vec<SystemPrompt> = settings
            .table
            .seats()
            .map(|seat| SystemPrompt {
                seat,
                role: Role::Werewolf,
                strategy: "plain".to_string(),
                text: "x".repeat(length),
            })
            .collect();
        prompts[0].text = "x".repeat(longest);
        prompts.push(SystemPrompt {
            seat: "player1".to_string(),
            role: Role::Werewolf,
            strategy: RANDOM.to_string(),
            text: "x".repeat(longest * 2),
        });
        prompts
    }

    #[test]
    fn a_context_window_with_no_room_for_a_story_is_an_error_naming_the_model_and_where_it_came_from()
     {
        let settings = settle(PLAIN, Overrides::default());
        let short = prompts_of_length(&settings, 30, 30);
        let long = prompts_of_length(&settings, 30, 12000);
        // The longest prompt's day request, with no story, by estimate, of
        // those asking the model.
        let needed = settings.unstoried("qwen", &long).unwrap();
        assert!(needed > settings.unstoried("qwen", &short).unwrap() + 3900);
        assert!(needed < settings.unstoried("qwen", &short).unwrap() + 4500);
        assert!(needed > CONTEXT_WINDOW);
        assert_eq!(
            settings
                .context_window("qwen", Some(needed), &long)
                .unwrap(),
            ContextWindow {
                tokens: needed,
                source: Source::Provider,
            }
        );
        let refused = |settings: &Settings, reported| {
            let error = settings
                .context_window("qwen", reported, &long)
                .unwrap_err();
            format!("{error:#}")
        };
        let too_small =
            format!("smaller than the {needed} a request takes before its story, by estimate");
        let remedy = "; context_window in [models.qwen] gives the window";
        assert_eq!(
            refused(&settings, Some(needed - 1)),
            format!(
                "the context window the provider reports for qwen is {} tokens, {too_small}{remedy}",
                needed - 1
            )
        );
        assert_eq!(
            refused(&settings, None),
            format!("the default context window for qwen is 4096 tokens, {too_small}{remedy}")
        );
        let openai = PLAIN.replace("http://localhost:1234/v1", "https://api.openai.com/v1");
        let enormous = prompts_of_length(&settings, 30, 400_000);
        let error = settle(&openai, Overrides::default())
            .context_window("qwen", None, &enormous)
            .unwrap_err();
        assert!(
            format!("{error:#}")
                .starts_with("the provider's default context window for qwen is 128000 tokens"),
            "{error:#}"
        );
        let file = PLAIN.replace("id = ", "context_window = 2000\nid = ");
        assert_eq!(
            refused(&settle(&file, Overrides::default()), None),
            format!("[models.qwen] context_window is 2000 tokens, {too_small}")
        );
    }

    #[test]
    fn an_omitted_key_variable_means_no_key() {
        let settings = settle(PLAIN, Overrides::default());
        assert!(settings.config.models["qwen"].api_key().unwrap().is_none());
    }

    #[test]
    fn the_key_is_read_from_the_named_variable_and_never_shown() {
        // A variable Cargo sets for every test, since setting one is unsafe.
        let name = "CARGO_MANIFEST_DIR";
        let value = std::env::var(name).unwrap();
        let file = PLAIN.replace("id = ", &format!("api_key_env = \"{name}\"\nid = "));
        let settings = settle(&file, Overrides::default());
        let key = settings.config.models["qwen"].api_key().unwrap().unwrap();
        assert_eq!(key.expose_secret(), value);
        let shown = format!("{key:?}\n{settings:?}");
        assert!(shown.contains(name), "{shown}");
        assert!(!shown.contains(&value), "{shown}");
    }

    #[test]
    fn a_known_provider_s_key_variable_is_filled_in_unless_the_file_names_one() {
        let openai = PLAIN.replace("http://localhost:1234/v1", "https://api.openai.com/v1");
        let settings = settle(&openai, Overrides::default());
        assert_eq!(
            settings.config.models["qwen"].key_variable(),
            Some("OPENAI_API_KEY")
        );
        let named = openai.replace("id = ", "api_key_env = \"MY_KEY\"\nid = ");
        let settings = settle(&named, Overrides::default());
        assert_eq!(
            settings.config.models["qwen"].key_variable(),
            Some("MY_KEY")
        );
    }

    /// The model in play the configuration tests record, made to call a
    /// tool, with the window its file gives.
    fn qwen() -> ModelInPlay {
        ModelInPlay {
            base_url: "http://localhost:1234/v1".to_string(),
            id: "qwen2.5-7b-instruct".to_string(),
            request_timeout: Duration::from_secs(10),
            api_key_env: None,
            tool_choice: ToolChoice::Required,
            context_window: ContextWindow {
                tokens: 8192,
                source: Source::File,
            },
        }
    }

    /// A file saying something under every table, for a game of one
    /// werewolf, one seer and one villager.
    fn configured() -> String {
        format!(
            "{}
[phases]
night_limit = \"30s\"
day_limit = \"1m 30s\"
ratify_limit = \"10s\"
[text]
rules = \"The rules.\"
[roles.werewolf]
strategy = \"bold\"
count = 1
[roles.villager.strategies]
dice = 1
[roles.doctor]
count = 0
[roles.seer]
count = 1
",
            three()
                .replace("{{ name }} the", "{{ rules }} {{ name }} the")
                .replace(
                    "id = ",
                    "request_timeout = \"10s\"\ncontext_window = 8192\nid = "
                )
        )
    }

    /// A deal at the table [`configured`] sets.
    fn dealt() -> HashMap<PlayerId, Dealt> {
        let dealt = |role, strategy: &str| Dealt {
            role,
            strategy: strategy.to_string(),
        };
        HashMap::from([
            ("player1".to_string(), dealt(Role::Villager, "dice")),
            ("player2".to_string(), dealt(Role::Werewolf, "bold")),
            ("player3".to_string(), dealt(Role::Seer, "plain")),
        ])
    }

    /// The configuration of a game dealt `deal` under `settings`, with
    /// every prompt rendered and [`qwen`] in play.
    fn configuration_of(
        settings: &Settings,
        deal: &HashMap<PlayerId, Dealt>,
    ) -> anyhow::Result<Configuration> {
        let prompts = settings.prompts()?;
        let models = Models::from([("qwen".to_string(), qwen())]);
        settings.configuration(deal, &prompts, models)
    }

    #[test]
    fn the_configuration_carries_the_settings_as_they_took_effect() {
        // The night limit from the command line, everything else from the
        // file or the code.
        let overrides = Overrides {
            limits: PhaseLimits {
                night_limit: Some(Duration::from_secs(5)),
                day_limit: None,
            },
            ..Overrides::default()
        };
        let configuration = configuration_of(&settle(&configured(), overrides), &dealt()).unwrap();
        let prompted = |system: &str| {
            Strategy::Prompted(Prompted {
                system: system.to_string(),
                model: None,
            })
        };
        assert_eq!(
            configuration,
            Configuration {
                role_counts: BTreeMap::from([
                    (Role::Werewolf, 1),
                    (Role::Villager, 1),
                    (Role::Doctor, 0),
                    (Role::Seer, 1),
                ]),
                night_limit: Duration::from_secs(5),
                day_limit: Duration::from_secs(90),
                ratify_limit: Duration::from_secs(10),
                strategies: BTreeMap::from([
                    ("bold".to_string(), prompted("Bold {{ name }}.")),
                    ("dice".to_string(), Strategy::Random(Random {})),
                    (
                        "plain".to_string(),
                        prompted("{{ rules }} {{ name }} the {{ role }}")
                    ),
                    (RANDOM.to_string(), Strategy::Random(Random {})),
                ]),
                allotments: Allotments::from([
                    (Role::Werewolf, allotted(&[("bold", 1)])),
                    (Role::Villager, allotted(&[("dice", 1)])),
                    (Role::Doctor, allotted(&[("plain", 0)])),
                    (Role::Seer, allotted(&[("plain", 1)])),
                ]),
                models: Models::from([("qwen".to_string(), qwen())]),
                prompts: BTreeMap::from([
                    ("player2".to_string(), "Bold player2.".to_string()),
                    (
                        "player3".to_string(),
                        "The rules. player3 the seer".to_string()
                    ),
                ]),
                text: BTreeMap::from([("rules".to_string(), "The rules.".to_string())]),
            }
        );
    }

    #[test]
    fn a_prompted_player_with_no_prompt_rendered_is_an_error_naming_the_seat_the_role_and_the_strategy()
     {
        let mut deal = dealt();
        deal.insert(
            "player3".to_string(),
            Dealt {
                role: Role::Doctor,
                strategy: "plain".to_string(),
            },
        );
        let error =
            configuration_of(&settle(&configured(), Overrides::default()), &deal).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "no system prompt rendered for player3 as the doctor playing plain"
        );
    }

    #[test]
    fn the_configuration_entry_writes_durations_as_humantime_does_and_survives_a_trip_through_json()
    {
        let configuration =
            configuration_of(&settle(&configured(), Overrides::default()), &dealt()).unwrap();
        let entry = Entry::Configuration(Box::new(configuration));
        let json = serde_json::to_value(&entry).unwrap();
        let configuration = &json["Configuration"];
        assert_eq!(configuration["night_limit"], json!("30s"));
        assert_eq!(configuration["day_limit"], json!("1m 30s"));
        assert_eq!(configuration["ratify_limit"], json!("10s"));
        let qwen = &configuration["models"]["qwen"];
        assert_eq!(qwen["request_timeout"], json!("10s"));
        assert_eq!(
            qwen["context_window"],
            json!({"tokens": 8192, "source": "file"})
        );
        assert_eq!(
            configuration["strategies"]["dice"],
            json!({"policy": "random"})
        );
        // A role is a key by its name.
        assert_eq!(configuration["role_counts"]["Werewolf"], json!(1));
        assert_eq!(configuration["allotments"]["Villager"], json!({"dice": 1}));
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, entry);
        // So does one of random players, with no models.
        let settings = settle("", Overrides::default());
        let configuration = settings
            .configuration(&settings.deal(), &[], Models::new())
            .unwrap();
        assert!(configuration.models.is_empty() && configuration.prompts.is_empty());
        let entry = Entry::Configuration(Box::new(configuration));
        let back: Entry = serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back, entry);
    }

    /// The second model in play the configuration tests record, asked to
    /// call a tool, with the default window.
    fn claude() -> ModelInPlay {
        ModelInPlay {
            base_url: "https://api.anthropic.com/v1".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            tool_choice: ToolChoice::Auto,
            context_window: ContextWindow {
                tokens: CONTEXT_WINDOW,
                source: Source::Default,
            },
            ..qwen()
        }
    }

    #[test]
    fn a_configuration_of_two_models_survives_a_trip_through_json_and_is_told_model_by_model() {
        let settings = settle(&configured(), Overrides::default());
        let models = Models::from([
            ("claude".to_string(), claude()),
            ("qwen".to_string(), qwen()),
        ]);
        let configuration = settings
            .configuration(&dealt(), &settings.prompts().unwrap(), models)
            .unwrap();
        let entry = Entry::Configuration(Box::new(configuration));
        let json = serde_json::to_value(&entry).unwrap();
        let names: Vec<&String> = json["Configuration"]["models"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(names, ["claude", "qwen"]);
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, entry);
        assert_eq!(
            Narrator::default().narrate(&logged(entry)),
            [
                "claude's context window is 4096 tokens, the default.",
                "claude does not accept a required tool call, \
                 so its players are asked to call one.",
                "qwen's context window is 8192 tokens, as the configuration file says.",
            ]
        );
    }

    #[test]
    fn the_configuration_names_the_key_s_variable_and_never_holds_the_key() {
        // A variable Cargo sets for every test, since setting one is unsafe.
        let name = "CARGO_MANIFEST_DIR";
        let value = std::env::var(name).unwrap();
        let model = ModelInPlay {
            api_key_env: Some(name.to_string()),
            ..qwen()
        };
        let settings = settle(&configured(), Overrides::default());
        let configuration = settings
            .configuration(
                &dealt(),
                &settings.prompts().unwrap(),
                Models::from([("qwen".to_string(), model)]),
            )
            .unwrap();
        let shown = format!(
            "{configuration:?}\n{}",
            serde_json::to_string(&configuration).unwrap()
        );
        assert!(shown.contains(name), "{shown}");
        assert!(!shown.contains(&value), "{shown}");
    }

    #[tokio::test(start_paused = true)]
    async fn each_prompted_player_asks_its_strategy_s_model_with_that_model_s_id_tool_choice_and_window()
     {
        // The werewolves play `plain`, asking `qwen`, and the villagers
        // `smart`, asking `claude`.
        let file = two_models(
            "[roles.werewolf]
strategy = \"plain\"
[roles.villager]
strategy = \"smart\"
count = 5
[roles.doctor]
count = 0
[roles.seer]
count = 0
",
        )
        .replace("Smart.", "Smart {{ name }}.");
        let settings = settle(&file, Overrides::default());
        let deal = settings.deal();
        let prompts = settings.prompts().unwrap();
        let models = Models::from([
            ("claude".to_string(), claude()),
            ("qwen".to_string(), qwen()),
        ]);
        let configuration = settings.configuration(&deal, &prompts, models).unwrap();
        let fakes = [
            ("claude", Arc::new(FakeModel::answering(choosing_first))),
            ("qwen", Arc::new(FakeModel::answering(choosing_first))),
        ];
        let handles: Handles = fakes
            .iter()
            .map(|(name, fake)| (name.to_string(), Arc::clone(fake) as Arc<dyn model::Model>))
            .collect();
        let players = players(&configuration, &deal, &handles);
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let rules = settings.rules();
        let roles: HashMap<PlayerId, Role> = deal
            .iter()
            .map(|(seat, dealt)| (seat.clone(), dealt.role))
            .collect();
        let episode = game(
            deal.clone(),
            settings.rules(),
            configuration.clone(),
            winner,
            players,
            logger,
        );
        let (winner, entries, _) = run(episode, won, log).await.unwrap();
        between_the_deal_and_the_end(&entries, &rules, &roles, winner);
        // Each model was asked by its strategy's players alone, with its own
        // id and tool choice, and within its own window.
        for (name, fake) in &fakes {
            let model = &configuration.models[*name];
            let requests = fake.requests();
            assert!(!requests.is_empty(), "{name}");
            for request in requests {
                assert_eq!(request.model, model.id, "{name}");
                assert_eq!(request.tool_choice, model.tool_choice, "{name}");
                assert!(
                    model::estimated(&request) <= model.context_window.tokens,
                    "{name}"
                );
                let system = &request.messages[0].content;
                let (seat, _) = configuration
                    .prompts
                    .iter()
                    .find(|(_, prompt)| *prompt == system)
                    .unwrap();
                assert_eq!(settings.model_of(&deal[seat].strategy), Some(*name));
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_table_of_random_werewolves_and_prompted_villagers_plays_to_a_winner() {
        // A werewolf and two villagers of each policy, the prompted players
        // asking one model, which answers as the canned player does.
        let file = format!(
            "{PLAIN}
[roles.werewolf.strategies]
random = 1
plain = 1
[roles.villager.strategies]
plain = 2
random = 2
[roles.doctor]
count = 0
[roles.seer]
count = 0
"
        );
        let settings = settle(&file, Overrides::default());
        let deal = settings.deal();
        let prompts = settings.prompts().unwrap();
        let models = Models::from([("qwen".to_string(), qwen())]);
        let configuration = settings.configuration(&deal, &prompts, models).unwrap();
        let handles = Handles::from([(
            "qwen".to_string(),
            Arc::new(FakeModel::answering(choosing_first)) as Arc<dyn model::Model>,
        )]);
        let players = players(&configuration, &deal, &handles);
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let rules = settings.rules();
        let roles: HashMap<PlayerId, Role> = deal
            .iter()
            .map(|(seat, dealt)| (seat.clone(), dealt.role))
            .collect();
        let episode = game(
            deal.clone(),
            settings.rules(),
            configuration,
            winner,
            players,
            logger,
        );
        let (winner, entries, _) = run(episode, won, log).await.unwrap();
        between_the_deal_and_the_end(&entries, &rules, &roles, winner);
        // The log opens with each player's strategy beside its role.
        assert!(
            matches!(&entries[0], Entry::Start { strategies, .. }
                if *strategies == deal.iter().map(|(seat, dealt)| (seat.clone(), dealt.strategy.clone())).collect::<HashMap<_, _>>()),
            "{entries:?}"
        );
        // Each selection is made as its player's policy makes it: a random
        // player's with no model to have been shown anything, a prompted
        // player's by a model shown its memories. Both werewolves hunt the
        // first night, and everyone left alive votes the first day.
        let prompted: HashSet<&PlayerId> = deal
            .iter()
            .filter(|(_, dealt)| dealt.strategy == "plain")
            .map(|(seat, _)| seat)
            .collect();
        let mut selected = HashSet::new();
        let mut first_victim = None;
        for entry in &entries {
            if let Entry::Action {
                from,
                message: Message::Select { .. },
                seen,
                ..
            } = entry
            {
                assert_eq!(seen.is_some(), prompted.contains(from), "{from}");
                selected.insert(from);
            }
            if let Entry::Hunted { seq: 1, hunt } = entry {
                first_victim = hunt.dead();
            }
        }
        let survived_the_first_night: HashSet<&PlayerId> = deal
            .keys()
            .filter(|seat| Some(*seat) != first_victim)
            .collect();
        assert!(
            selected.is_superset(&survived_the_first_night),
            "{entries:?}"
        );
        // The table is told with every player's strategy.
        let mut narrator = Narrator::default();
        let told = narrator.narrate(&logged(entries[0].clone()));
        assert_eq!(told[0], "6 players:");
        for (seat, dealt) in &deal {
            let line = format!("{seat}, a {}, plays {}", dealt.role, dealt.strategy);
            assert!(told.contains(&line), "{line} in {told:?}");
        }
    }
}
