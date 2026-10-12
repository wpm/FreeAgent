//! The model-played variant: every player is a language model, set up by
//! a TOML configuration file.
//!
//! The configuration is the [`Config`] the file is parsed into, the
//! [`Overrides`] the command line may give, and the [`Settings`] that take
//! effect once the two are combined with the defaults in the code. A
//! setting comes from the command line if it is given there, otherwise
//! from the file, otherwise from the code.
//!
//! Each player's system prompt is a `minijinja` template from the file,
//! rendered for its seat and role. [`Settings::prompts`] renders every
//! [`SystemPrompt`] a game could call for, so that a broken template stops
//! the program before any game begins.
//!
//! The API key never appears in the file. The file names the environment
//! variable that holds it, and [`Model::api_key`] reads that variable into
//! a [`SecretString`], which is redacted wherever it is shown.
//!
//! The model's [`ContextWindow`] is settled before any game begins: from
//! the command line, otherwise from the file, otherwise from the provider's
//! listing of the model, otherwise from the code.
//!
//! The game is the announcing environment's, with a model [`Player`] in
//! every seat, and the day's vote is ratified: a day ends one ratify limit
//! after its votes stop changing, or at the day limit with nobody voted
//! out. A player remembers what it is shown, what it hears and what it
//! does, as a [`Memory`] of each message stamped when it was received or
//! sent, and keeps with the history the story that tells it from its own
//! point of view: each memory adds its lines to the story as it is
//! remembered, so the story is never told over. Every living player is
//! announced every phase, so a player knows which phase it is in, and it
//! ignores a message tagged with any other: it logs the receipt, as it does
//! every message's, and neither remembers the message nor wakes for it.
//! Each time it is woken, it tells the model the story so far and where the
//! game stands, with how long the phase lasts, and carries out what the
//! model answers. The call is made in the player's think loop, so the
//! player keeps perceiving while the model works.
//!
//! By night a player is woken once, by its announcement, and has the model
//! select one of its [candidates] with one `select` call, sent to
//! the environment alone. By day it is woken by every message it receives:
//! the announcement, another player's utterance or selection, and the
//! environment's tentative decision. Wake-ups are debounced: the perceive
//! loop remembers each message and wakes the think loop only when no
//! wake-up is already waiting, and the think loop takes the history as it
//! stands when it is woken, so a player has at most one call in progress
//! and one waiting, however fast the talk comes. A day's request offers
//! three tools, of which the model calls one and may call several:
//! `speak`, which sends what it says to every other living player and not
//! to the environment; `select`, which sends its vote to the environment
//! and every other living player, so the votes are public; and `wait`,
//! which asks to be woken after some seconds unless something wakes the
//! player sooner. A reminder is numbered so that only the latest set
//! counts, and one for a phase that has ended is ignored. The calls are
//! carried out in the order the model made them. Before carrying out
//! anything a response asks for, by night as well as by day, the player
//! checks that the phase the call began in is still the one it was last
//! announced; if not, it sends nothing and logs each message it would have
//! sent as [`Entry::Withheld`].
//!
//! A model that is not reached, or an answer that is not well formed, is a
//! failure: the player logs it as [`Entry::Failed`], carries nothing out,
//! and the game goes on. The player logs an [`Entry::Observation`] for each
//! message it receives and an [`Entry::Action`] for each utterance and
//! selection it sends, numbered together from 1, with how many memories the
//! model was shown when it made the call that produced the message. Before
//! a game begins, [`check_plays`] asks the model once as a player would by
//! night and once as by day, so a model that does not answer either request
//! well fails at launch, in the provider's words. Every request makes the
//! model call a tool, unless the probe finds the model refuses that, and
//! then every request asks it to call one, and an answer in prose is a
//! failure like any other. [`game`] builds the
//! episode from the deal, the rules, the model, and the [`Configuration`]
//! the log opens with, which [`Settings::configuration`] records from the
//! settings, the deal and the rendered prompts.

use super::announced;
use super::report::Narrator;
use super::{
    Entry, Message, Phase, PhaseLimits, PlayerId, Role, RoleCounts, Rules, Table, Team, View, act,
    awake, candidates, observe, receivers,
};
use crate::model::{
    self, CONTEXT_WINDOW, Provider, REQUEST_TIMEOUT, Request, Response, Tool, ToolChoice, api_key,
    key_variable,
};
use anyhow::{Context as _, anyhow, bail};
use async_trait::async_trait;
use clap::Args;
use free_agent::{
    ActorId, Behavior, Builder, Context, Episode, Event, Logger, Think, ThinkBuilder,
};
use humantime::format_duration;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use secrecy::SecretString;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// What the command line may say over the file: the role counts, the
/// phase limits, the ratify limit, and the model. Each is unset unless
/// given, so that the file can be heard.
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// How many of each role sit at the table.
    #[command(flatten)]
    pub roles: RoleCounts,
    /// How long each phase waits for the players.
    #[command(flatten)]
    pub limits: PhaseLimits,
    /// How long a day's votes must stand to be ratified.
    #[arg(long, value_parser = humantime::parse_duration, help = ratify_help())]
    pub ratify_limit: Option<Duration>,
    /// The model and where it is.
    #[command(flatten)]
    pub model: ModelOverrides,
}

/// The help for `--ratify-limit`, naming the default the code fills in.
fn ratify_help() -> String {
    format!(
        "How long a day's votes must stand to be ratified [default: {}]",
        format_duration(RATIFY_LIMIT)
    )
}

/// What the command line may say over the file's `[model]` table, for
/// trying another model or provider without editing the file. Each is
/// unset unless given. The help lists them under a heading of their own.
#[derive(Args, Debug, Default, PartialEq, Eq)]
#[command(next_help_heading = "Model")]
pub struct ModelOverrides {
    /// The root of the provider's OpenAI-compatible API, ending in /v1
    #[arg(long = "model-base-url", value_name = "BASE_URL")]
    pub base_url: Option<String>,
    /// The model's id, exactly as the provider lists it
    #[arg(long = "model-id", value_name = "ID")]
    pub id: Option<String>,
    /// The model's context window, over what the provider reports
    #[arg(long, value_name = "TOKENS", help = context_window_help())]
    pub context_window: Option<u64>,
}

/// The help for `--context-window`, naming the default the code fills in.
fn context_window_help() -> String {
    format!(
        "The model's context window in tokens, over what the provider reports [default: {CONTEXT_WINDOW}]"
    )
}

/// The configuration file, as written. Every table but `[model]` may be
/// left out, and an unknown key anywhere is an error, so a misspelled
/// setting fails loudly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The model every player uses and how to reach it.
    pub model: Model,
    /// How long each phase waits for the players.
    #[serde(default)]
    pub phases: Phases,
    /// How many of each role there are, and what each is told.
    #[serde(default)]
    pub roles: Roles,
    /// Named blocks of text for the templates to use.
    #[serde(default)]
    pub text: BTreeMap<String, String>,
    /// The templates every role falls back on.
    #[serde(default)]
    pub prompt: Prompt,
    /// What each seat, `player1` onward, is like, for the seats that are
    /// anyone in particular.
    #[serde(default)]
    pub personas: BTreeMap<String, String>,
}

/// The `[model]` table: one model from one provider for the whole table.
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

    /// The provider the table is reached through, with its key read from
    /// the environment as [`Model::api_key`] reads it.
    pub fn provider(&self) -> anyhow::Result<Provider> {
        Provider::new(&self.base_url, self.api_key()?, self.request_timeout)
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
    /// The role's own system prompt template, over the default.
    pub system: Option<String>,
}

/// The `[prompt]` table: the templates every role falls back on.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    /// The default system prompt template.
    pub system: Option<String>,
}

/// The settings that took effect: the file, with whatever the command
/// line said over its model, and the settings the command line and the
/// code have a say in, decided.
#[derive(Debug)]
pub struct Settings {
    /// The file as written, but for the model, which is as the command
    /// line said if it said anything.
    pub config: Config,
    /// How many of each role sit at the table.
    pub table: Table,
    /// How long the night waits for a player.
    pub night_limit: Duration,
    /// How long the day waits for a player.
    pub day_limit: Duration,
    /// How long a day's votes must stand to be ratified.
    pub ratify_limit: Duration,
    /// The model's context window, when the command line or the file
    /// gives one.
    pub context_window: Option<ContextWindow>,
}

impl Config {
    /// Read and parse the file at `path`. Reading it says nothing about
    /// the API key, so a file can be checked without the key being set.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&read(path)?).with_context(|| format!("in {}", path.display()))
    }

    /// Parse `file`, the text of a configuration file. A `[text]` block
    /// named like one of the template variables is an error, since the
    /// templates could not tell them apart.
    pub fn parse(file: &str) -> anyhow::Result<Self> {
        let config: Self = toml::from_str(file)?;
        if let Some(taken) = variables()
            .into_iter()
            .find(|v| config.text.contains_key(*v))
        {
            bail!("[text] has a block named {taken}, which is a template variable");
        }
        Ok(config)
    }

    /// The settings that take effect with `overrides` from the command
    /// line: each role count and limit from the command line if given
    /// there, otherwise from the file, otherwise from the code, and the
    /// model's provider and id from the command line if given there,
    /// otherwise from the file, and the context window from the command
    /// line if given there, otherwise from the file if given there. A ratify
    /// limit longer than the day limit is an error naming both, since the
    /// day would run out before any vote stood, and a context window of 0
    /// is an error naming the setting that gave it.
    pub fn settle(mut self, overrides: Overrides) -> anyhow::Result<Settings> {
        if let Some(base_url) = overrides.model.base_url {
            self.model.base_url = base_url;
        }
        if let Some(id) = overrides.model.id {
            self.model.id = id;
        }
        let file = RoleCounts {
            werewolves: self.roles.werewolf.count,
            villagers: self.roles.villager.count,
            doctors: self.roles.doctor.count,
            seers: self.roles.seer.count,
        };
        let rules = Rules::default();
        let day_limit = overrides
            .limits
            .day_limit
            .or(self.phases.day_limit)
            .unwrap_or(rules.day_limit);
        let ratify_limit = overrides
            .ratify_limit
            .or(self.phases.ratify_limit)
            .unwrap_or(RATIFY_LIMIT);
        if ratify_limit > day_limit {
            bail!(
                "the ratify limit ({}) is longer than the day limit ({})",
                format_duration(ratify_limit),
                format_duration(day_limit)
            );
        }
        let given = |tokens, source| ContextWindow { tokens, source };
        let context_window = overrides
            .model
            .context_window
            .map(|tokens| given(tokens, Source::CommandLine))
            .or(self
                .model
                .context_window
                .map(|tokens| given(tokens, Source::File)));
        if let Some(ContextWindow { tokens: 0, source }) = context_window {
            let setting = match source {
                Source::CommandLine => "--context-window",
                _ => "context_window",
            };
            bail!("{setting} is 0; a context window holds at least 1 token");
        }
        Ok(Settings {
            table: overrides.roles.or(file.or(Table::default())),
            night_limit: overrides
                .limits
                .night_limit
                .or(self.phases.night_limit)
                .unwrap_or(rules.night_limit),
            day_limit,
            ratify_limit,
            context_window,
            config: self,
        })
    }
}

/// The variables every template has besides the text blocks, for the seat
/// `name` dealt `role`, with the seat's `persona`.
fn player(name: &str, role: &str, persona: &str) -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        ("name", Value::from(name)),
        ("role", Value::from(role)),
        ("persona", Value::from(persona)),
    ])
}

/// The names of the variables every template has besides the text blocks.
fn variables() -> Vec<&'static str> {
    player("", "", "").into_keys().collect()
}

/// One player's system prompt, rendered for a seat dealt a role.
#[derive(Debug)]
pub struct SystemPrompt {
    /// The seat, `player1` onward.
    pub seat: PlayerId,
    /// The role the seat was dealt.
    pub role: Role,
    /// The rendered prompt.
    pub text: String,
}

impl Settings {
    /// Render the system prompt of every seat for every role at the table,
    /// since any seat may be dealt any of them, in seat order and then the
    /// order the roles are dealt. A role's prompt is rendered from its own
    /// `[roles.<role>] system` template if the file has one, otherwise
    /// from `[prompt] system`, with the variables `name`, `role` and
    /// `persona` and every `[text]` block. An undefined variable is an
    /// error, as is a role at the table with no template or a persona for
    /// a seat that is not there. A template that does not compile is an
    /// error naming the role; one that does not render, the seat and the
    /// role.
    pub fn prompts(&self) -> anyhow::Result<Vec<SystemPrompt>> {
        let config = &self.config;
        let seats: Vec<_> = self.table.seats().collect();
        if let Some(seat) = config.personas.keys().find(|seat| !seats.contains(seat)) {
            bail!("[personas] names {seat}, which is not a seat at this table");
        }
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        // Each role's template is compiled once, and rendered once a seat.
        let mut templates = Vec::new();
        for (role, _) in self.table.counts().into_iter().filter(|&(_, n)| n > 0) {
            let source = config
                .roles
                .get(role)
                .system
                .as_deref()
                .or(config.prompt.system.as_deref())
                .ok_or_else(|| {
                    anyhow!("no system prompt for the {role}: neither [roles.{role}] nor [prompt] has one")
                })?;
            let template = env
                .template_from_str(source)
                .with_context(|| format!("compiling the {role} prompt template"))?;
            templates.push((role, template));
        }
        let blocks = Value::from(&config.text);
        let mut prompts = Vec::new();
        for seat in &seats {
            let persona = config.personas.get(seat).map_or("", String::as_str);
            for (role, template) in &templates {
                let role = *role;
                let own = Value::from(player(seat, &role.to_string(), persona));
                let text = template
                    .render(context! { ..own, ..blocks.clone() })
                    .with_context(|| format!("rendering the {role} prompt for {seat}"))?;
                prompts.push(SystemPrompt {
                    seat: seat.clone(),
                    role,
                    text,
                });
            }
        }
        Ok(prompts)
    }

    /// The configuration a game of `roles` runs under, as the log records
    /// it right after the deal: every setting as it took effect, each
    /// player's system prompt for the role it was dealt from `prompts`,
    /// which [`Settings::prompts`] rendered for every seat and role at the
    /// table, and the templates and text blocks as written. The key's
    /// variable is named, and the key itself never recorded. Each request
    /// is made under `tool_choice`, as the probe found, within
    /// `context_window`, as [`Settings::context_window`] settled it. A
    /// player with no prompt rendered for its role is an error naming the
    /// seat and the role.
    pub fn configuration(
        &self,
        roles: &HashMap<PlayerId, Role>,
        prompts: &[SystemPrompt],
        tool_choice: ToolChoice,
        context_window: ContextWindow,
    ) -> anyhow::Result<Configuration> {
        let rendered: HashMap<(&PlayerId, Role), &str> = prompts
            .iter()
            .map(|prompt| ((&prompt.seat, prompt.role), prompt.text.as_str()))
            .collect();
        let prompts = roles
            .iter()
            .map(|(seat, role)| {
                let text = rendered
                    .get(&(seat, *role))
                    .ok_or_else(|| anyhow!("no system prompt rendered for {seat} as {role}"))?;
                Ok((seat.clone(), text.to_string()))
            })
            .collect::<anyhow::Result<_>>()?;
        let config = &self.config;
        let model = &config.model;
        let counts = self.table.counts();
        Ok(Configuration {
            role_counts: counts.into_iter().collect(),
            night_limit: self.night_limit,
            day_limit: self.day_limit,
            ratify_limit: self.ratify_limit,
            request_timeout: model.request_timeout,
            base_url: model.base_url.clone(),
            model: model.id.clone(),
            tool_choice,
            context_window,
            api_key_env: model.key_variable().map(String::from),
            personas: config.personas.clone(),
            prompts,
            default_template: config.prompt.system.clone(),
            // Every role's own template, at the table or not.
            role_templates: counts
                .into_iter()
                .filter_map(|(role, _)| Some((role, config.roles.get(role).system.clone()?)))
                .collect(),
            text: config.text.clone(),
        })
    }

    /// The model's context window: the one the command line or the file
    /// gives, otherwise `reported`, the one the provider's listing reports
    /// for the model, otherwise [`CONTEXT_WINDOW`].
    pub fn context_window(&self, reported: Option<u64>) -> ContextWindow {
        self.context_window
            .or(reported.map(|tokens| ContextWindow {
                tokens,
                source: Source::Provider,
            }))
            .unwrap_or(ContextWindow {
                tokens: CONTEXT_WINDOW,
                source: Source::Default,
            })
    }

    /// The rules a game is played under: the variant's, with the limits
    /// that took effect.
    pub fn rules(&self) -> Rules {
        Rules {
            night_limit: self.night_limit,
            day_limit: self.day_limit,
            ratify_limit: Some(self.ratify_limit),
            ..rules()
        }
    }
}

/// The default `request_timeout`, for `serde`.
fn request_timeout() -> Duration {
    REQUEST_TIMEOUT
}

/// How long a day's votes must stand to be ratified, unless the file or
/// the command line says otherwise.
pub const RATIFY_LIMIT: Duration = Duration::from_secs(20);

/// The rules of the model-played variant: the uniform-random variant's,
/// played as "LLM", with the day's vote ratified once it has stood for
/// [`RATIFY_LIMIT`].
pub fn rules() -> Rules {
    Rules {
        variation: "LLM".to_string(),
        ratify_limit: Some(RATIFY_LIMIT),
        ..Rules::default()
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

/// Where a setting came from, written as `command line`, `file`,
/// `provider` or `default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The command line.
    #[serde(rename = "command line")]
    CommandLine,
    /// The configuration file.
    File,
    /// The provider.
    Provider,
    /// The code's default.
    Default,
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
    /// How long each model request could take.
    #[serde(with = "humantime_serde")]
    pub request_timeout: Duration,
    /// The root of the provider's OpenAI-compatible API.
    pub base_url: String,
    /// The model's id, as the provider lists it.
    pub model: String,
    /// Whether each request made the model call a tool, as the probe
    /// found the model would.
    pub tool_choice: ToolChoice,
    /// The model's context window and where it came from.
    pub context_window: ContextWindow,
    /// The name of the environment variable the key came from, if any.
    pub api_key_env: Option<String>,
    /// Each seat's persona, for the seats that have one.
    pub personas: BTreeMap<PlayerId, String>,
    /// Each player's rendered system prompt, for the role it was dealt.
    pub prompts: BTreeMap<PlayerId, String>,
    /// The default system prompt template as written, if any.
    pub default_template: Option<String>,
    /// Each role's own system prompt template as written, for the roles
    /// that have one.
    pub role_templates: BTreeMap<Role, String>,
    /// The `[text]` blocks as written.
    pub text: BTreeMap<String, String>,
}

/// An episode of a game of `roles` under `rules`: the announcing
/// environment, which may reach and stop every player and holds `logger`,
/// and a model player for each role, who may send to the environment and
/// to every other player. The environment logs `configuration` right after
/// the deal. Each player is told its system prompt, which `configuration`
/// has for every player in `roles`, and makes each selection by asking
/// `model` as the model `configuration` names. The winning team is sent on
/// `winner` when the game ends.
pub fn game(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    configuration: Configuration,
    model: Arc<dyn model::Model>,
    winner: oneshot::Sender<Team>,
    logger: Logger<Entry>,
) -> Episode<Actor> {
    let mut prompts = configuration.prompts.clone();
    let model_id = configuration.model.clone();
    let tool_choice = configuration.tool_choice;
    let players: Vec<PlayerId> = roles.keys().cloned().collect();
    let environment = announced::init(
        roles,
        rules,
        Some(Entry::Configuration(Box::new(configuration))),
        winner,
        Actor::Environment,
    );
    let player = |id: &PlayerId| {
        let prompt = prompts
            .remove(id)
            .unwrap_or_else(|| panic!("no system prompt for {id}"));
        Player::init(
            id.clone(),
            prompt,
            model_id.clone(),
            tool_choice,
            Arc::clone(&model),
        )
    };
    super::thinking_episode(environment, players, player, logger)
}

/// What an actor in the game does: run it, or play in it. An episode holds
/// one kind of actor, so the two sides meet here and each method goes to
/// whichever side this is.
pub enum Actor {
    /// The side that holds the game.
    Environment(announced::Environment),
    /// A side that sees only what it is shown.
    Player(Player),
    /// A player who says whatever its script says.
    #[cfg(test)]
    Puppet(super::uniform_random::tests::Puppet),
}

#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Player(player) => player.context(),
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.context(),
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.receive(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Player(player) => player.start().await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.start().await,
        }
    }
}

/// What a player remembers: a message it received from another actor in
/// the phase it was in, or one it sent. Reminders to itself, messages from
/// another phase, and what it withheld, are not remembered.
#[derive(Debug, Clone, PartialEq)]
pub enum Memory {
    /// The player received `message`, whose [`Message::sender`] sent it.
    Observed {
        /// What was sent.
        message: Message,
    },
    /// The player sent `message`.
    Acted {
        /// What it sent.
        message: Message,
    },
}

/// What a player's two loops share: what it has been shown and what it has
/// done, with the story that tells it, the phase it is in, whether a
/// wake-up is waiting, how many reminders it has set, what it is told, and
/// whom it asks.
struct Mind {
    /// The history and the story told from it. Never locked across an
    /// await.
    remembered: Mutex<Remembered>,
    /// The number of the last message the player sent to others.
    n: AtomicU64,
    /// The latest phase the player was announced: the phase it is in, so far
    /// as it knows. Never locked across an await.
    announced: Mutex<Option<Announced>>,
    /// Whether a [`Message::Wake`] is waiting for the think loop. The
    /// perceive loop hands the think loop one only when none is waiting,
    /// and the think loop clears this before it takes the history, so
    /// however fast messages come the player has at most one call in
    /// progress and one waiting, and nothing it heard is left out of both.
    waking: AtomicBool,
    /// How many reminders the player has set, which numbers them so that
    /// only the latest counts.
    reminders: AtomicU64,
    /// Its system prompt, rendered for its seat and role.
    prompt: String,
    /// The model's id, as each request names it.
    model_id: String,
    /// Whether each request makes the model call a tool.
    tool_choice: ToolChoice,
    /// The model asked each time the player is woken.
    model: Arc<dyn model::Model>,
}

/// A player's history and the story that tells it, kept together so that
/// the story is always the narration of exactly the history, and a request
/// joins the story's lines instead of narrating every memory again.
struct Remembered {
    /// The history, in order: each message received and each selection or
    /// utterance sent, stamped when the player remembered it.
    history: Vec<Event<Memory>>,
    /// The player's narrator, which has read every memory in the history.
    narrator: Narrator,
    /// The lines the narrator told from the history, in order.
    story: Vec<String>,
}

/// A phase as a player was announced it: its number, what the player may
/// see of it, and how long it lasts.
#[derive(Clone)]
struct Announced {
    seq: u64,
    view: View,
    limit: Duration,
}

impl Mind {
    /// The mind of `me` with nothing in its history yet, told `prompt`,
    /// asking `model` as the model `model_id` under `tool_choice`.
    fn new(
        me: PlayerId,
        prompt: String,
        model_id: String,
        tool_choice: ToolChoice,
        model: Arc<dyn model::Model>,
    ) -> Self {
        Self {
            remembered: Mutex::new(Remembered {
                history: Vec::new(),
                narrator: Narrator::for_player(me),
                story: Vec::new(),
            }),
            n: AtomicU64::new(0),
            announced: Mutex::new(None),
            waking: AtomicBool::new(false),
            reminders: AtomicU64::new(0),
            prompt,
            model_id,
            tool_choice,
            model,
        }
    }

    /// Add `event` to the end of the history, and what it adds to the
    /// account to the end of the story, together. A message is remembered
    /// when it is received or sent, so a selection sent in the moment
    /// between the check that its phase is still on and the send comes
    /// after the next phase's announcement, and is told in that phase.
    fn remember(&self, event: Event<Memory>) {
        let mut remembered = self.remembered.lock().unwrap();
        let lines = remembered.narrator.recall(&event);
        remembered.story.extend(lines);
        remembered.history.push(event);
    }

    /// The story so far, its lines joined, and how many memories it was
    /// told from.
    fn story(&self) -> (String, usize) {
        let remembered = self.remembered.lock().unwrap();
        (remembered.story.join("\n"), remembered.history.len())
    }

    /// The next number for a message to others: one up from the last.
    fn number(&self) -> u64 {
        self.n.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Take in the announcement of the phase numbered `seq`, showing `view`
    /// and lasting `limit`: the phase the player is in from now on.
    fn announce(&self, seq: u64, view: &View, limit: Duration) {
        *self.announced.lock().unwrap() = Some(Announced {
            seq,
            view: view.clone(),
            limit,
        });
    }

    /// The latest phase the player was announced, if any.
    fn announced(&self) -> Option<Announced> {
        self.announced.lock().unwrap().clone()
    }

    /// The number of the latest phase the player was announced, and 0
    /// before any.
    fn seq(&self) -> u64 {
        self.announced
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |announced| announced.seq)
    }

    /// Whether the latest phase the player was announced is a day.
    fn by_day(&self) -> bool {
        self.announced
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|announced| announced.view.phase == Phase::Day)
    }

    /// Mark a wake-up waiting, and say whether none was before: whether
    /// the think loop has to be handed one.
    fn waking(&self) -> bool {
        !self.waking.swap(true, Ordering::Relaxed)
    }

    /// Take the wake-up waiting, so that the next message heard sets
    /// another.
    fn woken(&self) {
        self.waking.store(false, Ordering::Relaxed);
    }

    /// The number of a new reminder: one up from the last, which no longer
    /// counts.
    fn remind(&self) -> u64 {
        self.reminders.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Whether the phase numbered `seq` is the one the player is in: the
    /// latest it was announced.
    fn in_phase(&self, seq: u64) -> bool {
        seq == self.seq()
    }

    /// Whether the reminder numbered `count` for the phase numbered `seq`
    /// counts: it is the latest set, and the phase is the one the player is
    /// in.
    fn reminder_counts(&self, seq: u64, count: u64) -> bool {
        count == self.reminders.load(Ordering::Relaxed) && self.in_phase(seq)
    }
}

/// A player whose decisions are made by a language model. Its perceive
/// loop and its think loop are each a `Player` with a context of its own
/// over one shared mind: the perceive loop remembers each message it is
/// sent and wakes the think loop, which tells the model the history as a
/// story and carries out what the model answers. A variant's actor enum
/// holds the perceive loop's, built by [`Player::init`].
pub struct Player {
    context: Context<Message, Entry>,
    mind: Arc<Mind>,
}

impl Player {
    /// Builds a player's two loops once the episode has made their
    /// contexts: the perceive loop as the actor, and the think loop. The
    /// player `me` is told `prompt`, and asks `model` as the model
    /// `model_id` under `tool_choice`.
    pub fn init(
        me: PlayerId,
        prompt: String,
        model_id: String,
        tool_choice: ToolChoice,
        model: Arc<dyn model::Model>,
    ) -> (Builder<Actor>, Option<ThinkBuilder<Message, Entry>>) {
        let mind = Arc::new(Mind::new(me, prompt, model_id, tool_choice, model));
        let shared = Arc::clone(&mind);
        let behavior: Builder<Actor> =
            Box::new(move |context| Actor::Player(Player { context, mind }));
        let think: ThinkBuilder<Message, Entry> = Box::new(move |context| {
            Box::new(Player {
                context,
                mind: shared,
            })
        });
        (behavior, Some(think))
    }

    /// Add `memory` to the history, stamped now, and what it tells to the
    /// story.
    fn remember(&self, memory: Memory) {
        self.mind.remember(Event::now(self.context.episode, memory));
    }

    /// Wake the think loop, unless a wake-up is already waiting for it.
    fn wake(&self) -> anyhow::Result<()> {
        if self.mind.waking() {
            self.context.think(Message::Wake)?;
        }
        Ok(())
    }

    /// The request for a decision in the phase `view` shows, which lasts
    /// `limit`: the system prompt, then the story so far, a blank line, and
    /// where the game stands, with `tools` to answer with. With it, how
    /// many memories the story was told from.
    fn request(
        &self,
        view: &View,
        limit: Duration,
        tools: Vec<Tool>,
    ) -> anyhow::Result<(Request, usize)> {
        let me = &self.context.id;
        let (story, seen) = self.mind.story();
        let asked = format!("{story}\n\n{}", situation(me, view, limit)?);
        let request = request(
            &self.mind.model_id,
            self.mind.tool_choice,
            &self.mind.prompt,
            asked,
            tools,
        );
        Ok((request, seen))
    }

    /// Woken, decide: take the wake-up, and ask the model about the phase
    /// the player is in, by night for a selection and by day for what to
    /// say, whom to vote for, or how long to wait. If that fails, log the
    /// failure with the phase and what went wrong, causes included, which
    /// is all there is to do about it: the player has not acted, and the
    /// game goes on. The failure is not remembered, so the model is never
    /// told of it. In a phase the player sleeps through, which a wake-up
    /// from the day before can arrive in, or with nobody to select, the
    /// model is not asked. Being woken before any phase was announced is an
    /// error.
    async fn decide(&self) -> anyhow::Result<()> {
        self.mind.woken();
        let Announced { seq, view, limit } = self
            .mind
            .announced()
            .context("woken before any phase was announced")?;
        let me = &self.context.id;
        if !awake(me, &view) {
            return Ok(());
        }
        let candidates = candidates(me, &view);
        if candidates.is_empty() {
            return Ok(());
        }
        let acted = match view.phase {
            Phase::Night => self.select_by_night(seq, &view, limit, &candidates).await,
            Phase::Day => self.play_by_day(seq, &view, limit, &candidates).await,
        };
        if let Err(error) = acted {
            self.context.log(Entry::Failed {
                player: self.context.id.clone(),
                seq,
                error: format!("{error:#}"),
            });
        }
        Ok(())
    }

    /// Ask the model to select one of `candidates` in the night numbered
    /// `seq`, which `view` shows and which lasts `limit`, and send the
    /// selection to the environment, unless the night has ended by the time
    /// the model answers, and then it is withheld. The model's failure to
    /// select, as [`selected`] judges it, is an error, and no selection; so
    /// is a request that cannot be put together or a selection that cannot
    /// be sent, which are the player's own doing, and they read so in the
    /// error.
    async fn select_by_night(
        &self,
        seq: u64,
        view: &View,
        limit: Duration,
        candidates: &[PlayerId],
    ) -> anyhow::Result<()> {
        let (request, seen) = self.request(view, limit, night_tools(candidates))?;
        let response = self.mind.model.complete(&request).await?;
        let target = selected(&response, candidates)?;
        self.carry_out(seq, view, vec![Sending::Select(target)], seen)?;
        Ok(())
    }

    /// Ask the model what to do in the day numbered `seq`, which `view`
    /// shows and which lasts `limit`: speak, vote for one of `candidates`,
    /// or wait, in any number and order, with one response. A response
    /// that is not well formed, as [`asked`] judges it, is an error, and
    /// nothing in it is carried out. Otherwise the messages are sent in the
    /// order the model asked for them, each utterance to every other living
    /// player and each selection to them and the environment, and a wait
    /// sets a reminder; unless the day has ended by the time the model
    /// answers, and then the messages are withheld and no reminder is set.
    async fn play_by_day(
        &self,
        seq: u64,
        view: &View,
        limit: Duration,
        candidates: &[PlayerId],
    ) -> anyhow::Result<()> {
        let (request, seen) = self.request(view, limit, day_tools(candidates, limit))?;
        let response = self.mind.model.complete(&request).await?;
        let Asked { sends, wait } = asked(&response, candidates, limit)?;
        let still_on = self.carry_out(seq, view, sends, seen)?;
        if still_on && let Some(after) = wait {
            self.remind(seq, after)?;
        }
        Ok(())
    }

    /// Carry out `sends`, what a response made from `seen` memories asked
    /// for in the phase numbered `seq`, which `view` shows, if that is
    /// still the phase the player was last announced: send each in order,
    /// numbered, to whom the rules of the game send it. A call may begin in
    /// one phase and end in the next, and then nothing is sent, and each
    /// message is logged as withheld, unnumbered. Says whether the phase
    /// was still on.
    fn carry_out(
        &self,
        seq: u64,
        view: &View,
        sends: Vec<Sending>,
        seen: usize,
    ) -> anyhow::Result<bool> {
        let me = &self.context.id;
        let still_on = self.mind.in_phase(seq);
        for sending in sends {
            if still_on {
                let message = sending.message(seq, me, self.mind.number());
                let to = receivers(&message, me, view);
                self.send(message, to, seen)?;
            } else {
                self.context.log(Entry::Withheld {
                    from: me.clone(),
                    message: sending.message(seq, me, 0),
                    seen: Some(seen),
                });
            }
        }
        Ok(still_on)
    }

    /// Remember sending `message` and send it to everyone in `to`, logged
    /// as the player's action with `seen`, how many memories the model was
    /// shown when it made the call that produced it.
    fn send(&self, message: Message, to: HashSet<ActorId>, seen: usize) -> anyhow::Result<()> {
        self.remember(Memory::Acted {
            message: message.clone(),
        });
        act(&self.context, message, to, Some(seen))
    }

    /// Set a reminder to be woken `after` from now in the phase numbered
    /// `seq`, numbered so that any reminder set before it no longer counts.
    fn remind(&self, seq: u64, after: Duration) -> anyhow::Result<()> {
        let count = self.mind.remind();
        self.context
            .think_after(after, Message::Remind { seq, count })
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log a message from another actor, which is any numbered one, as
    /// observed in the latest phase announced. An announcement is of a new
    /// phase: it is remembered, and wakes the think loop if the player is
    /// awake in the phase. Anything else, another player's utterance or
    /// selection or the environment's tentative decision, counts only if
    /// it is tagged with that phase: then it is remembered and, by day,
    /// wakes the think loop, so a player announced a night it sleeps
    /// through is woken by nothing until the day. One tagged with another
    /// phase is observed and nothing more, neither remembered nor woken
    /// for: the player keeps to the phase it is in, as the environment does
    /// when it drops a late selection, and the observation's `seq` beside
    /// the message's shows it came late. A reminder to itself is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        // Whether the message counts, and if it does, whether it wakes the
        // think loop.
        let counts = match message {
            Message::Announce {
                seq, view, limit, ..
            } => {
                self.mind.announce(*seq, view, *limit);
                Some(awake(&self.context.id, view))
            }
            _ if message.seq().is_some_and(|seq| self.mind.in_phase(seq)) => {
                Some(self.mind.by_day())
            }
            _ => None,
        };
        observe(&self.context, message.sender(), n, self.mind.seq());
        if let Some(wakes) = counts {
            self.remember(Memory::Observed {
                message: message.clone(),
            });
            if wakes {
                self.wake()?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Think for Player {
    type Message = Message;

    /// Woken, decide what to do in the phase the player is in. Reminded,
    /// wake if the reminder still counts: it is the latest set and its
    /// phase has not ended. Nothing else is thought about. The think loop
    /// drops an error a thought returns, so the log is the one place a
    /// failure shows, whether the model's or the player's own.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        match message {
            Message::Wake => self.decide().await,
            Message::Remind { seq, count } if self.mind.reminder_counts(*seq, *count) => {
                self.wake()
            }
            _ => Ok(()),
        }
    }
}

/// Where the game stands for `me` as `view` shows it, how long the phase
/// lasts, and what is asked of it: by night, what its role does, and by
/// day, that it may speak, vote, or wait. A player awake by night is a
/// werewolf, the doctor or the seer, and is shown its own role.
fn situation(me: &PlayerId, view: &View, limit: Duration) -> anyhow::Result<String> {
    let (phase, asked) = match (&view.phase, view.roles.get(me)) {
        (Phase::Day, _) => ("day", DAY_ASKED),
        (Phase::Night, Some(Role::Werewolf)) => {
            ("night", "Choose whom the werewolves should kill.")
        }
        (Phase::Night, Some(Role::Doctor)) => ("night", "Choose whom to protect."),
        (Phase::Night, Some(Role::Seer)) => ("night", "Choose whom to ask about."),
        (Phase::Night, Some(Role::Villager)) => bail!("{me} is a villager, who sleeps by night"),
        (Phase::Night, None) => bail!("{me} is not shown its own role"),
    };
    Ok(format!(
        "It is {phase} {}, which lasts {}. {asked}",
        view.round,
        format_duration(limit)
    ))
}

/// A request of a player's shape to `model_id` under `tool_choice`:
/// `prompt` as the system message, `asked` and then [`ANSWER`] as the user
/// message, and `tools` to answer with. This is the one way a player
/// builds a request and the one way the probe does, so they agree on what
/// is asked.
fn request(
    model_id: &str,
    tool_choice: ToolChoice,
    prompt: &str,
    mut asked: String,
    tools: Vec<Tool>,
) -> Request {
    asked.push(' ');
    asked.push_str(ANSWER);
    Request::offering(
        model_id,
        vec![model::Message::system(prompt), model::Message::user(asked)],
        tools,
        tool_choice,
    )
}

/// What every request ends with, under either tool choice: that a tool
/// call is the one answer that counts.
const ANSWER: &str = "Answer by calling one of the tools.";

/// The tools a player is offered by night: [`select`] over `candidates`.
fn night_tools(candidates: &[PlayerId]) -> Vec<Tool> {
    vec![select(candidates)]
}

/// The tools a player is offered by day: [`speak`], [`vote`] over
/// `candidates`, and [`wait`] up to `limit`, how long the day lasts.
fn day_tools(candidates: &[PlayerId], limit: Duration) -> Vec<Tool> {
    vec![speak(), vote(candidates), wait(limit)]
}

/// What a player is asked to do by day.
const DAY_ASKED: &str = "You may speak, vote, or wait.";

/// The two placeholder candidates the probe asks about.
fn placeholders() -> [PlayerId; 2] {
    ["player1".to_string(), "player2".to_string()]
}

/// How long the probe's day lasts.
const PROBE_DAY: Duration = Duration::from_secs(60);

/// What [`check_plays`] found.
#[derive(Debug)]
pub enum Probed {
    /// Asked as by night and as by day, the model answered both requests
    /// as a player must, under the tool choice it is to be asked under.
    Played(ToolChoice),
    /// One of the requests the model did not answer as a player must: the
    /// provider refused it as bad, or the answer was not well formed. The
    /// error names the model and the request, and carries the cause
    /// beneath it.
    DidNotPlay(anyhow::Error),
}

/// Check that `model`, asked as a player is, plays, and find out whether
/// it may be made to call a tool: two requests of a player's shape, one
/// after the other, each with a short system prompt and over two
/// placeholder candidates. The night's asks for a selection with the
/// `select` tool, and is judged as a player's night answer is by
/// `selected`; the day's says it is day and the player may speak, vote,
/// or wait, offers the day's three tools over a day of one minute, and is
/// judged as a player's day answer is by `asked`.
///
/// The night's request requires a tool call. If the provider refuses that
/// as bad, it is asked again allowing one, and both refusals are given if
/// that is refused too. The first tool choice it is answered under is the
/// game's, and the day's request is asked under it. The day's request is
/// not made when the night's fails. A model that could not be asked at
/// all, because the provider was not reached or answered with anything
/// but the model's answer, is an error naming the model, with the cause
/// beneath it.
pub async fn check_plays(model: &dyn model::Model, model_id: &str) -> anyhow::Result<Probed> {
    let candidates = placeholders();
    let by_night = async |tool_choice| {
        let night = request(
            model_id,
            tool_choice,
            PROBE_PROMPT,
            "Select one of the candidates.".to_string(),
            night_tools(&candidates),
        );
        let response = model.complete(&night).await?;
        selected(&response, &candidates).map(drop)
    };
    let by_day = async |tool_choice| {
        let situation = format!(
            "It is day, which lasts {}. {DAY_ASKED}",
            format_duration(PROBE_DAY)
        );
        let day = request(
            model_id,
            tool_choice,
            PROBE_PROMPT,
            situation,
            day_tools(&candidates, PROBE_DAY),
        );
        let response = model.complete(&day).await?;
        asked(&response, &candidates, PROBE_DAY).map(drop)
    };
    let (tool_choice, night) = match by_night(ToolChoice::Required).await {
        Err(required) if model::refused(&required) => {
            let night = by_night(ToolChoice::Auto).await.map_err(|auto| {
                if model::concerns_the_model(&auto) {
                    anyhow!("made to call a tool: {required:#}\nasked to call one: {auto:#}")
                } else {
                    auto
                }
            });
            (ToolChoice::Auto, night)
        }
        night => (ToolChoice::Required, night),
    };
    let failed = match night {
        Err(error) => Some(("night", error)),
        Ok(()) => by_day(tool_choice).await.err().map(|error| ("day", error)),
    };
    match failed {
        None => Ok(Probed::Played(tool_choice)),
        Some((phase, error)) if model::concerns_the_model(&error) => Ok(Probed::DidNotPlay(
            error.context(format!("{model_id} did not answer a {phase}'s request")),
        )),
        Some((_, error)) => Err(error.context(format!("{model_id} could not be asked to play"))),
    }
}

/// The system prompt the probe tells the model.
const PROBE_PROMPT: &str = "You are a player in a game, about to be asked to play.";

/// The name of the tool a player selects with, by night and by day.
const SELECT: &str = "select";

/// The name of the tool a player speaks with, by day.
const SPEAK: &str = "speak";

/// The name of the tool a player waits with, by day.
const WAIT: &str = "wait";

/// The tool a player selects with by night: `select`, taking a `target`
/// that is one of `candidates`.
fn select(candidates: &[PlayerId]) -> Tool {
    selecting(
        "Knowing everything you know, select the best one.",
        candidates,
    )
}

/// The tool a player votes with by day: `select`, taking a `target` that
/// is one of `candidates`, described as a vote that may be changed.
fn vote(candidates: &[PlayerId]) -> Tool {
    selecting(
        "Vote for the player you want voted out. Your latest vote is the one that counts.",
        candidates,
    )
}

/// The `select` tool, described to the model by `description`, taking a
/// `target` that is one of `candidates`.
fn selecting(description: &str, candidates: &[PlayerId]) -> Tool {
    Tool::new(
        SELECT,
        description,
        json!({
            "type": "object",
            "properties": {"target": {"type": "string", "enum": candidates}},
            "required": ["target"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player speaks with by day: `speak`, taking `text`, which is
/// not empty, nor blank once trimmed.
fn speak() -> Tool {
    Tool::new(
        SPEAK,
        "Say something to every other living player.",
        json!({
            "type": "object",
            "properties": {"text": {"type": "string", "minLength": 1}},
            "required": ["text"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player waits with by day: `wait`, taking `seconds`, a whole
/// number from 1 to the day's `limit` in whole seconds.
fn wait(limit: Duration) -> Tool {
    Tool::new(
        WAIT,
        "Say nothing now, and be woken after this many seconds unless something wakes you sooner.",
        json!({
            "type": "object",
            "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": limit.as_secs()}},
            "required": ["seconds"],
            "additionalProperties": false,
        }),
    )
}

/// The arguments of a [`select`] call.
#[derive(Deserialize)]
struct Selection {
    target: PlayerId,
}

/// The arguments of a [`speak`] call.
#[derive(Deserialize)]
struct Speech {
    text: String,
}

/// The arguments of a [`wait`] call.
#[derive(Deserialize)]
struct Waiting {
    seconds: u64,
}

/// What a day's response asks for: the messages to send, in the order the
/// model made the calls, and how long to wait, if it asked to. Of several
/// waits, the last counts.
#[derive(Debug, Default, PartialEq)]
struct Asked {
    /// What to send, in order.
    sends: Vec<Sending>,
    /// How long to wait, if asked.
    wait: Option<Duration>,
}

impl Asked {
    /// Take in `call`, one of a day's, if it is well formed, as [`asked`]
    /// judges it.
    fn take(
        &mut self,
        call: &model::Call,
        candidates: &[PlayerId],
        limit: Duration,
    ) -> anyhow::Result<()> {
        match call.name.as_str() {
            SPEAK => {
                let Speech { text } = arguments(call)?;
                if text.trim().is_empty() {
                    bail!("the model's {SPEAK} call says nothing");
                }
                self.sends.push(Sending::Say(text));
            }
            SELECT => self.sends.push(Sending::Select(target(call, candidates)?)),
            WAIT => {
                let Waiting { seconds } = arguments(call)?;
                if !(1..=limit.as_secs()).contains(&seconds) {
                    bail!(
                        "the model asked to wait {seconds}s, which is not between 1s and {}",
                        format_duration(limit)
                    );
                }
                self.wait = Some(Duration::from_secs(seconds));
            }
            other => bail!("the model called {other}, which is not a tool it was offered"),
        }
        Ok(())
    }
}

/// What a call has the player send: by day whatever it asked for, by
/// night its one selection.
#[derive(Debug, PartialEq)]
enum Sending {
    /// Say `text` to every other living player.
    Say(String),
    /// Vote for `target`.
    Select(PlayerId),
}

impl Sending {
    /// The message `from` sends for this in the phase numbered `seq`,
    /// numbered `n`.
    fn message(self, seq: u64, from: &PlayerId, n: u64) -> Message {
        let from = from.clone();
        match self {
            Sending::Say(text) => Message::Say { seq, from, n, text },
            Sending::Select(target) => Message::Select {
                seq,
                from,
                n,
                target,
            },
        }
    }
}

/// Whom `response` selects from `candidates`: the `target` of its call to
/// [`select`]. No tool call, a call to anything else, arguments that do not
/// parse, or a target who is not a candidate is an error.
fn selected(response: &Response, candidates: &[PlayerId]) -> anyhow::Result<PlayerId> {
    let call = &response
        .tool_calls()
        .first()
        .ok_or_else(|| anyhow!("the model made no tool call"))?
        .function;
    if call.name != SELECT {
        bail!("the model called {} instead of {SELECT}", call.name);
    }
    target(call, candidates)
}

/// What a day's `response` asks for, its calls taken in the order the
/// model made them. The response is judged as a whole: no tool call, a
/// call to a tool other than [`speak`], [`select`] and [`wait`], arguments
/// that do not parse, a target who is not one of `candidates`, text that is
/// empty once trimmed, or seconds not between 1 and `limit` in whole
/// seconds is an error, and
/// then nothing in it is carried out.
fn asked(response: &Response, candidates: &[PlayerId], limit: Duration) -> anyhow::Result<Asked> {
    let made = response.tool_calls();
    if made.is_empty() {
        bail!("the model made no tool call");
    }
    let mut asked = Asked::default();
    for call in made {
        asked.take(&call.function, candidates, limit)?;
    }
    Ok(asked)
}

/// The arguments of `call`, parsed. Arguments that do not parse are an
/// error naming the tool and showing them.
fn arguments<T: DeserializeOwned>(call: &model::Call) -> anyhow::Result<T> {
    serde_json::from_str(&call.arguments).with_context(|| {
        format!(
            "the arguments of the model's {} call: {}",
            call.name, call.arguments
        )
    })
}

/// Whom `call`, a call to [`select`], selects from `candidates`. Arguments
/// that do not parse, or a target who is not a candidate, is an error.
fn target(call: &model::Call, candidates: &[PlayerId]) -> anyhow::Result<PlayerId> {
    let Selection { target } = arguments(call)?;
    if !candidates.contains(&target) {
        bail!("the model selected {target}, who is not a candidate");
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned;
    use crate::model::fake::{FakeModel, SAID, choosing_first, voted_in_this_phase};
    use crate::werewolf::tests::{
        Played, announcement, between_the_deal_and_the_end, logged, observed_by,
        observed_by_the_environment, one_wolf_against, received, run, seen, seen_by_day, selection,
        selections, selections_sent, tentative, utterance,
    };
    use crate::werewolf::uniform_random::tests::{Puppet, Script, Seat, at_once};
    use crate::werewolf::{
        ENVIRONMENT, thinking_episode, to_everyone_but, to_the_environment, to_the_players_but,
    };
    use clap::Parser;
    use secrecy::ExposeSecret;
    use std::num::NonZero;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc::unbounded_channel;

    /// A file naming only the model, which is all that is required.
    const MODEL: &str = r#"
[model]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
"#;

    /// The settings `file` and `overrides` produce under the code defaults.
    fn settle(file: &str, overrides: Overrides) -> Settings {
        Config::parse(file).unwrap().settle(overrides).unwrap()
    }

    /// The command line of a variant that takes the overrides.
    #[derive(Parser)]
    struct Variant {
        #[command(flatten)]
        overrides: Overrides,
    }

    #[test]
    fn the_examples_parse_and_render() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/werewolf/llm");
        let files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        assert!(!files.is_empty(), "no examples in {dir}");
        for file in files {
            let name = file.display().to_string();
            let config = Config::load(&file).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let settings = config
                .settle(Overrides::default())
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let prompts = settings
                .prompts()
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert!(!prompts.is_empty(), "{name}");
            // Each example sets the ratify limit, and tells the players
            // that a vote stands once nobody has changed theirs for that
            // long, and that by day they may speak, vote, or wait.
            let ratify = settings.config.phases.ratify_limit.unwrap();
            assert_eq!(ratify, settings.ratify_limit, "{name}");
            let rules = &settings.config.text["rules"];
            assert!(
                rules.contains(&format_duration(ratify).to_string()),
                "{name}: {rules}"
            );
            assert!(rules.contains("speak"), "{name}: {rules}");
            assert!(rules.contains("vote, or wait"), "{name}: {rules}");
        }
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
    fn the_model_section_is_required() {
        assert!(Config::parse("[phases]\nnight_limit = \"1s\"\n").is_err());
    }

    #[test]
    fn the_model_and_its_provider_fall_back_from_the_command_line_to_the_file() {
        let settings = settle(MODEL, Overrides::default());
        assert_eq!(settings.config.model.base_url, "http://localhost:1234/v1");
        assert_eq!(settings.config.model.id, "qwen2.5-7b-instruct");
        let args = [
            "llm",
            "--model-base-url",
            "https://api.openai.com/v1",
            "--model-id",
            "gpt-5.5",
        ];
        let settings = settle(MODEL, Variant::parse_from(args).overrides);
        assert_eq!(settings.config.model.base_url, "https://api.openai.com/v1");
        assert_eq!(settings.config.model.id, "gpt-5.5");
        // With everything that follows from the provider.
        assert_eq!(settings.config.model.key_variable(), Some("OPENAI_API_KEY"));
    }

    #[test]
    fn the_role_counts_fall_back_from_the_command_line_to_the_file_to_the_code() {
        let file = format!(
            "{MODEL}
[roles.werewolf]
count = 5
[roles.villager]
count = 6
[roles.doctor]
count = 7
[roles.seer]
count = 8
"
        );
        // The code alone.
        assert_eq!(settle(MODEL, Overrides::default()).table, Table::default());
        // The file over the code.
        assert_eq!(
            settle(&file, Overrides::default()).table,
            Table {
                werewolves: 5,
                villagers: 6,
                doctors: 7,
                seers: 8
            }
        );
        // The command line over the file.
        let given = |roles| {
            settle(
                &file,
                Overrides {
                    roles,
                    ..Overrides::default()
                },
            )
            .table
        };
        let counts = RoleCounts {
            werewolves: Some(1),
            villagers: Some(2),
            doctors: Some(3),
            seers: Some(4),
        };
        assert_eq!(
            given(counts),
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
            given(counts),
            Table {
                werewolves: 1,
                villagers: 6,
                doctors: 7,
                seers: 4
            }
        );
    }

    #[test]
    fn a_role_count_in_the_file_may_be_left_out() {
        let file = format!("{MODEL}\n[roles.seer]\nsystem = \"You see.\"\n");
        let settings = settle(&file, Overrides::default());
        assert_eq!(settings.table, Table::default());
        assert_eq!(
            settings.config.roles.seer.system.as_deref(),
            Some("You see.")
        );
    }

    #[test]
    fn the_phase_limits_fall_back_from_the_command_line_to_the_file_to_the_code() {
        let file = format!("{MODEL}\n[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"2m\"\n");
        // The code alone: the same limits every variant plays under.
        let settings = settle(MODEL, Overrides::default());
        assert_eq!(settings.night_limit, Rules::default().night_limit);
        assert_eq!(settings.day_limit, Rules::default().day_limit);
        // The file over the code.
        let settings = settle(&file, Overrides::default());
        assert_eq!(settings.night_limit, Duration::from_secs(90));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
        // The command line over the file.
        let settings = settle(
            &file,
            Overrides {
                limits: PhaseLimits {
                    night_limit: Some(Duration::from_secs(5)),
                    day_limit: Some(Duration::from_secs(70)),
                },
                ..Overrides::default()
            },
        );
        assert_eq!(settings.night_limit, Duration::from_secs(5));
        assert_eq!(settings.day_limit, Duration::from_secs(70));
        // A limit the command line does not give still comes from the file.
        let settings = settle(
            &file,
            Overrides {
                limits: PhaseLimits {
                    night_limit: Some(Duration::from_secs(5)),
                    day_limit: None,
                },
                ..Overrides::default()
            },
        );
        assert_eq!(settings.night_limit, Duration::from_secs(5));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
    }

    #[test]
    fn the_ratify_limit_falls_back_from_the_command_line_to_the_file_to_the_code() {
        let file = format!("{MODEL}\n[phases]\nratify_limit = \"15s\"\n");
        // The code alone.
        assert_eq!(
            settle(MODEL, Overrides::default()).ratify_limit,
            RATIFY_LIMIT
        );
        assert_eq!(RATIFY_LIMIT, Duration::from_secs(20));
        // The file over the code.
        assert_eq!(
            settle(&file, Overrides::default()).ratify_limit,
            Duration::from_secs(15)
        );
        // The command line over the file.
        let overrides = Overrides {
            ratify_limit: Some(Duration::from_secs(5)),
            ..Overrides::default()
        };
        assert_eq!(
            settle(&file, overrides).ratify_limit,
            Duration::from_secs(5)
        );
    }

    #[test]
    fn a_ratify_limit_longer_than_the_day_limit_is_an_error_naming_both() {
        let file = format!("{MODEL}\n[phases]\nday_limit = \"20s\"\nratify_limit = \"30s\"\n");
        let error = Config::parse(&file)
            .unwrap()
            .settle(Overrides::default())
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "the ratify limit (30s) is longer than the day limit (20s)"
        );
        // However the two are given.
        let overrides = Overrides {
            limits: PhaseLimits {
                night_limit: None,
                day_limit: Some(Duration::from_secs(10)),
            },
            ..Overrides::default()
        };
        let error = Config::parse(MODEL).unwrap().settle(overrides).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "the ratify limit (20s) is longer than the day limit (10s)"
        );
        // One as long as the day is allowed.
        let overrides = Overrides {
            limits: PhaseLimits {
                night_limit: None,
                day_limit: Some(Duration::from_secs(20)),
            },
            ..Overrides::default()
        };
        assert_eq!(
            settle(MODEL, overrides).ratify_limit,
            Duration::from_secs(20)
        );
    }

    #[test]
    fn the_rules_are_the_variant_s_with_the_limits_that_took_effect() {
        let file = format!(
            "{MODEL}\n[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"7s\"\nratify_limit = \"3s\"\n"
        );
        let settings = settle(&file, Overrides::default());
        let settled = settings.rules();
        assert_eq!(settled.variation, "LLM");
        assert_eq!(settled.night_limit, settings.night_limit);
        assert_eq!(settled.day_limit, settings.day_limit);
        assert_eq!(settled.ratify_limit, Some(Duration::from_secs(3)));
        // The variant's own rules ratify the vote after the code's default.
        assert_eq!(rules().ratify_limit, Some(RATIFY_LIMIT));
    }

    #[test]
    fn the_command_line_takes_durations_in_humantime_form() {
        let variant = Variant::parse_from([
            "llm",
            "--night-limit",
            "30s",
            "--day-limit",
            "1m 30s",
            "--ratify-limit",
            "15s",
        ]);
        assert_eq!(
            variant.overrides.limits,
            PhaseLimits {
                night_limit: Some(Duration::from_secs(30)),
                day_limit: Some(Duration::from_secs(90)),
            }
        );
        assert_eq!(
            variant.overrides.ratify_limit,
            Some(Duration::from_secs(15))
        );
        assert!(Variant::try_parse_from(["llm", "--night-limit", "soon"]).is_err());
        assert!(Variant::try_parse_from(["llm", "--day-limit", "30"]).is_err());
        assert!(Variant::try_parse_from(["llm", "--ratify-limit", "soon"]).is_err());
    }

    #[test]
    fn the_request_timeout_comes_from_the_file_or_the_code() {
        assert_eq!(
            settle(MODEL, Overrides::default())
                .config
                .model
                .request_timeout,
            REQUEST_TIMEOUT
        );
        let file = format!("{MODEL}request_timeout = \"10s\"\n");
        assert_eq!(
            settle(&file, Overrides::default())
                .config
                .model
                .request_timeout,
            Duration::from_secs(10)
        );
    }

    #[test]
    fn the_context_window_falls_back_from_the_command_line_to_the_file_to_the_provider_to_the_code()
    {
        let window = |tokens, source| ContextWindow { tokens, source };
        let file = format!("{MODEL}context_window = 8192\n");
        let command_line = || Variant::parse_from(["llm", "--context-window", "16384"]).overrides;
        let settings = settle(&file, command_line());
        assert_eq!(
            settings.context_window(Some(32768)),
            window(16384, Source::CommandLine)
        );
        let settings = settle(MODEL, command_line());
        assert_eq!(
            settings.context_window(None),
            window(16384, Source::CommandLine)
        );
        let settings = settle(&file, Overrides::default());
        assert_eq!(
            settings.context_window(Some(32768)),
            window(8192, Source::File)
        );
        let settings = settle(MODEL, Overrides::default());
        assert_eq!(
            settings.context_window(Some(32768)),
            window(32768, Source::Provider)
        );
        assert_eq!(
            settings.context_window(None),
            window(CONTEXT_WINDOW, Source::Default)
        );
        assert_eq!(CONTEXT_WINDOW, 4096);
    }

    #[test]
    fn a_context_window_of_0_is_an_error_naming_the_setting() {
        let error = |file: &str, overrides| {
            let error = Config::parse(file).unwrap().settle(overrides).unwrap_err();
            format!("{error:#}")
        };
        let file = format!("{MODEL}context_window = 0\n");
        assert_eq!(
            error(&file, Overrides::default()),
            "context_window is 0; a context window holds at least 1 token"
        );
        let overrides = Variant::parse_from(["llm", "--context-window", "0"]).overrides;
        assert_eq!(
            error(MODEL, overrides),
            "--context-window is 0; a context window holds at least 1 token"
        );
    }

    #[test]
    fn a_duration_the_file_cannot_parse_is_an_error() {
        let file = format!("{MODEL}request_timeout = \"soon\"\n");
        assert!(Config::parse(&file).is_err());
        let file = format!("{MODEL}request_timeout = 10\n");
        assert!(Config::parse(&file).is_err());
        let file = format!("{MODEL}[phases]\nnight_limit = \"a while\"\n");
        assert!(Config::parse(&file).is_err());
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        for file in [
            format!("{MODEL}temperature = 0.5\n"),
            format!("{MODEL}[phases]\nnight = \"1m\"\n"),
            format!("{MODEL}[roles.seer]\nprompt = \"\"\n"),
            format!("{MODEL}[prompt]\nuser = \"\"\n"),
            format!("{MODEL}[tuning]\n"),
        ] {
            assert!(Config::parse(&file).is_err(), "{file}");
        }
    }

    #[test]
    fn an_unknown_role_is_an_error() {
        let file = format!("{MODEL}[roles.hunter]\ncount = 1\n");
        let error = Config::parse(&file).unwrap_err();
        assert!(format!("{error:#}").contains("hunter"), "{error:#}");
    }

    #[test]
    fn the_templates_are_kept_as_written() {
        let file = format!(
            "{MODEL}
[text]
rules = \"The rules.\"
[prompt]
system = \"{{{{ rules }}}} You are {{{{ name }}}}.\"
[roles.werewolf]
system = \"Howl.\"
[personas]
player1 = \"Cautious.\"
"
        );
        let config = settle(&file, Overrides::default()).config;
        assert_eq!(config.text["rules"], "The rules.");
        assert_eq!(
            config.prompt.system.as_deref(),
            Some("{{ rules }} You are {{ name }}.")
        );
        assert_eq!(config.roles.werewolf.system.as_deref(), Some("Howl."));
        assert_eq!(config.roles.villager.system, None);
        assert_eq!(config.personas["player1"], "Cautious.");
    }

    /// A file whose one template shows every variable but the text blocks.
    const TEMPLATE: &str = r#"
[prompt]
system = "{{ name }} the {{ role }}[{{ persona }}]"
"#;

    /// The prompt `prompts` holds for `seat` dealt `role`.
    fn prompt<'a>(prompts: &'a [SystemPrompt], seat: &str, role: Role) -> &'a str {
        &prompts
            .iter()
            .find(|prompt| prompt.seat == seat && prompt.role == role)
            .unwrap_or_else(|| panic!("no prompt for {seat} as {role}"))
            .text
    }

    #[test]
    fn a_role_s_own_template_is_used_over_the_default() {
        let file = format!(
            "{MODEL}
[prompt]
system = \"Default for the {{{{ role }}}}.\"
[roles.seer]
system = \"The seer's own.\"
"
        );
        let prompts = settle(&file, Overrides::default()).prompts().unwrap();
        assert_eq!(
            prompt(&prompts, "player1", Role::Werewolf),
            "Default for the werewolf."
        );
        assert_eq!(prompt(&prompts, "player1", Role::Seer), "The seer's own.");
    }

    #[test]
    fn a_role_at_the_table_with_no_template_is_an_error_naming_it() {
        let file = format!("{MODEL}[roles.seer]\nsystem = \"The seer's own.\"\n");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        assert!(format!("{error:#}").contains("werewolf"), "{error:#}");
        // A role that is not at the table needs no template.
        let overrides = Overrides {
            roles: RoleCounts {
                werewolves: Some(0),
                villagers: Some(0),
                doctors: Some(0),
                seers: Some(2),
            },
            ..Overrides::default()
        };
        let prompts = settle(&file, overrides).prompts().unwrap();
        assert_eq!(prompts.len(), 2);
    }

    #[test]
    fn the_seat_the_role_and_the_persona_are_variables() {
        let file = format!("{MODEL}{TEMPLATE}[personas]\nplayer2 = \"Cautious.\"\n");
        let prompts = settle(&file, Overrides::default()).prompts().unwrap();
        assert_eq!(
            prompt(&prompts, "player2", Role::Doctor),
            "player2 the doctor[Cautious.]"
        );
        assert_eq!(
            prompt(&prompts, "player3", Role::Seer),
            "player3 the seer[]"
        );
    }

    #[test]
    fn every_text_block_is_a_variable() {
        let file = format!(
            "{MODEL}
[text]
rules = \"The rules.\"
wolf = \"Howl.\"
[prompt]
system = \"{{{{ rules }}}} {{{{ wolf }}}}\"
"
        );
        let prompts = settle(&file, Overrides::default()).prompts().unwrap();
        assert_eq!(
            prompt(&prompts, "player1", Role::Villager),
            "The rules. Howl."
        );
    }

    #[test]
    fn an_undefined_variable_is_an_error_naming_the_seat_and_the_role() {
        let file = format!("{MODEL}[prompt]\nsystem = \"{{{{ rulez }}}}\"\n");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        let shown = format!("{error:#}");
        for expected in ["player1", "werewolf", "rulez"] {
            assert!(shown.contains(expected), "{shown}");
        }
    }

    #[test]
    fn a_template_that_does_not_compile_is_an_error_naming_the_role() {
        let file = format!("{MODEL}[roles.seer]\nsystem = \"{{% if %}}\"\n{TEMPLATE}");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("seer"), "{shown}");
        assert!(shown.contains("syntax"), "{shown}");
    }

    #[test]
    fn a_text_block_named_like_a_variable_is_an_error_naming_it() {
        assert_eq!(variables(), ["name", "persona", "role"]);
        for variable in variables() {
            let file = format!("{MODEL}{TEMPLATE}[text]\n{variable} = \"Taken.\"\n");
            let error = Config::parse(&file).unwrap_err();
            assert!(format!("{error:#}").contains(variable), "{error:#}");
        }
    }

    #[test]
    fn a_persona_for_a_seat_that_does_not_exist_is_an_error_naming_it() {
        let file = format!("{MODEL}{TEMPLATE}[personas]\nplayer8 = \"Late.\"\n");
        let error = settle(&file, Overrides::default()).prompts().unwrap_err();
        assert!(format!("{error:#}").contains("player8"), "{error:#}");
        // The seats follow the settled counts, not the file's alone.
        let overrides = Overrides {
            roles: RoleCounts {
                villagers: Some(4),
                ..RoleCounts::default()
            },
            ..Overrides::default()
        };
        settle(&file, overrides).prompts().unwrap();
    }

    #[test]
    fn every_seat_is_rendered_for_every_role_at_the_table_in_order() {
        let file = format!("{MODEL}{TEMPLATE}");
        let prompts = settle(&file, Overrides::default()).prompts().unwrap();
        let rendered: Vec<_> = prompts
            .iter()
            .map(|prompt| (prompt.seat.clone(), prompt.role))
            .collect();
        let expected: Vec<_> = (1..=7)
            .flat_map(|seat| {
                [Role::Werewolf, Role::Villager, Role::Doctor, Role::Seer]
                    .map(|role| (format!("player{seat}"), role))
            })
            .collect();
        assert_eq!(rendered, expected);
    }

    #[test]
    fn an_omitted_key_variable_means_no_key() {
        let settings = settle(MODEL, Overrides::default());
        assert!(settings.config.model.api_key().unwrap().is_none());
    }

    #[test]
    fn a_named_key_variable_that_is_not_set_is_an_error_naming_it() {
        let file = format!("{MODEL}api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"\n");
        let error = settle(&file, Overrides::default())
            .config
            .model
            .api_key()
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("SOCIAL_DEDUCTION_NO_SUCH_KEY"),
            "{error:#}"
        );
    }

    #[test]
    fn the_key_is_read_from_the_named_variable_and_never_shown() {
        // A variable Cargo sets for every test, since setting one is unsafe.
        let name = "CARGO_MANIFEST_DIR";
        let value = std::env::var(name).unwrap();
        let file = format!("{MODEL}api_key_env = \"{name}\"\n");
        let settings = settle(&file, Overrides::default());
        let key = settings.config.model.api_key().unwrap().unwrap();
        assert_eq!(key.expose_secret(), value);
        let shown = format!("{key:?}\n{settings:?}");
        assert!(shown.contains(name), "{shown}");
        assert!(!shown.contains(&value), "{shown}");
    }

    #[test]
    fn a_known_provider_s_key_variable_is_filled_in_unless_the_file_names_one() {
        let openai = MODEL.replace("http://localhost:1234/v1", "https://api.openai.com/v1");
        let settings = settle(&openai, Overrides::default());
        assert_eq!(settings.config.model.key_variable(), Some("OPENAI_API_KEY"));
        let named = format!("{openai}api_key_env = \"MY_KEY\"\n");
        let settings = settle(&named, Overrides::default());
        assert_eq!(settings.config.model.key_variable(), Some("MY_KEY"));
    }

    /// A file saying something under every table, for a game of one
    /// werewolf, one seer and one villager.
    fn configured() -> String {
        format!(
            "{MODEL}
request_timeout = \"10s\"
context_window = 8192
[phases]
night_limit = \"30s\"
day_limit = \"1m 30s\"
ratify_limit = \"10s\"
[roles.werewolf]
count = 1
[roles.villager]
count = 1
[roles.doctor]
count = 0
[roles.seer]
count = 1
system = \"{{{{ name }}}} sees.\"
[text]
rules = \"The rules.\"
[prompt]
system = \"{{{{ rules }}}} {{{{ name }}}} the {{{{ role }}}}[{{{{ persona }}}}]\"
[personas]
player2 = \"Cautious.\"
"
        )
    }

    /// A deal at the table [`configured`] sets.
    fn dealt() -> HashMap<PlayerId, Role> {
        HashMap::from([
            ("player1".to_string(), Role::Villager),
            ("player2".to_string(), Role::Werewolf),
            ("player3".to_string(), Role::Seer),
        ])
    }

    /// The configuration of a game of `roles` under `settings`, with every
    /// prompt rendered.
    fn configuration_of(
        settings: &Settings,
        roles: &HashMap<PlayerId, Role>,
    ) -> anyhow::Result<Configuration> {
        settings.configuration(
            roles,
            &settings.prompts()?,
            ToolChoice::Required,
            settings.context_window(None),
        )
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
                request_timeout: Duration::from_secs(10),
                base_url: "http://localhost:1234/v1".to_string(),
                model: MODEL_ID.to_string(),
                tool_choice: ToolChoice::Required,
                context_window: ContextWindow {
                    tokens: 8192,
                    source: Source::File,
                },
                api_key_env: None,
                personas: BTreeMap::from([("player2".to_string(), "Cautious.".to_string())]),
                prompts: BTreeMap::from([
                    (
                        "player1".to_string(),
                        "The rules. player1 the villager[]".to_string(),
                    ),
                    (
                        "player2".to_string(),
                        "The rules. player2 the werewolf[Cautious.]".to_string(),
                    ),
                    ("player3".to_string(), "player3 sees.".to_string()),
                ]),
                default_template: Some(
                    "{{ rules }} {{ name }} the {{ role }}[{{ persona }}]".to_string()
                ),
                role_templates: BTreeMap::from([(Role::Seer, "{{ name }} sees.".to_string())]),
                text: BTreeMap::from([("rules".to_string(), "The rules.".to_string())]),
            }
        );
    }

    #[test]
    fn a_player_with_no_prompt_rendered_for_its_role_is_an_error_naming_the_seat_and_the_role() {
        let mut roles = dealt();
        roles.insert("player3".to_string(), Role::Doctor);
        let error =
            configuration_of(&settle(&configured(), Overrides::default()), &roles).unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("player3"), "{shown}");
        assert!(shown.contains("doctor"), "{shown}");
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
        assert_eq!(configuration["request_timeout"], json!("10s"));
        assert_eq!(
            configuration["context_window"],
            json!({"tokens": 8192, "source": "file"})
        );
        // A role is a key by its name.
        assert_eq!(configuration["role_counts"]["Werewolf"], json!(1));
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn the_configuration_names_the_key_s_variable_and_never_holds_the_key() {
        // A variable Cargo sets for every test, since setting one is unsafe.
        let name = "CARGO_MANIFEST_DIR";
        let value = std::env::var(name).unwrap();
        let file = format!("{MODEL}api_key_env = \"{name}\"\n{TEMPLATE}");
        let configuration =
            configuration_of(&settle(&file, Overrides::default()), &dealt()).unwrap();
        assert_eq!(configuration.api_key_env.as_deref(), Some(name));
        let shown = format!(
            "{configuration:?}\n{}",
            serde_json::to_string(&configuration).unwrap()
        );
        assert!(!shown.contains(&value), "{shown}");
        // The variable a known provider's key is read from is named too.
        let openai = MODEL.replace("http://localhost:1234/v1", "https://api.openai.com/v1");
        let file = format!("{openai}{TEMPLATE}");
        let configuration =
            configuration_of(&settle(&file, Overrides::default()), &dealt()).unwrap();
        assert_eq!(configuration.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    }

    /// The id of the model every test's players ask for.
    const MODEL_ID: &str = "qwen2.5-7b-instruct";

    /// A model answering every request with `answer` of it, held so that
    /// the test can read what it was asked while the players ask it.
    fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Arc<FakeModel> {
        Arc::new(FakeModel::answering(answer))
    }

    /// The prompt the player `name` is told, which names it, so that a
    /// request says whose it is.
    fn told(name: &str) -> String {
        format!("You are {name}.")
    }

    /// What the player `name` asked `model`, in order: the user message of
    /// each request carrying its prompt.
    fn asked_of(model: &FakeModel, name: &str) -> Vec<String> {
        let prompt = model::Message::system(told(name));
        model
            .requests()
            .iter()
            .filter(|request| request.messages[0] == prompt)
            .map(|request| request.messages[1].content.clone())
            .collect()
    }

    /// The story a request's user message tells, before the situation.
    fn story(asked: &str) -> &str {
        asked.rsplit_once("\n\n").unwrap().0
    }

    /// The situation a request's user message ends with, before what
    /// every request ends with.
    fn asked(asked: &str) -> &str {
        let ending = asked.rsplit_once("\n\n").unwrap().1;
        ending.strip_suffix(&format!(" {ANSWER}")).unwrap()
    }

    /// The configuration of a game of `roles` under [`MODEL`], whose every
    /// player is told its prompt.
    fn configuration(roles: &HashMap<PlayerId, Role>) -> Configuration {
        let prompts: Vec<SystemPrompt> = roles
            .iter()
            .map(|(id, role)| SystemPrompt {
                seat: id.clone(),
                role: *role,
                text: told(id),
            })
            .collect();
        let settings = settle(MODEL, Overrides::default());
        settings
            .configuration(
                roles,
                &prompts,
                ToolChoice::Required,
                settings.context_window(None),
            )
            .unwrap()
    }

    /// Run a game of `roles` under `rules`, every player a model player
    /// told its prompt and asking `model`.
    async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        model: Arc<FakeModel>,
    ) -> anyhow::Result<Played> {
        let configuration = configuration(&roles);
        play_configured(roles, rules, configuration, model).await
    }

    /// Run a game of `roles` under `rules` and `configuration`, every
    /// player a model player asking `model`.
    async fn play_configured(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        configuration: Configuration,
        model: Arc<FakeModel>,
    ) -> anyhow::Result<Played> {
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let episode = game(roles, rules, configuration, model, winner, logger);
        run(episode, won, log).await
    }

    /// Run a game of `roles` under `rules`, every player a model player
    /// told its prompt and asking `model`, except those in `puppets`, who
    /// follow their own scripts.
    async fn play_with(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        model: Arc<FakeModel>,
        mut puppets: HashMap<&str, Seat<Actor>>,
    ) -> anyhow::Result<Played> {
        let configuration = Entry::Configuration(Box::new(configuration(&roles)));
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let players: Vec<PlayerId> = roles.keys().cloned().collect();
        let environment = announced::init(
            roles,
            rules,
            Some(configuration),
            winner,
            Actor::Environment,
        );
        let model: Arc<dyn model::Model> = model;
        let episode = thinking_episode(
            environment,
            players,
            |id| {
                puppets.remove(id.as_str()).unwrap_or_else(|| {
                    Player::init(
                        id.clone(),
                        told(id),
                        MODEL_ID.to_string(),
                        ToolChoice::Required,
                        Arc::clone(&model),
                    )
                })
            },
            logger,
        );
        run(episode, won, log).await
    }

    /// A puppet following `script`.
    fn puppet(script: Script) -> Seat<Actor> {
        Puppet::init(script, Actor::Puppet)
    }

    /// A player who never says anything.
    fn mute() -> Seat<Actor> {
        puppet(Box::new(|_, _, _| vec![]))
    }

    /// A werewolf who kills its first candidate `after` each night begins,
    /// and says nothing by day.
    fn hunting_after(after: Duration) -> Seat<Actor> {
        hunting_after_each(move |_| after)
    }

    /// A werewolf who kills its first candidate `after` the night numbered
    /// `seq` begins, and says nothing by day.
    fn hunting_after_each(after: impl Fn(u64) -> Duration + Send + 'static) -> Seat<Actor> {
        puppet(Box::new(move |me, seq, view| match view.phase {
            Phase::Night => vec![(
                after(seq),
                selection(seq, me, seq, &candidates(me, view)[0]),
            )],
            Phase::Day => vec![],
        }))
    }

    /// A werewolf who kills its first candidate at once the first night and
    /// three seconds into every night after.
    fn hunting_at_once_then_after_three_seconds() -> Seat<Actor> {
        hunting_after_each(|seq| Duration::from_secs(if seq == 1 { 0 } else { 3 }))
    }

    /// Whether `request` is a day's: it offers the tool to speak with.
    fn is_day(request: &Request) -> bool {
        request.tools.iter().any(|tool| tool.name == SPEAK)
    }

    /// A model answering a day's request with `by_day` of it, and a night's
    /// as the canned player does, at once.
    fn by_day(
        by_day: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> FakeModel {
        FakeModel::answering(move |request| {
            if is_day(request) {
                by_day(request)
            } else {
                choosing_first(request)
            }
        })
    }

    /// A tool call as a model makes it: the tool's name and its arguments.
    type Made = (String, serde_json::Value);

    /// A call to `speak` saying `text`.
    fn speaking(text: &str) -> Made {
        (SPEAK.to_string(), json!({"text": text}))
    }

    /// A call to `select` voting against `target`.
    fn voting(target: &str) -> Made {
        (SELECT.to_string(), json!({"target": target}))
    }

    /// A call to `wait` for `seconds`.
    fn waiting(seconds: u64) -> Made {
        (WAIT.to_string(), json!({"seconds": seconds}))
    }

    /// The answer that waits out a day of `seconds`.
    fn waiting_out(seconds: u64) -> impl Fn(&Request) -> anyhow::Result<Response> {
        move |_| Ok(Response::calling(&[waiting(seconds)]))
    }

    /// The messages `player` logged sending, each with everyone it was sent
    /// to and how many memories the model was shown, in the log's order.
    fn sent_by<'a>(
        logged: &'a [Entry],
        player: &str,
    ) -> Vec<(&'a Message, &'a HashSet<ActorId>, Option<usize>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from,
                    to,
                    message,
                    seen,
                    ..
                } if from == player => Some((message, to, *seen)),
                _ => None,
            })
            .collect()
    }

    /// The utterances the players logged as their actions, in the log's
    /// order.
    fn utterances(logged: &[Entry]) -> Vec<&Message> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    message: message @ Message::Say { .. },
                    ..
                } => Some(message),
                _ => None,
            })
            .collect()
    }

    /// What `player` logged withholding, each with how many memories the
    /// model was shown, in the log's order.
    fn withheld_by<'a>(logged: &'a [Entry], player: &str) -> Vec<(&'a Message, Option<usize>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Withheld {
                    from,
                    message,
                    seen,
                } if from == player => Some((message, *seen)),
                _ => None,
            })
            .collect()
    }

    /// Short limits, so a phase that waits them out is told apart from
    /// one that does not: the votes stand after five seconds.
    fn quick() -> Rules {
        Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(10),
            ratify_limit: Some(Duration::from_secs(5)),
            ..rules()
        }
    }

    /// A werewolf, a doctor and two villagers. With every player selecting
    /// its first candidate, the doctor protects whomever the werewolf
    /// picks each night, the village votes a villager out each day, and
    /// the werewolves win after two days.
    fn doctored() -> HashMap<PlayerId, Role> {
        HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("doc".to_string(), Role::Doctor),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ])
    }

    /// The answer that selects the werewolf itself, who is no candidate.
    fn naming_itself(_: &Request) -> anyhow::Result<Response> {
        Ok(Response::tool_call(SELECT, &json!({"target": "wolf"})))
    }

    /// A response as a provider sends it, for the shapes `Response::tool_call`
    /// cannot make.
    fn answered(said: serde_json::Value) -> Response {
        serde_json::from_value(said).unwrap()
    }

    /// The answer that makes no tool call.
    fn saying_nothing(_: &Request) -> anyhow::Result<Response> {
        Ok(answered(
            json!({"choices": [{"message": {"content": "bob"}}]}),
        ))
    }

    /// The answer of a model that is down: a request that failed, and why.
    fn down(_: &Request) -> anyhow::Result<Response> {
        Err(anyhow!("connection refused")
            .context("POST http://localhost:1234/v1/chat/completions failed"))
    }

    /// The omniscient account of `entries`, line by line.
    fn narrated(entries: Vec<Entry>) -> Vec<String> {
        let mut narrator = Narrator::default();
        entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .collect()
    }

    /// The failures the players logged, each as who failed, in which
    /// phase, and what went wrong, in the log's order.
    fn failed(logged: &[Entry]) -> Vec<(&str, u64, &str)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Failed { player, seq, error } => {
                    Some((player.as_str(), *seq, error.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn the_request_tells_the_prompt_the_story_and_the_situation_and_forces_the_select_tool() {
        let model = answering(choosing_first);
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, _, _) = play(roles, rules(), Arc::clone(&model)).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        let requests = model.requests();
        assert_eq!(
            requests,
            [Request::offering(
                MODEL_ID,
                vec![
                    model::Message::system("You are wolf."),
                    model::Message::user(
                        "You are wolf, the only werewolf.\n\nNight 1.\n\n\
                         It is night 1, which lasts 1m. Choose whom the werewolves should kill. \
                         Answer by calling one of the tools."
                    ),
                ],
                vec![Tool::new(
                    "select",
                    "Knowing everything you know, select the best one.",
                    json!({
                        "type": "object",
                        "properties": {"target": {"type": "string", "enum": ["ann", "bob"]}},
                        "required": ["target"],
                        "additionalProperties": false,
                    }),
                )],
                ToolChoice::Required,
            )]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn allowed_a_tool_call_a_player_that_answers_in_prose_fails_and_the_game_goes_on() {
        // Every request allows a tool call. The werewolf answers in prose by
        // night, and everyone selects its first candidate by day.
        let model = answering(|request| {
            if request.messages[1].content.contains("It is night") {
                saying_nothing(request)
            } else {
                choosing_first(request)
            }
        });
        let roles = one_wolf_against(&["ann", "bob"]);
        let configuration = Configuration {
            tool_choice: ToolChoice::Auto,
            ..configuration(&roles)
        };
        let (winner, logged, _) =
            play_configured(roles, quick(), configuration, Arc::clone(&model))
                .await
                .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(
            failed(&logged),
            [("wolf", 1, "the model made no tool call")]
        );
        let requests = model.requests();
        assert!(!requests.is_empty());
        assert!(
            requests
                .iter()
                .all(|request| request.tool_choice == ToolChoice::Auto)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_good_answer_is_the_player_s_selection_for_the_phase() {
        let model = answering(|_| Ok(Response::tool_call(SELECT, &json!({"target": "bob"}))));
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(selections(&logged), [&selection(1, "wolf", 1, "bob")]);
        assert!(took < quick().night_limit, "{took:?}");
    }

    /// How many memories each of `player`'s selections says the model was
    /// shown, in the order they were made.
    fn seen_by(logged: &[Entry], player: &str) -> Vec<Option<usize>> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from,
                    message: Message::Select { .. },
                    seen,
                    ..
                } if from == player => Some(*seen),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn each_request_begins_with_the_history_of_the_one_before() {
        // The doctor is asked once a night and at least once a day, and
        // each request is told from everything it had heard and done by
        // then.
        let model = answering(choosing_first);
        play(doctored(), rules(), Arc::clone(&model)).await.unwrap();
        let asked = asked_of(&model, "doc");
        assert!(asked.len() >= 4, "{asked:?}");
        for pair in asked.windows(2) {
            assert!(pair[1].starts_with(story(&pair[0])), "{pair:?}");
        }
    }

    /// A player's memory of sending `message`, in no episode in particular.
    fn sent(message: Message) -> Event<Memory> {
        logged(Memory::Acted { message })
    }

    #[test]
    fn the_story_kept_with_the_history_is_at_every_step_a_fresh_narration_of_it() {
        let mind = Mind::new(
            "wolf".to_string(),
            told("wolf"),
            MODEL_ID.to_string(),
            ToolChoice::Required,
            Arc::new(FakeModel::failing("down")),
        );
        let pack = [("wolf", Role::Werewolf)];
        let night_2 = View {
            round: NonZero::new(2).unwrap(),
            ..seen(&["wolf", "bob"], &pack)
        };
        // A night, a day of talk and changing votes, and the next night,
        // each message remembered as it is heard or sent.
        let script = [
            received(announcement(1, seen(&["wolf", "ann", "bob"], &pack))),
            sent(selection(1, "wolf", 1, "ann")),
            received(announcement(2, seen_by_day(&["wolf", "bob"], &pack))),
            received(utterance(2, "bob", 1, "Who killed ann?")),
            sent(utterance(2, "wolf", 2, "Not me.")),
            received(selection(2, "bob", 2, "wolf")),
            received(tentative(2, 3, Some("wolf"))),
            sent(selection(2, "wolf", 3, "bob")),
            received(tentative(2, 4, None)),
            received(announcement(3, night_2)),
        ];
        for (n, event) in script.iter().enumerate() {
            mind.remember(event.clone());
            let remembered = mind.remembered.lock().unwrap();
            assert_eq!(remembered.history, script[..=n]);
            let mut narrator = Narrator::for_player("wolf".to_string());
            let fresh: Vec<String> = remembered
                .history
                .iter()
                .flat_map(|event| narrator.recall(event))
                .collect();
            assert_eq!(remembered.story, fresh, "after {} memories", n + 1);
        }
        assert_eq!(
            mind.story(),
            (
                [
                    "You are wolf, the only werewolf.",
                    "",
                    "Night 1.",
                    "You vote to kill ann.",
                    "ann dies.",
                    "",
                    "Day 1.",
                    "bob says, \"Who killed ann?\"",
                    "You say, \"Not me.\"",
                    "bob votes against wolf (werewolf).",
                    "wolf (werewolf) will be voted out in 20s unless the votes change.",
                    "You vote against bob.",
                    "Nobody is about to be voted out.",
                    "Nobody dies.",
                    "",
                    "Night 2.",
                ]
                .join("\n"),
                script.len()
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_by_night_is_logged_as_no_selection_and_the_day_is_played_normally() {
        // Nobody dies the night the werewolf fails to select, which waits
        // out its limit. By day everyone selects its first candidate, and
        // ann dies by day, bringing the werewolf to parity.
        for (by_night, error) in [
            (
                down as fn(&Request) -> anyhow::Result<Response>,
                "POST http://localhost:1234/v1/chat/completions failed: connection refused",
            ),
            (saying_nothing, "the model made no tool call"),
            (
                naming_itself,
                "the model selected wolf, who is not a candidate",
            ),
        ] {
            let model = answering(move |request| {
                if request.messages[1].content.contains("It is night") {
                    by_night(request)
                } else {
                    choosing_first(request)
                }
            });
            let roles = one_wolf_against(&["ann", "bob"]);
            let (winner, logged, took) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
            assert_eq!(winner, Team::Werewolves);
            // The night runs out, and the day's votes stand after five
            // seconds.
            assert_eq!(took, quick().night_limit + Duration::from_secs(5));
            assert_eq!(failed(&logged), [("wolf", 1, error)]);
            // A failure takes no number: by day each player speaks first,
            // so its vote is its second message, the werewolf's too.
            let mut selected = selections(&logged);
            selected.sort_by_key(|message| format!("{message:?}"));
            assert_eq!(
                selected,
                [
                    &selection(2, "ann", 2, "bob"),
                    &selection(2, "bob", 2, "ann"),
                    &selection(2, "wolf", 2, "ann")
                ]
            );
            // The werewolf's story is what it was shown and what it did,
            // and nothing of the failure.
            let asked = asked_of(&model, "wolf");
            assert!(
                story(&asked[1]).starts_with(
                    "You are wolf, the only werewolf.\n\
                     \n\
                     Night 1.\n\
                     Nobody dies.\n\
                     \n\
                     Day 1."
                ),
                "{}",
                asked[1]
            );
            assert!(
                asked.iter().all(|asked| !asked.contains("fail")),
                "{asked:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_whose_model_always_fails_is_told_failure_by_failure_and_still_ends() {
        // Two werewolves against two villagers are at parity once the first
        // night ends, so the game ends with nobody selecting anything.
        let roles = HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
        ]);
        let model = Arc::new(FakeModel::failing("the model is down"));
        let (winner, entries, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().night_limit);
        assert!(selections(&entries).is_empty());
        let mut told = narrated(entries);
        // The werewolves fail side by side, in no particular order. The
        // night takes no time because every event is stamped at the epoch,
        // while the game took its night limit by the environment's clock.
        told[9..11].sort();
        assert_eq!(
            told.join("\n"),
            "LLM with 4 players:\n\
             ann (villager)\n\
             bob (villager)\n\
             wolf1 (werewolf)\n\
             wolf2 (werewolf)\n\
             A night waits 10s and a day 10s, and a vote stands after 5s.\n\
             The model's context window is 4096 tokens, the default.\n\
             \n\
             Night 1.\n\
             wolf1 (werewolf) fails to select: the model is down\n\
             wolf2 (werewolf) fails to select: the model is down\n\
             Nobody dies.\n\
             Night 1 took 0s.\n\
             \n\
             The werewolves win after 0 days.\n\
             The game took 10s.\n\
             Survivors:\n\
             ann (villager)\n\
             bob (villager)\n\
             wolf1 (werewolf)\n\
             wolf2 (werewolf)"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn with_nobody_to_select_the_model_is_not_asked() {
        // Two werewolves know each other, so neither has anyone to select.
        let roles = HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
        ]);
        let model = answering(choosing_first);
        let (winner, logged, took) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert!(model.requests().is_empty());
        assert!(selections(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn the_log_opens_with_the_deal_then_the_configuration_then_the_first_night() {
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, _) = play(roles.clone(), rules(), answering(choosing_first))
            .await
            .unwrap();
        let between = between_the_deal_and_the_end(&logged, &rules(), &roles, winner);
        assert_eq!(
            between[0],
            Entry::Configuration(Box::new(configuration(&roles)))
        );
        // Every living player is announced the night, in name order, each
        // shown its own view.
        let everyone = ["wolf", "ann", "bob"];
        let announced = |n: u64, to: &str, known: Role| Entry::Action {
            from: ENVIRONMENT.to_string(),
            n,
            to: HashSet::from([to.to_string()]),
            message: Message::Announce {
                seq: 1,
                n,
                view: seen(&everyone, &[(to, known)]),
                limit: rules().night_limit,
            },
            seen: None,
        };
        assert_eq!(between[1], announced(1, "ann", Role::Villager));
        assert_eq!(between[2], announced(2, "bob", Role::Villager));
        assert_eq!(between[3], announced(3, "wolf", Role::Werewolf));
        // The configuration is logged once.
        let configurations = between[1..]
            .iter()
            .filter(|entry| matches!(entry, Entry::Configuration(_)))
            .count();
        assert_eq!(configurations, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_of_model_players_runs_to_a_winner() {
        let roles = doctored();
        let model = answering(choosing_first);
        let (winner, logged, _) = play(roles.clone(), rules(), Arc::clone(&model))
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Each announcement, as whom it went to and what it showed.
        let announced: Vec<(&PlayerId, &View)> =
            between_the_deal_and_the_end(&logged, &rules(), &roles, winner)
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Action {
                        to,
                        message: Message::Announce { view, .. },
                        ..
                    } => Some((to.iter().next().unwrap(), view)),
                    _ => None,
                })
                .collect();
        let awake = announced
            .iter()
            .filter(|(to, view)| super::awake(to, view))
            .count();
        let by_day = announced
            .iter()
            .filter(|(_, view)| view.phase == Phase::Day)
            .count();
        // Every living player is announced every phase: two nights of four
        // and three, and two days of the same. Every announcement of a
        // phase the player is awake in was asked about, and by day every
        // player was asked again about what it heard. Every player awake
        // selected once a phase, and spoke once a day.
        assert_eq!(announced.len(), 14);
        assert_eq!(awake, 11);
        assert_eq!(by_day, 7);
        assert!(model.requests().len() >= awake);
        assert_eq!(selections(&logged).len(), awake);
        assert_eq!(utterances(&logged).len(), by_day);
        let situations: HashSet<String> = model
            .requests()
            .iter()
            .map(|request| asked(&request.messages[1].content).to_string())
            .collect();
        assert_eq!(
            situations,
            HashSet::from([
                "It is night 1, which lasts 1m. Choose whom the werewolves should kill."
                    .to_string(),
                "It is night 1, which lasts 1m. Choose whom to protect.".to_string(),
                "It is day 1, which lasts 1m. You may speak, vote, or wait.".to_string(),
                "It is night 2, which lasts 1m. Choose whom the werewolves should kill."
                    .to_string(),
                "It is night 2, which lasts 1m. Choose whom to protect.".to_string(),
                "It is day 2, which lasts 1m. You may speak, vote, or wait.".to_string(),
            ])
        );
    }

    /// A werewolf against ann, bob, cat and dan, whose model by day 1 has
    /// bob say "Vote cat." and vote against cat, and everyone else wait,
    /// and plays as the canned player does otherwise: the werewolf kills
    /// ann the first night; by day only bob votes, and cat is voted out
    /// when the vote stands; the second night the werewolf kills bob and
    /// wins.
    fn only_bob_speaks_by_day_1() -> (HashMap<PlayerId, Role>, Arc<FakeModel>) {
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan"]);
        let model = Arc::new(by_day(|request| {
            let bob = request.messages[0] == model::Message::system(told("bob"));
            let calls = if bob && !voted_in_this_phase(&request.messages[1].content) {
                vec![speaking("Vote cat."), voting("cat")]
            } else {
                vec![waiting(10)]
            };
            Ok(Response::calling(&calls))
        }));
        (roles, model)
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_that_speaks_and_selects_sends_each_in_turn_to_whom_it_concerns() {
        let (roles, model) = only_bob_speaks_by_day_1();
        let (winner, logged, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Night 1 ends at once, day 1 five seconds after bob's vote, and
        // night 2 at once.
        assert_eq!(took, Duration::from_secs(5));
        let bob = "bob".to_string();
        let others = ["wolf", "cat", "dan"].map(String::from);
        // Bob's utterance goes to every other living player, and its vote
        // to them and the environment, in the order the model made the
        // calls, numbered in one series, both from the one call.
        let sent = sent_by(&logged, &bob);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0, &utterance(2, "bob", 1, "Vote cat."));
        assert_eq!(sent[0].1, &to_the_players_but(&bob, &others));
        assert_eq!(sent[1].0, &selection(2, "bob", 2, "cat"));
        assert_eq!(sent[1].1, &to_everyone_but(&bob, &others));
        assert!(sent[0].2.is_some());
        assert_eq!(sent[0].2, sent[1].2);
        // The other players observe both; the environment only the vote.
        for player in ["wolf", "cat", "dan"] {
            let observed = observed_by(&logged, player);
            assert!(observed.contains(&("bob", 1, 2)), "{player}: {observed:?}");
            assert!(observed.contains(&("bob", 2, 2)), "{player}: {observed:?}");
        }
        assert_eq!(
            observed_by_the_environment(&logged),
            [("wolf", 1, 1), ("bob", 2, 2), ("wolf", 2, 3)]
        );
        assert_eq!(
            selections_sent(&logged),
            [
                (&selection(1, "wolf", 1, "ann"), &to_the_environment()),
                (
                    &selection(2, "bob", 2, "cat"),
                    &to_everyone_but(&bob, &others)
                ),
                (&selection(3, "wolf", 2, "bob"), &to_the_environment()),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_remembers_what_it_hears_by_day_and_is_told_it_in_its_next_request() {
        let (roles, model) = only_bob_speaks_by_day_1();
        let (_, logged, _) = play(roles, quick(), Arc::clone(&model)).await.unwrap();
        // The werewolf's second night is told from everything it heard by
        // day: what bob said, bob's vote and the decision it made.
        let asked = asked_of(&model, "wolf");
        assert_eq!(
            asked.last().unwrap(),
            "You are wolf, the only werewolf.\n\
             \n\
             Night 1.\n\
             You vote to kill ann.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob says, \"Vote cat.\"\n\
             bob votes against cat.\n\
             cat will be voted out in 5s unless the votes change.\n\
             cat dies.\n\
             \n\
             Night 2.\n\
             \n\
             It is night 2, which lasts 10s. Choose whom the werewolves should kill. \
             Answer by calling one of the tools."
        );
        // What it heard counts among the memories it was shown.
        assert_eq!(seen_by(&logged, "wolf"), [Some(1), Some(7)]);
        // Every player logs observing bob's utterance, bob's vote and the
        // decision, in the phase it was in.
        let decision = decided(&logged);
        for player in ["wolf", "cat", "dan"] {
            let by_day: Vec<(&str, u64, u64)> = observed_by(&logged, player)
                .into_iter()
                .filter(|(_, _, seq)| *seq == 2)
                .collect();
            let announcement = announced_to(&logged, player, 2);
            assert_eq!(
                by_day,
                [
                    (ENVIRONMENT, announcement, 2),
                    ("bob", 1, 2),
                    ("bob", 2, 2),
                    (ENVIRONMENT, decision, 2)
                ]
            );
        }
    }

    /// The number of the environment's first tentative decision.
    fn decided(logged: &[Entry]) -> u64 {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Action {
                    from,
                    n,
                    message: Message::Tentative { .. },
                    ..
                } if from == ENVIRONMENT => Some(*n),
                _ => None,
            })
            .unwrap()
    }

    /// The number of the environment's announcement of the phase numbered
    /// `seq` to `player`.
    fn announced_to(logged: &[Entry], player: &str, seq: u64) -> u64 {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Action {
                    n,
                    to,
                    message: Message::Announce { seq: announced, .. },
                    ..
                } if *announced == seq && to.contains(player) => Some(*n),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn the_situation_is_the_phase_how_long_it_lasts_and_what_the_role_does_in_it() {
        let night = |known: &[(&str, Role)]| seen(&["wolf", "doc", "seer", "ann"], known);
        let me = |name: &str| name.to_string();
        let limit = Duration::from_secs(90);
        assert_eq!(
            situation(&me("wolf"), &night(&[("wolf", Role::Werewolf)]), limit).unwrap(),
            "It is night 1, which lasts 1m 30s. Choose whom the werewolves should kill."
        );
        assert_eq!(
            situation(&me("doc"), &night(&[("doc", Role::Doctor)]), limit).unwrap(),
            "It is night 1, which lasts 1m 30s. Choose whom to protect."
        );
        assert_eq!(
            situation(&me("seer"), &night(&[("seer", Role::Seer)]), limit).unwrap(),
            "It is night 1, which lasts 1m 30s. Choose whom to ask about."
        );
        // By day everyone votes, whatever its role.
        let mut day = night(&[("ann", Role::Villager)]);
        day.phase = Phase::Day;
        day.round = NonZero::new(2).unwrap();
        assert_eq!(
            situation(&me("ann"), &day, Duration::from_secs(60)).unwrap(),
            "It is day 2, which lasts 1m. You may speak, vote, or wait."
        );
        // A villager sleeps by night, and a player is shown its own role.
        assert!(situation(&me("ann"), &night(&[("ann", Role::Villager)]), limit).is_err());
        assert!(situation(&me("ann"), &night(&[]), limit).is_err());
    }

    #[tokio::test]
    async fn the_probe_asks_as_by_night_and_then_as_by_day_and_passes_when_the_model_plays_both() {
        let model = FakeModel::answering(choosing_first);
        let probed = check_plays(&model, MODEL_ID).await;
        assert!(
            matches!(probed, Ok(Probed::Played(ToolChoice::Required))),
            "{probed:?}"
        );
        assert!(not_playing(probed).is_none());
        let requests = model.requests();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(request.model, MODEL_ID);
            assert_eq!(request.tool_choice, ToolChoice::Required);
            assert!(request.messages[1].content.ends_with(ANSWER));
            assert_eq!(request.messages.len(), 2);
            assert_eq!(request.messages[0].role, model::Role::System);
            assert_eq!(request.messages[1].role, model::Role::User);
        }
        // The night's request offers a player's one tool by night, and the
        // day's a player's three by day, over the same candidates and a day
        // of a minute, and says what a day says.
        assert_eq!(requests[0].tools, night_tools(&placeholders()));
        assert_eq!(requests[1].tools, day_tools(&placeholders(), PROBE_DAY));
        assert!(
            requests[1].messages[1]
                .content
                .ends_with(&format!("{DAY_ASKED} {ANSWER}"))
        );
    }

    #[tokio::test]
    async fn the_probe_names_the_day_s_request_when_the_model_selects_by_night_but_makes_no_call_by_day()
     {
        let model = by_day(saying_nothing);
        let error = not_playing(check_plays(&model, MODEL_ID).await).unwrap();
        assert_eq!(
            format!("{error:#}"),
            format!("{MODEL_ID} did not answer a day's request: the model made no tool call")
        );
        assert_eq!(model.requests().len(), 2);
    }

    #[tokio::test]
    async fn the_probe_names_the_night_s_request_when_that_fails_and_never_sends_the_day_s() {
        let model = FakeModel::failing("the model is down");
        let error = not_playing(check_plays(&model, MODEL_ID).await).unwrap();
        assert_eq!(
            format!("{error:#}"),
            format!("{MODEL_ID} did not answer a night's request: the model is down")
        );
        assert_eq!(model.requests().len(), 1);
    }

    /// The error of a probe that found the model did not play, if it found
    /// so.
    fn not_playing(probed: anyhow::Result<Probed>) -> Option<anyhow::Error> {
        match probed.unwrap() {
            Probed::Played(_) => None,
            Probed::DidNotPlay(error) => Some(error),
        }
    }

    #[tokio::test]
    async fn the_probe_allows_a_tool_call_when_the_model_refuses_to_be_made_to_call_one() {
        // The night's request is refused under "required" and answered
        // under "auto", and the day's is asked under "auto", which this
        // provider would refuse under "required".
        let base_url = canned::refusing_required(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert!(matches!(
            check_plays(&provider, MODEL_ID).await.unwrap(),
            Probed::Played(ToolChoice::Auto)
        ));
    }

    #[tokio::test]
    async fn the_probe_tells_a_model_that_could_not_be_asked_after_refusing_to_be_made_to_call_a_tool()
     {
        let base_url = canned::refusing_required_then_busy(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = check_plays(&provider, MODEL_ID).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(&format!(
                "{MODEL_ID} could not be asked to play: POST {base_url}/chat/completions \
                 was answered 429 Too Many Requests"
            )),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn the_probe_gives_both_refusals_when_the_night_s_request_is_refused_either_way() {
        let base_url = canned::refusing(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = not_playing(check_plays(&provider, MODEL_ID).await).unwrap();
        let shown = format!("{error:#}");
        let refused = format!(
            "POST {base_url}/chat/completions was answered 400 Bad Request. \
             The provider said: {}",
            canned::refusal(MODEL_ID)
        );
        assert_eq!(
            shown,
            format!(
                "{MODEL_ID} did not answer a night's request: \
                 made to call a tool: {refused}\nasked to call one: {refused}"
            )
        );
    }

    #[tokio::test]
    async fn the_probe_asks_once_when_the_provider_refuses_for_another_reason_than_a_bad_request() {
        // The provider answers one request, so a second would not be
        // answered 401.
        let (base_url, _served) =
            canned::answering("401 Unauthorized", r#"{"error": {"message": "no key"}}"#);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = check_plays(&provider, MODEL_ID).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("was answered 401 Unauthorized"), "{shown}");
    }

    #[tokio::test]
    async fn the_probe_tells_a_model_that_could_not_be_asked_from_one_that_did_not_play() {
        // A provider that answers anything but success or a refusal of the
        // request as bad never put the question to the model.
        let (base_url, _served) = canned::answering("429 Too Many Requests", "{}");
        let provider = Provider::new(base_url.clone(), None, REQUEST_TIMEOUT).unwrap();
        let error = check_plays(&provider, MODEL_ID).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(&format!(
                "{MODEL_ID} could not be asked to play: POST {base_url}/chat/completions \
                 was answered 429 Too Many Requests"
            )),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn the_probe_judges_each_answer_as_a_player_s_is_judged() {
        let candidates = placeholders();
        // By night, as a selection; by day, as a whole.
        let elsewhere = Response::tool_call(SELECT, &json!({"target": "player3"}));
        let too_long = Response::calling(&[speaking("Hello."), waiting(61)]);
        let judged = [
            ("night", selected(&elsewhere, &candidates).unwrap_err()),
            (
                "day",
                super::asked(&too_long, &candidates, PROBE_DAY).unwrap_err(),
            ),
        ];
        let models = [
            FakeModel::responding(elsewhere),
            by_day(move |_| Ok(too_long.clone())),
        ];
        for ((phase, judged), model) in judged.into_iter().zip(models) {
            let error = not_playing(check_plays(&model, MODEL_ID).await).unwrap();
            let shown = format!("{error:#}");
            assert!(
                shown.starts_with(&format!("{MODEL_ID} did not answer a {phase}'s request")),
                "{shown}"
            );
            assert!(shown.ends_with(&format!("{judged:#}")), "{shown}");
        }
    }

    #[tokio::test]
    async fn the_probe_names_the_url_and_what_the_server_said_when_a_request_is_refused() {
        // A provider refusing the night's, and one refusing the day's after
        // answering the night's.
        let refused = [
            (
                canned::refusing(&[MODEL_ID]),
                "night",
                canned::refusal(MODEL_ID),
            ),
            (
                canned::refusing_days(&[MODEL_ID]),
                "day",
                canned::day_refusal(MODEL_ID),
            ),
        ];
        for (base_url, phase, said) in refused {
            let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
            let error = not_playing(check_plays(&provider, MODEL_ID).await).unwrap();
            let shown = format!("{error:#}");
            assert!(
                shown.starts_with(&format!("{MODEL_ID} did not answer a {phase}'s request")),
                "{shown}"
            );
            assert!(
                shown.contains(&format!(
                    "POST {base_url}/chat/completions was answered 400"
                )),
                "{shown}"
            );
            assert!(shown.ends_with(&said), "{shown}");
        }
    }

    #[test]
    fn an_answer_selects_only_by_a_select_call_whose_target_is_a_candidate() {
        let candidates = ["ann".to_string(), "bob".to_string()];
        assert_eq!(
            selected(
                &Response::tool_call(SELECT, &json!({"target": "bob"})),
                &candidates
            )
            .unwrap(),
            "bob"
        );
        let wrong = [
            ("no choice", answered(json!({"choices": []}))),
            (
                "no tool call",
                answered(json!({"choices": [{"message": {"content": "bob"}}]})),
            ),
            (
                "another tool",
                Response::tool_call("vote", &json!({"target": "bob"})),
            ),
            (
                "arguments that do not parse",
                answered(json!({
                    "choices": [{"message": {"tool_calls": [{"function": {"name": "select", "arguments": "{"}}]}}]
                })),
            ),
            ("no target", Response::tool_call(SELECT, &json!({}))),
            (
                "a target that is not a name",
                Response::tool_call(SELECT, &json!({"target": 3})),
            ),
            (
                "a target who is not a candidate",
                Response::tool_call(SELECT, &json!({"target": "wolf"})),
            ),
        ];
        for (what, response) in wrong {
            assert!(selected(&response, &candidates).is_err(), "{what}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn by_day_the_request_offers_the_three_tools_and_says_the_player_may_speak_vote_or_wait()
    {
        // Pat, a villager, sleeps through night 1 and is woken by day 1 by
        // its announcement, asks to wait nine seconds, and is woken again by
        // that reminder, to the same request, since nobody else says
        // anything.
        let model = Arc::new(by_day(waiting_out(9)));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", mute()),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, _, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        let candidates = ["bob", "wolf"];
        let requests = model.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        assert_eq!(
            requests[0],
            Request::offering(
                MODEL_ID,
                vec![
                    model::Message::system("You are pat."),
                    model::Message::user(
                        "You are pat, a villager.\n\nNight 1.\nYou sleep.\nann dies.\n\nDay 1.\n\n\
                         It is day 1, which lasts 10s. You may speak, vote, or wait. \
                         Answer by calling one of the tools."
                    ),
                ],
                vec![
                    Tool::new(
                        "speak",
                        "Say something to every other living player.",
                        json!({
                            "type": "object",
                            "properties": {"text": {"type": "string", "minLength": 1}},
                            "required": ["text"],
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "select",
                        "Vote for the player you want voted out. Your latest vote is the one that counts.",
                        json!({
                            "type": "object",
                            "properties": {"target": {"type": "string", "enum": candidates}},
                            "required": ["target"],
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "wait",
                        "Say nothing now, and be woken after this many seconds unless something wakes you sooner.",
                        json!({
                            "type": "object",
                            "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": 10}},
                            "required": ["seconds"],
                            "additionalProperties": false,
                        }),
                    ),
                ],
                ToolChoice::Required,
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn messages_heard_during_a_call_lead_to_one_more_call_made_from_all_of_them() {
        // Pat takes two seconds to answer and waits the day out. Bob says
        // five things a second into the day, while pat's first call is in
        // progress: they wake pat once more, and that call is told them all.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(2)));
        // Bob is a villager, announced by day alone.
        let bob = puppet(Box::new(|me, seq, _| {
            (1..=5)
                .map(|i| {
                    let point = utterance(seq, me, i, &format!("Point {i}."));
                    (Duration::from_secs(1), point)
                })
                .collect()
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", bob),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        let asked = asked_of(&model, "pat");
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(!asked[0].contains("bob says"), "{}", asked[0]);
        for i in 1..=5 {
            let heard = format!("bob says, \"Point {i}.\"");
            assert!(story(&asked[1]).contains(&heard), "{}", asked[1]);
        }
        // Pat observed each of them by day.
        let heard = observed_by(&logged, "pat")
            .into_iter()
            .filter(|(from, _, seq)| *from == "bob" && *seq == 2)
            .count();
        assert_eq!(heard, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_wakes_the_player_when_it_is_up_unless_a_later_wait_or_the_end_of_the_phase_came_first()
     {
        // Pat, the seer, is woken three times by day 1. By the
        // announcement, when it asks to wait five seconds and then two, of
        // which only the second counts. Two seconds in, by that reminder,
        // when it votes against bob and asks to wait six. And at once by
        // the decision its vote brings, when it asks to wait nine. Bob is
        // voted out five seconds after the vote, and pat is announced night
        // 2 then, so the reminder from its first answer, five seconds in,
        // and the one from its second, eight seconds in, are both ignored.
        // Were either honored, pat would be asked a fourth time by day,
        // answer with no call, and fail; it never fails.
        let answers = [
            vec![waiting(5), waiting(2)],
            vec![voting("bob"), waiting(6)],
            vec![waiting(9)],
        ];
        let day_calls = AtomicUsize::new(0);
        let model = Arc::new(by_day(move |_| {
            let call = day_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Response::calling(
                &answers.get(call).cloned().unwrap_or_default(),
            ))
        }));
        // The werewolf kills ann at once the first night, and takes three
        // seconds every night after.
        let wolf = hunting_at_once_then_after_three_seconds();
        let mut roles = one_wolf_against(&["ann", "bob", "cat"]);
        roles.insert("pat".to_string(), Role::Seer);
        let puppets = HashMap::from([
            ("wolf", wolf),
            ("ann", mute()),
            ("bob", mute()),
            ("cat", mute()),
        ]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Day 1 ends seven seconds in, and night 2 three seconds later.
        assert_eq!(took, Duration::from_secs(10));
        assert_eq!(asked_of(&model, "pat").len(), 5);
        assert!(failed(&logged).is_empty(), "{logged:?}");
        assert_eq!(
            selections(&logged),
            [
                &selection(1, "wolf", 1, "ann"),
                &selection(1, "pat", 1, "ann"),
                &selection(2, "pat", 2, "bob"),
                &selection(3, "pat", 3, "cat"),
                &selection(3, "wolf", 3, "cat"),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_day_s_response_with_one_bad_call_is_a_failure_and_nothing_in_it_is_carried_out() {
        let model = Arc::new(by_day(|_| {
            Ok(Response::calling(&[
                speaking("I accuse bob."),
                voting("pat"),
            ]))
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", mute()),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        assert_eq!(
            failed(&logged),
            [("pat", 2, "the model selected pat, who is not a candidate")]
        );
        assert!(sent_by(&logged, "pat").is_empty(), "{logged:?}");
        assert_eq!(model.requests().len(), 1);
    }

    #[test]
    fn a_day_s_response_is_judged_as_a_whole_and_its_calls_kept_in_order() {
        let candidates = ["ann".to_string(), "bob".to_string()];
        let limit = Duration::from_secs(60);
        let judged = |calls: &[Made]| super::asked(&Response::calling(calls), &candidates, limit);
        assert_eq!(
            judged(&[speaking("Hi."), voting("bob"), waiting(60)]).unwrap(),
            Asked {
                sends: vec![
                    Sending::Say("Hi.".to_string()),
                    Sending::Select("bob".to_string())
                ],
                wait: Some(Duration::from_secs(60)),
            }
        );
        // The messages keep the model's order, and of several waits the
        // last counts.
        assert_eq!(
            judged(&[voting("ann"), waiting(5), speaking("Hi."), waiting(1)]).unwrap(),
            Asked {
                sends: vec![
                    Sending::Select("ann".to_string()),
                    Sending::Say("Hi.".to_string())
                ],
                wait: Some(Duration::from_secs(1)),
            }
        );
        assert_eq!(
            judged(&[speaking("Hi.")]).unwrap(),
            Asked {
                sends: vec![Sending::Say("Hi.".to_string())],
                wait: None,
            }
        );
        let wrong: [(&str, Vec<Made>, &str); 10] = [
            ("no tool call", vec![], "the model made no tool call"),
            (
                "another tool",
                vec![("vote".to_string(), json!({"target": "bob"}))],
                "the model called vote, which is not a tool it was offered",
            ),
            (
                "arguments that do not parse",
                vec![(SPEAK.to_string(), json!("Hi."))],
                "the arguments of the model's speak call",
            ),
            (
                "no text",
                vec![(SPEAK.to_string(), json!({}))],
                "speak call",
            ),
            (
                "empty text",
                vec![speaking("")],
                "the model's speak call says nothing",
            ),
            (
                "blank text",
                vec![speaking(" \n")],
                "the model's speak call says nothing",
            ),
            (
                "a target who is not a candidate",
                vec![speaking("Hi."), voting("cat")],
                "the model selected cat, who is not a candidate",
            ),
            (
                "too short a wait",
                vec![waiting(0)],
                "the model asked to wait 0s, which is not between 1s and 1m",
            ),
            (
                "too long a wait",
                vec![waiting(61)],
                "the model asked to wait 61s, which is not between 1s and 1m",
            ),
            (
                "a wait that is not a number",
                vec![(WAIT.to_string(), json!({"seconds": "soon"}))],
                "the arguments of the model's wait call",
            ),
        ];
        for (what, calls, said) in wrong {
            let error = judged(&calls).unwrap_err();
            assert!(format!("{error:#}").contains(said), "{what}: {error:#}");
        }
        // A response with no choice makes no tool call.
        let none = super::asked(&answered(json!({"choices": []})), &candidates, limit);
        assert!(none.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_that_comes_after_the_phase_ended_is_withheld_and_logged_as_such() {
        // Sam, the seer, takes three seconds to answer. Bob votes against
        // cat as soon as day 1 opens, and the vote stands a second later,
        // so night 2 is announced while sam's day call is in progress. When
        // it comes back, speaking, voting and asking to wait, nothing is
        // sent and no reminder is set.
        let model = Arc::new(
            by_day(|_| {
                Ok(Response::calling(&[
                    speaking(SAID),
                    voting("bob"),
                    waiting(2),
                ]))
            })
            .taking(Duration::from_secs(3)),
        );
        // Bob is a villager, announced by day alone.
        let bob = puppet(Box::new(|me, seq, _| {
            at_once(vec![selection(seq, me, 1, "cat")])
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::from_secs(3))),
            ("ann", mute()),
            ("bob", bob),
            ("cat", mute()),
        ]);
        let mut roles = one_wolf_against(&["ann", "bob", "cat"]);
        roles.insert("sam".to_string(), Role::Seer);
        let rules = Rules {
            ratify_limit: Some(Duration::from_secs(1)),
            ..quick()
        };
        let (winner, entries, took) = play_with(roles, rules, Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Night 1 takes three seconds, day 1 one, and night 2 five: the
        // werewolf kills three seconds in, and sam, woken for the night as
        // soon as its day call came back, answers two seconds after that.
        assert_eq!(took, Duration::from_secs(9));
        let withheld = withheld_by(&entries, "sam");
        assert_eq!(withheld.len(), 2, "{withheld:?}");
        assert_eq!(withheld[0].0, &utterance(2, "sam", 0, SAID));
        assert_eq!(withheld[1].0, &selection(2, "sam", 0, "bob"));
        assert!(withheld[0].1.is_some());
        assert_eq!(withheld[0].1, withheld[1].1);
        // Nothing of the day was sent, remembered or numbered: sam's two
        // selections are its first and second messages.
        assert!(utterances(&entries).is_empty(), "{entries:?}");
        let sam: Vec<&Message> = sent_by(&entries, "sam")
            .into_iter()
            .map(|(message, _, _)| message)
            .collect();
        assert_eq!(
            sam,
            [
                &selection(1, "sam", 1, "ann"),
                &selection(3, "sam", 2, "bob")
            ]
        );
        assert_eq!(asked_of(&model, "sam").len(), 3);
        let told = narrated(entries);
        for line in [
            format!("sam (seer) is too late to say, for day 1: \"{SAID}\""),
            "sam (seer) chooses bob (villager) for day 1 too late to count.".to_string(),
        ] {
            assert!(told.contains(&line), "{told:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn by_night_a_player_is_woken_only_by_its_announcement_and_ignores_talk_from_the_day() {
        // The werewolf takes two seconds to answer and waits each day out.
        // Dan says something ten and a half seconds into day 1, which has
        // ended by then: the werewolf hears it by night 2, during its call
        // for the night, which is not asked again, observes it, and is
        // never told it.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(2)));
        // Dan is a villager, announced by day alone.
        let dan = puppet(Box::new(|me, seq, _| {
            vec![(
                Duration::from_millis(10_500),
                utterance(seq, me, 1, "Anyone there?"),
            )]
        }));
        let puppets = HashMap::from([
            ("ann", mute()),
            ("bob", mute()),
            ("cat", mute()),
            ("dan", dan),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        // Each night takes two seconds and each day runs out; the werewolf
        // kills ann, bob and cat in turn and is at parity with dan.
        assert_eq!(took, Duration::from_secs(26));
        let asked = asked_of(&model, "wolf");
        assert_eq!(asked.len(), 5, "{asked:?}");
        assert_eq!(
            story(&asked[3]),
            "You are wolf, the only werewolf.\n\
             \n\
             Night 1.\n\
             You vote to kill ann.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             Nobody dies.\n\
             \n\
             Night 2.\n\
             You vote to kill bob.\n\
             bob dies.\n\
             \n\
             Day 2."
        );
        // The werewolf observed dan's utterance in night 2.
        assert!(
            observed_by(&logged, "wolf").contains(&("dan", 1, 3)),
            "{logged:?}"
        );
        assert!(
            asked.iter().all(|asked| !asked.contains("Anyone there?")),
            "{asked:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_announced_a_night_it_sleeps_through_is_woken_by_nothing_until_the_day() {
        // Pat, a villager, takes a tenth of a second to answer and asks to
        // wait two seconds whenever asked. The werewolf kills ann at once
        // the first night, and bob three seconds into the second. By day 1
        // bob votes against cat at once, whose vote stands a second later,
        // and says two things just before the day ends and one more during
        // night 2, which bob does not know has begun, and pat ignores. By
        // day 2 dan votes the werewolf out.
        let model = Arc::new(by_day(waiting_out(2)).taking(Duration::from_millis(100)));
        let wolf = hunting_at_once_then_after_three_seconds();
        let bob = puppet(Box::new(|me, seq, _| {
            vec![
                (Duration::ZERO, selection(seq, me, 1, "cat")),
                (
                    Duration::from_millis(950),
                    utterance(seq, me, 2, "Going quiet."),
                ),
                (Duration::from_millis(970), utterance(seq, me, 3, "Really.")),
                (
                    Duration::from_millis(2_500),
                    utterance(seq, me, 4, "Anyone there?"),
                ),
            ]
        }));
        let dan = puppet(Box::new(|me, seq, _| match seq {
            4 => at_once(vec![selection(seq, me, 1, "wolf")]),
            _ => vec![],
        }));
        let puppets = HashMap::from([
            ("wolf", wolf),
            ("ann", mute()),
            ("bob", bob),
            ("cat", mute()),
            ("dan", dan),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "cat", "dan", "pat"]);
        let rules = Rules {
            ratify_limit: Some(Duration::from_secs(1)),
            ..quick()
        };
        let (winner, logged, took) = play_with(roles, rules, Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Villagers);
        // Night 1 ends at once, day 1 a second later, night 2 three seconds
        // after that, and day 2 a second after it opens.
        assert_eq!(took, Duration::from_secs(5));
        assert!(failed(&logged).is_empty(), "{logged:?}");
        // Pat observes each night's announcement in it, and bob's last
        // utterance in night 2, which it does not remember, and is asked
        // nothing by night. Its day reminders fall in night 2 and are
        // ignored.
        let pat = observed_by(&logged, "pat");
        for night in [1, 3] {
            let announcement = announced_to(&logged, "pat", night);
            assert!(pat.contains(&(ENVIRONMENT, announcement, night)), "{pat:?}");
        }
        assert!(pat.contains(&("bob", 4, 3)), "{pat:?}");
        let asked = asked_of(&model, "pat");
        assert!(
            asked.iter().all(|asked| !asked.contains("It is night")),
            "{asked:?}"
        );
        // By day 1 pat is asked three times: by the announcement, once for
        // bob's vote and the decision together, and by bob's first
        // utterance. That call is in progress when bob's second arrives and
        // when night 2 is announced, and the wake-up waiting behind it finds
        // pat asleep. By day 2 it is asked twice: by the announcement, and
        // once for dan's vote and the decision together.
        let by_day_1 = asked
            .iter()
            .filter(|asked| asked.contains("It is day 1"))
            .count();
        assert_eq!(by_day_1, 3, "{asked:?}");
        assert_eq!(asked.len(), 5, "{asked:?}");
        assert_eq!(
            story(&asked[3]),
            "You are pat, a villager.\n\
             \n\
             Night 1.\n\
             You sleep.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob votes against cat.\n\
             cat will be voted out in 1s unless the votes change.\n\
             bob says, \"Going quiet.\"\n\
             bob says, \"Really.\"\n\
             cat dies.\n\
             \n\
             Night 2.\n\
             You sleep.\n\
             bob dies.\n\
             \n\
             Day 2.\n\
             dan votes against wolf."
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_ignores_messages_from_another_phase_and_takes_in_those_of_its_own() {
        // Pat, a villager, takes a second to answer and waits each day out.
        // As soon as day 1 opens, bob sends talk, a vote and a tentative
        // decision tagged with night 1, while pat's call for the
        // announcement is in progress: pat observes each and ignores it,
        // and none leaves a wake-up waiting behind the call. Three, four
        // and five seconds in, bob sends one of each tagged with the day,
        // each of which wakes pat, and pat is told them. Bob's vote against
        // the werewolf stands, and the village wins nine seconds in.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(1)));
        // Bob is a villager, announced by day alone.
        let bob = puppet(Box::new(|me, seq, _| {
            vec![
                (Duration::ZERO, utterance(seq - 1, me, 1, "Night talk.")),
                (Duration::ZERO, selection(seq - 1, me, 2, "pat")),
                (Duration::ZERO, tentative(seq - 1, 30, Some("pat"))),
                (Duration::from_secs(3), utterance(seq, me, 3, "Day talk.")),
                (Duration::from_secs(4), selection(seq, me, 4, "wolf")),
                (Duration::from_secs(5), tentative(seq, 60, None)),
            ]
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", bob),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Villagers);
        assert_eq!(took, Duration::from_secs(9));
        assert!(failed(&logged).is_empty(), "{logged:?}");
        // Pat observed all six in the day, among the environment's
        // announcement and decision. A tentative decision names no sender,
        // so bob's are observed as the environment's.
        let announcement = announced_to(&logged, "pat", 2);
        let decision = decided(&logged);
        let by_day: Vec<(&str, u64, u64)> = observed_by(&logged, "pat")
            .into_iter()
            .filter(|(_, _, seq)| *seq == 2)
            .collect();
        assert_eq!(
            by_day,
            [
                (ENVIRONMENT, announcement, 2),
                ("bob", 1, 2),
                ("bob", 2, 2),
                (ENVIRONMENT, 30, 2),
                ("bob", 3, 2),
                ("bob", 4, 2),
                (ENVIRONMENT, decision, 2),
                (ENVIRONMENT, 60, 2),
            ]
        );
        // Pat was never told the three from the night, and its second call
        // is the one bob's day talk woke, with nothing in between.
        let asked = asked_of(&model, "pat");
        assert!(asked.len() >= 4, "{asked:?}");
        for line in [
            "Night talk.",
            "bob votes against pat.",
            "pat will be voted out",
        ] {
            assert!(asked.iter().all(|asked| !asked.contains(line)), "{asked:?}");
        }
        assert!(asked[1].contains("bob says, \"Day talk.\""), "{asked:?}");
        assert_eq!(
            story(asked.last().unwrap()),
            "You are pat, a villager.\n\
             \n\
             Night 1.\n\
             You sleep.\n\
             ann dies.\n\
             \n\
             Day 1.\n\
             bob says, \"Day talk.\"\n\
             bob votes against wolf.\n\
             wolf will be voted out in 5s unless the votes change.\n\
             Nobody is about to be voted out."
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_night_s_response_that_comes_after_the_day_is_announced_is_withheld_and_logged() {
        // Sam, the seer, takes three seconds to answer, and the night waits
        // two for it. Day 1 is announced before sam's selection comes back,
        // so nothing is sent, and the selection is logged as withheld. Bob
        // votes sam out at once, and the werewolf wins four seconds in.
        let model = Arc::new(by_day(waiting_out(10)).taking(Duration::from_secs(3)));
        let bob = puppet(Box::new(|me, seq, _| {
            at_once(vec![selection(seq, me, 1, "sam")])
        }));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", bob),
        ]);
        let mut roles = one_wolf_against(&["ann", "bob"]);
        roles.insert("sam".to_string(), Role::Seer);
        let rules = Rules {
            night_limit: Duration::from_secs(2),
            ratify_limit: Some(Duration::from_secs(2)),
            ..quick()
        };
        let (winner, entries, took) = play_with(roles, rules, Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(4));
        assert!(failed(&entries).is_empty(), "{entries:?}");
        let withheld = withheld_by(&entries, "sam");
        assert_eq!(withheld.len(), 1, "{withheld:?}");
        assert_eq!(withheld[0].0, &selection(1, "sam", 0, "ann"));
        assert!(withheld[0].1.is_some());
        assert!(sent_by(&entries, "sam").is_empty(), "{entries:?}");
        let line = "sam (seer) chooses ann (villager) for night 1 too late to count.".to_string();
        assert!(narrated(entries).contains(&line));
    }

    #[tokio::test(start_paused = true)]
    async fn a_speak_that_is_blank_is_a_failure_and_nothing_is_sent() {
        let model = Arc::new(by_day(|_| Ok(Response::calling(&[speaking(" ")]))));
        let puppets = HashMap::from([
            ("wolf", hunting_after(Duration::ZERO)),
            ("ann", mute()),
            ("bob", mute()),
        ]);
        let roles = one_wolf_against(&["ann", "bob", "pat"]);
        let (winner, logged, took) = play_with(roles, quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().day_limit);
        assert_eq!(
            failed(&logged),
            [("pat", 2, "the model's speak call says nothing")]
        );
        assert!(utterances(&logged).is_empty(), "{logged:?}");
        assert_eq!(model.requests().len(), 1);
    }
}
