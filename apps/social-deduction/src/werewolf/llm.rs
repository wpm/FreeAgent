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
//! The game is the announcing environment's, with a model [`Player`] in
//! every seat, and the day's vote is ratified: a day ends one ratify limit
//! after its votes stop changing, or at the day limit with nobody voted
//! out. A player remembers what it is shown, what it hears and what it
//! does, as a [`Memory`] of each message stamped when it was received or
//! sent. Each time it is woken, it tells the model that history as a story
//! from its own point of view and where the game stands, with how long the
//! phase lasts, and carries out what the model answers. The call is made in
//! the player's think loop, so the player keeps perceiving while the model
//! works.
//!
//! By night a player is woken once, by its announcement, and has the model
//! select one of its [candidates] with one forced `select` call, sent to
//! the environment alone. By day it is woken by every message it receives:
//! the announcement, another player's utterance or selection, and the
//! environment's tentative decision. Wake-ups are debounced: the perceive
//! loop remembers each message and wakes the think loop only when no
//! wake-up is already waiting, and the think loop takes the history as it
//! stands when it is woken, so a player has at most one call in progress
//! and one waiting, however fast the talk comes. A day's request offers
//! three tools, of which the model must call one and may call several:
//! `speak`, which sends what it says to every other living player and not
//! to the environment; `select`, which sends its vote to the environment
//! and every other living player, so the votes are public; and `wait`,
//! which asks to be woken after some seconds unless something wakes the
//! player sooner. A reminder is numbered so that only the latest set
//! counts, and one for a phase that has ended is ignored. The calls are
//! carried out in the order the model made them. Before carrying a day's
//! out, the player checks that the day the call began in is still the phase
//! it was last announced; if not, it sends nothing and logs each message it
//! would have sent as [`Entry::Withheld`]. A night's selection is sent
//! whatever the phase, as before, and the environment drops a late one.
//!
//! A model that is not reached, or an answer that is not well formed, is a
//! failure: the player logs it as [`Entry::Failed`], carries nothing out,
//! and the game goes on. The player logs an [`Entry::Observation`] for each
//! message it receives and an [`Entry::Action`] for each utterance and
//! selection it sends, numbered together from 1, with how many memories the
//! model was shown when it made the call that produced the message. Before
//! a game begins, [`check_selects`] asks the model for one selection as a
//! player would by night, so a model that does not select fails at launch,
//! in the provider's words. [`game`] builds the episode from the deal, the
//! rules, the model, and the [`Configuration`] the log opens with, which
//! [`Settings::configuration`] records from the settings, the deal and the
//! rendered prompts.

use super::announced;
use super::report::Narrator;
use super::{
    Entry, Message, Phase, PhaseLimits, PlayerId, Role, RoleCounts, Rules, Table, Team, View, act,
    candidates, observe, to_everyone_but, to_the_environment, to_the_players_but,
};
use crate::model::{
    self, Provider, REQUEST_TIMEOUT, Request, Response, Tool, api_key, key_variable,
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
    /// otherwise from the file. A ratify limit longer than the day limit
    /// is an error naming both, since the day would run out before any
    /// vote stood.
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
        Ok(Settings {
            table: overrides.roles.or(file.or(Table::default())),
            night_limit: overrides
                .limits
                .night_limit
                .or(self.phases.night_limit)
                .unwrap_or(rules.night_limit),
            day_limit,
            ratify_limit,
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
    /// variable is named, and the key itself never recorded. A player with
    /// no prompt rendered for its role is an error naming the seat and the
    /// role.
    pub fn configuration(
        &self,
        roles: &HashMap<PlayerId, Role>,
        prompts: &[SystemPrompt],
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
        Player::init(prompt, model_id.clone(), Arc::clone(&model))
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

/// What a player remembers: a message it received from another actor, or
/// one it sent. Reminders to itself, and what it withheld, are not
/// remembered.
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
/// done, the phase it is in, whether a wake-up is waiting, how many
/// reminders it has set, what it is told, and whom it asks.
struct Mind {
    /// The history, in order: each message received and each selection or
    /// utterance sent, stamped when the player remembered it. Never locked
    /// across an await.
    history: Mutex<Vec<Event<Memory>>>,
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
    /// The model asked each time the player is woken.
    model: Arc<dyn model::Model>,
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
    /// A mind with nothing in its history yet, told `prompt`, asking
    /// `model` as the model `model_id`.
    fn new(prompt: String, model_id: String, model: Arc<dyn model::Model>) -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            n: AtomicU64::new(0),
            announced: Mutex::new(None),
            waking: AtomicBool::new(false),
            reminders: AtomicU64::new(0),
            prompt,
            model_id,
            model,
        }
    }

    /// Add `event` to the history. A message received, or an utterance
    /// sent, goes at the end. A selection sent goes at the end of its own
    /// phase: before the first announcement of a later phase, if one has
    /// come, which is where it belongs in the story. One made after a later
    /// phase was announced, which the environment has dropped, was still
    /// made in its own phase, and is not told as a deed of the phase that
    /// followed.
    fn remember(&self, event: Event<Memory>) {
        let mut history = self.history.lock().unwrap();
        let at = match &event.payload {
            Memory::Acted {
                message: Message::Select { seq, .. },
            } => history
                .iter()
                .position(|remembered| {
                    matches!(
                        &remembered.payload,
                        Memory::Observed {
                            message: Message::Announce { seq: announced, .. },
                        } if announced > seq
                    )
                })
                .unwrap_or(history.len()),
            _ => history.len(),
        };
        history.insert(at, event);
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

    /// Whether the reminder numbered `count` for the phase numbered `seq`
    /// counts: it is the latest set, and the phase is the one the player is
    /// in.
    fn reminder_counts(&self, seq: u64, count: u64) -> bool {
        count == self.reminders.load(Ordering::Relaxed) && seq == self.seq()
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
    /// player is told `prompt`, and asks `model` as the model `model_id`.
    pub fn init(
        prompt: String,
        model_id: String,
        model: Arc<dyn model::Model>,
    ) -> (Builder<Actor>, Option<ThinkBuilder<Message, Entry>>) {
        let mind = Arc::new(Mind::new(prompt, model_id, model));
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

    /// Add `memory` to the history, stamped now, where it belongs in the
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
    /// `limit`: the system prompt, then the history as a story told to the
    /// player, a blank line, and where the game stands, with `tools` to
    /// answer with. With it, how many memories the story was told from.
    fn request(
        &self,
        view: &View,
        limit: Duration,
        tools: Vec<Tool>,
    ) -> anyhow::Result<(Request, usize)> {
        let me = &self.context.id;
        let mut narrator = Narrator::for_player(me.clone());
        let (story, seen) = {
            let history = self.mind.history.lock().unwrap();
            let story: Vec<String> = history
                .iter()
                .flat_map(|event| narrator.recall(event))
                .collect();
            (story, history.len())
        };
        let asked = format!("{}\n\n{}", story.join("\n"), situation(me, view, limit)?);
        let request = Request::offering(
            &self.mind.model_id,
            vec![
                model::Message::system(&self.mind.prompt),
                model::Message::user(asked),
            ],
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
    /// told of it. With nobody to select, the model is not asked. Being
    /// woken before any phase was announced is an error.
    async fn decide(&self) -> anyhow::Result<()> {
        self.mind.woken();
        let Announced { seq, view, limit } = self
            .mind
            .announced()
            .context("woken before any phase was announced")?;
        let candidates = candidates(&self.context.id, &view);
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
    /// selection to the environment. The model's failure to select, as
    /// [`selection`] judges it, is an error, and no selection; so is a
    /// request that cannot be put together or a selection that cannot be
    /// sent, which are the player's own doing, and they read so in the
    /// error.
    async fn select_by_night(
        &self,
        seq: u64,
        view: &View,
        limit: Duration,
        candidates: &[PlayerId],
    ) -> anyhow::Result<()> {
        let (request, seen) = self.request(view, limit, vec![select(candidates)])?;
        let target = selection(self.mind.model.as_ref(), &request, candidates).await?;
        let selection = Message::Select {
            seq,
            from: self.context.id.clone(),
            n: self.mind.number(),
            target,
        };
        self.send(selection, to_the_environment(), seen)
    }

    /// Ask the model what to do in the day numbered `seq`, which `view`
    /// shows and which lasts `limit`: speak, vote for one of `candidates`,
    /// or wait, in any number and order, with one response. A response
    /// that is not well formed, as [`asked`] judges it, is an error, and
    /// nothing in it is carried out. If the day has ended by the time the
    /// model answers, nothing is sent and no reminder set, and each message
    /// the response asked for is logged as withheld, unnumbered. Otherwise
    /// the messages are sent in the order the model asked for them, each
    /// utterance to every other living player and each selection to them
    /// and the environment, and a wait sets a reminder.
    async fn play_by_day(
        &self,
        seq: u64,
        view: &View,
        limit: Duration,
        candidates: &[PlayerId],
    ) -> anyhow::Result<()> {
        let me = &self.context.id;
        let tools = vec![speak(), vote(candidates), wait(limit)];
        let (request, seen) = self.request(view, limit, tools)?;
        let response = self.mind.model.complete(&request).await?;
        let Asked { sends, wait } = asked(&response, candidates, limit)?;
        if self.mind.seq() != seq {
            for sending in sends {
                self.context.log(Entry::Withheld {
                    from: me.clone(),
                    message: sending.message(seq, me, 0),
                    seen: Some(seen),
                });
            }
            return Ok(());
        }
        for sending in sends {
            let to = sending.receivers(me, &view.alive);
            let message = sending.message(seq, me, self.mind.number());
            self.send(message, to, seen)?;
        }
        if let Some(after) = wait {
            self.remind(seq, after)?;
        }
        Ok(())
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
    /// observed in the latest phase announced, and remember it. An
    /// announcement is of a new phase, and wakes the think loop. By day so
    /// does everything else heard: another player's utterance or
    /// selection, and the environment's tentative decision. By night those
    /// are remembered and nothing more. Anything else is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Some(n) = message.number() {
            let announced = if let Message::Announce {
                seq, view, limit, ..
            } = message
            {
                self.mind.announce(*seq, view, *limit);
                true
            } else {
                false
            };
            observe(&self.context, message.sender(), n, self.mind.seq());
            self.remember(Memory::Observed {
                message: message.clone(),
            });
            if announced || self.mind.by_day() {
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
        (Phase::Day, _) => ("day", "You may speak, vote, or wait."),
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

/// Whom `model` selects from `candidates` when sent `request`: its
/// response, judged by [`selected`]. A model that cannot be reached, or
/// answers with something that is not a selection of a candidate, is an
/// error. This is the one way a player asks by night, and the one way the
/// probe does, so they agree on what a selection is.
async fn selection(
    model: &dyn model::Model,
    request: &Request,
    candidates: &[PlayerId],
) -> anyhow::Result<PlayerId> {
    let response = model.complete(request).await?;
    selected(&response, candidates)
}

/// What [`check_selects`] found.
#[derive(Debug)]
pub enum Probed {
    /// Asked for a selection, the model selected a candidate.
    Selected,
    /// Asked for a selection, the model did not: the provider refused the
    /// request as bad, or the answer was no selection. The error names the
    /// model and carries the cause beneath it.
    DidNotSelect(anyhow::Error),
}

/// Check that `model`, asked as a player is, selects a candidate: one
/// request of a player's shape, a short system prompt, a line asking for
/// a selection, and the `select` tool over two placeholder candidates,
/// judged as a player's answer is. A model that could not be asked at
/// all, because the provider was not reached or answered with anything
/// but the model's answer, is an error naming the model, with the cause
/// beneath it.
pub async fn check_selects(model: &dyn model::Model, model_id: &str) -> anyhow::Result<Probed> {
    let candidates = ["player1".to_string(), "player2".to_string()];
    let request = Request::new(
        model_id,
        vec![
            model::Message::system("You are a player in a game, about to be asked to select."),
            model::Message::user("Select one of the candidates."),
        ],
        select(&candidates),
    );
    match selection(model, &request, &candidates).await {
        Ok(_) => Ok(Probed::Selected),
        Err(error) if model::concerns_the_model(&error) => Ok(Probed::DidNotSelect(
            error.context(format!("{model_id} did not select when asked")),
        )),
        Err(error) => Err(error.context(format!("{model_id} could not be asked to select"))),
    }
}

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
/// not empty.
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
                if text.is_empty() {
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

/// What a day's call has the player send.
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

    /// Whom among `alive` this goes to from `me`: an utterance to the other
    /// living players, a selection to them and the environment.
    fn receivers(&self, me: &PlayerId, alive: &HashSet<PlayerId>) -> HashSet<ActorId> {
        match self {
            Sending::Say(_) => to_the_players_but(me, alive),
            Sending::Select(_) => to_everyone_but(me, alive),
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
/// that do not parse, a target who is not one of `candidates`, empty text,
/// or seconds not between 1 and `limit` in whole seconds is an error, and
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
    use crate::werewolf::{ENVIRONMENT, thinking_episode};
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
        settings.configuration(roles, &settings.prompts()?)
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

    /// The situation a request's user message ends with.
    fn asked(asked: &str) -> &str {
        asked.rsplit_once("\n\n").unwrap().1
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
        settle(MODEL, Overrides::default())
            .configuration(roles, &prompts)
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
                    Player::init(told(id), MODEL_ID.to_string(), Arc::clone(&model))
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
        puppet(Box::new(move |me, seq, view| match view.phase {
            Phase::Night => vec![(after, selection(seq, me, seq, &candidates(me, view)[0]))],
            Phase::Day => vec![],
        }))
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
            [Request::new(
                MODEL_ID,
                vec![
                    model::Message::system("You are wolf."),
                    model::Message::user(
                        "You are wolf, the only werewolf.\n\nNight 1.\n\n\
                         It is night 1, which lasts 1m. Choose whom the werewolves should kill."
                    ),
                ],
                Tool::new(
                    "select",
                    "Knowing everything you know, select the best one.",
                    json!({
                        "type": "object",
                        "properties": {"target": {"type": "string", "enum": ["ann", "bob"]}},
                        "required": ["target"],
                        "additionalProperties": false,
                    }),
                ),
            )]
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

    #[test]
    fn a_late_selection_is_remembered_at_the_end_of_its_own_phase_and_told_there() {
        let mind = Mind::new(
            told("wolf"),
            MODEL_ID.to_string(),
            Arc::new(FakeModel::failing("down")),
        );
        let everyone = ["wolf", "ann", "bob"];
        let pack = [("wolf", Role::Werewolf)];
        let selected = |seq| {
            logged(Memory::Acted {
                message: selection(seq, "wolf", seq, "ann"),
            })
        };
        let night = received(announcement(1, seen(&everyone, &pack)));
        let day = received(announcement(2, seen_by_day(&everyone, &pack)));
        let heard = received(selection(2, "ann", 1, "bob"));
        // The day is announced, and ann's vote heard, while the model is
        // still deciding the night; the werewolf then decides the day too.
        mind.remember(night.clone());
        mind.remember(day.clone());
        mind.remember(heard.clone());
        mind.remember(selected(1));
        mind.remember(selected(2));
        let history = mind.history.lock().unwrap();
        assert_eq!(*history, [night, selected(1), day, heard, selected(2)]);
        let mut narrator = Narrator::for_player("wolf".to_string());
        let story: Vec<String> = history
            .iter()
            .flat_map(|event| narrator.recall(event))
            .collect();
        assert_eq!(
            story,
            [
                "You are wolf, the only werewolf.",
                "",
                "Night 1.",
                "You vote to kill ann.",
                "Nobody dies.",
                "",
                "Day 1.",
                "ann votes against bob.",
                "You vote against ann."
            ]
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
        let mut narrator = Narrator::default();
        let mut told: Vec<String> = entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .collect();
        // The werewolves fail side by side, in no particular order. The
        // night takes no time because every event is stamped at the epoch,
        // while the game took its night limit by the environment's clock.
        told[8..10].sort();
        assert_eq!(
            told.join("\n"),
            "LLM with 4 players:\n\
             ann (villager)\n\
             bob (villager)\n\
             wolf1 (werewolf)\n\
             wolf2 (werewolf)\n\
             A night waits 10s and a day 10s, and a vote stands after 5s.\n\
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
        assert_eq!(
            between[1],
            Entry::Action {
                from: ENVIRONMENT.to_string(),
                n: 1,
                to: HashSet::from(["wolf".to_string()]),
                message: Message::Announce {
                    seq: 1,
                    n: 1,
                    view: seen(&["wolf", "ann", "bob"], &[("wolf", Role::Werewolf)]),
                    limit: rules().night_limit,
                },
                seen: None,
            }
        );
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
        let announced: Vec<&Phase> =
            between_the_deal_and_the_end(&logged, &rules(), &roles, winner)
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Action {
                        message: Message::Announce { view, .. },
                        ..
                    } => Some(&view.phase),
                    _ => None,
                })
                .collect();
        let by_day = announced
            .iter()
            .filter(|phase| ***phase == Phase::Day)
            .count();
        // Every announcement was asked about, and by day every player was
        // asked again about what it heard. Every player selected once a
        // phase, and spoke once a day.
        assert_eq!(announced.len(), 11);
        assert_eq!(by_day, 7);
        assert!(model.requests().len() >= announced.len());
        assert_eq!(selections(&logged).len(), announced.len());
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
             It is night 2, which lasts 10s. Choose whom the werewolves should kill."
        );
        // What it heard counts among the memories it was shown.
        assert_eq!(seen_by(&logged, "wolf"), [Some(1), Some(7)]);
        // Every player logs observing bob's utterance, bob's vote and the
        // decision, in the phase it was in.
        let decision = logged.iter().find_map(|entry| match entry {
            Entry::Action {
                n,
                message: Message::Tentative { .. },
                ..
            } => Some(*n),
            _ => None,
        });
        let decision = decision.unwrap();
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
    fn a_tentative_decision_and_anothers_vote_are_remembered_at_the_end_and_told_in_turn() {
        let mind = Mind::new(
            told("wolf"),
            MODEL_ID.to_string(),
            Arc::new(FakeModel::failing("down")),
        );
        let everyone = ["wolf", "ann", "bob"];
        let pack = [("wolf", Role::Werewolf)];
        mind.remember(received(announcement(2, seen_by_day(&everyone, &pack))));
        mind.remember(received(selection(2, "ann", 1, "bob")));
        mind.remember(received(tentative(2, 4, Some("bob"))));
        mind.remember(received(selection(2, "bob", 1, "ann")));
        mind.remember(received(tentative(2, 5, None)));
        let history = mind.history.lock().unwrap();
        let mut narrator = Narrator::for_player("wolf".to_string());
        let story: Vec<String> = history
            .iter()
            .flat_map(|event| narrator.recall(event))
            .collect();
        assert_eq!(
            story,
            [
                "You are wolf, the only werewolf.",
                "",
                "Day 1.",
                "ann votes against bob.",
                "bob will be voted out in 20s unless the votes change.",
                "bob votes against ann.",
                "Nobody is about to be voted out."
            ]
        );
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
    async fn the_probe_asks_for_one_selection_as_a_player_does_and_passes_when_the_model_selects() {
        let model = FakeModel::answering(choosing_first);
        assert!(not_selecting(check_selects(&model, MODEL_ID).await).is_none());
        let requests = model.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.model, MODEL_ID);
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, model::Role::System);
        assert_eq!(request.messages[1].role, model::Role::User);
        assert_eq!(
            request.tools,
            [select(&["player1".to_string(), "player2".to_string()])]
        );
    }

    #[tokio::test]
    async fn the_probe_s_error_names_the_model_and_carries_the_failure() {
        let model = FakeModel::failing("the model is down");
        let error = not_selecting(check_selects(&model, MODEL_ID).await).unwrap();
        assert_eq!(
            format!("{error:#}"),
            format!("{MODEL_ID} did not select when asked: the model is down")
        );
    }

    /// The error of a probe that found the model did not select, if it
    /// found so.
    fn not_selecting(probed: anyhow::Result<Probed>) -> Option<anyhow::Error> {
        match probed.unwrap() {
            Probed::Selected => None,
            Probed::DidNotSelect(error) => Some(error),
        }
    }

    #[tokio::test]
    async fn the_probe_tells_a_model_that_could_not_be_asked_from_one_that_did_not_select() {
        // A provider that answers anything but success or a refusal of the
        // request as bad never put the question to the model.
        let (base_url, _served) = canned::answering("429 Too Many Requests", "{}");
        let provider = Provider::new(base_url.clone(), None, REQUEST_TIMEOUT).unwrap();
        let error = check_selects(&provider, MODEL_ID).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(&format!(
                "{MODEL_ID} could not be asked to select: POST {base_url}/chat/completions \
                 was answered 429 Too Many Requests"
            )),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn the_probe_judges_the_answer_as_a_player_s_is_judged() {
        let candidates = ["player1".to_string(), "player2".to_string()];
        let prose: Response =
            serde_json::from_value(json!({"choices": [{"message": {"content": "player1"}}]}))
                .unwrap();
        let elsewhere = Response::tool_call(SELECT, &json!({"target": "player3"}));
        for response in [prose, elsewhere] {
            let judged = selected(&response, &candidates).unwrap_err();
            let model = FakeModel::responding(response);
            let error = not_selecting(check_selects(&model, MODEL_ID).await).unwrap();
            let shown = format!("{error:#}");
            assert!(shown.starts_with(MODEL_ID), "{shown}");
            assert!(shown.ends_with(&format!("{judged:#}")), "{shown}");
        }
    }

    #[tokio::test]
    async fn the_probe_names_the_url_and_what_the_server_said_when_the_request_is_refused() {
        let base_url = canned::refusing(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = not_selecting(check_selects(&provider, MODEL_ID).await).unwrap();
        let shown = format!("{error:#}");
        assert!(shown.starts_with(MODEL_ID), "{shown}");
        assert!(
            shown.contains(&format!(
                "POST {base_url}/chat/completions was answered 400"
            )),
            "{shown}"
        );
        assert!(shown.ends_with(&canned::refusal(MODEL_ID)), "{shown}");
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

    #[test]
    fn an_utterance_is_remembered_at_the_end_whatever_its_phase_and_told_in_its_own() {
        let mind = Mind::new(
            told("wolf"),
            MODEL_ID.to_string(),
            Arc::new(FakeModel::failing("down")),
        );
        let everyone = ["wolf", "ann", "bob"];
        let pack = [("wolf", Role::Werewolf)];
        let said = |from, n, text| {
            logged(Memory::Acted {
                message: utterance(2, from, n, text),
            })
        };
        let mut night_2 = seen(&everyone, &pack);
        night_2.round = NonZero::new(2).unwrap();
        mind.remember(received(announcement(2, seen_by_day(&everyone, &pack))));
        mind.remember(received(utterance(2, "ann", 1, "I suspect bob.")));
        mind.remember(said("wolf", 1, "So do I."));
        mind.remember(received(announcement(3, night_2)));
        // Day talk heard by night, and said after the night was announced,
        // stays at the end: the history is only appended to.
        mind.remember(received(utterance(2, "bob", 1, "Too slow.")));
        mind.remember(said("wolf", 2, "Late words."));
        let history = mind.history.lock().unwrap();
        let mut narrator = Narrator::for_player("wolf".to_string());
        let story: Vec<String> = history
            .iter()
            .flat_map(|event| narrator.recall(event))
            .collect();
        assert_eq!(
            story,
            [
                "You are wolf, the only werewolf.",
                "",
                "Day 1.",
                "ann says, \"I suspect bob.\"",
                "You say, \"So do I.\"",
                "Nobody dies.",
                "",
                "Night 2.",
                "During day 1, bob said, \"Too slow.\"",
                "During day 1, you said, \"Late words.\""
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn by_day_the_request_offers_the_three_tools_and_says_the_player_may_speak_vote_or_wait()
    {
        // Pat, a villager, is woken by day 1 by its announcement, asks to
        // wait nine seconds, and is woken again by that reminder, to the
        // same request, since nobody else says anything.
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
                        "You are pat, a villager.\n\nDay 1.\n\n\
                         It is day 1, which lasts 10s. You may speak, vote, or wait."
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
        let wolf = puppet(Box::new(|me, seq, view| match view.phase {
            Phase::Night => {
                let after = if seq == 1 { 0 } else { 3 };
                let kill = selection(seq, me, seq, &candidates(me, view)[0]);
                vec![(Duration::from_secs(after), kill)]
            }
            Phase::Day => vec![],
        }));
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
        let wrong: [(&str, Vec<Made>, &str); 9] = [
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
        let mut narrator = Narrator::default();
        let told: Vec<String> = entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .collect();
        for line in [
            format!("sam (seer) is too late to say, for day 1: \"{SAID}\""),
            "sam (seer) chooses bob (villager) for day 1 too late to count.".to_string(),
        ] {
            assert!(told.contains(&line), "{told:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn by_night_a_player_is_woken_only_by_its_announcement_and_remembers_talk_in_its_phase() {
        // The werewolf takes two seconds to answer and waits each day out.
        // Dan says something ten and a half seconds into day 1, which has
        // ended by then: the werewolf hears it by night 2, during its call
        // for the night, which is not asked again, and is told it by day 2
        // as said during day 1.
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
             During day 1, dan said, \"Anyone there?\"\n\
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
    }
}
