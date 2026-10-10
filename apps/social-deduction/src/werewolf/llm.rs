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
//! every seat. A player remembers what it is shown and what it does, as
//! the entries the environment logs about it. Announced a phase, it tells
//! the model that history as a story from its own point of view and where
//! the game stands, and has it select one of the player's [candidates]
//! with one forced tool call. The call is made in the player's think loop,
//! so the player keeps perceiving while the model works. An answer that is
//! not a selection is no selection. [`game`] builds the episode from the
//! deal, the rules, the model, and the [`Configuration`] the log opens
//! with, which [`Settings::configuration`] records from the settings, the
//! deal and the rendered prompts.

use super::announced;
use super::report::Narrator;
use super::{
    ENVIRONMENT, Entry, Message, Observation, Phase, PhaseLimits, PlayerId, Role, RoleCounts,
    Rules, Table, Team, candidates,
};
use crate::model::{
    self, Provider, REQUEST_TIMEOUT, Request, Response, Tool, api_key, key_variable,
};
use anyhow::{Context as _, anyhow, bail};
use async_trait::async_trait;
use clap::Args;
use free_agent::{Behavior, Builder, Context, Episode, Logger, Think, ThinkBuilder};
use minijinja::{Environment, UndefinedBehavior, Value, context};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// What the command line may say over the file: the role counts, the
/// phase limits, and the model. Each is unset unless given, so that the
/// file can be heard.
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// How many of each role sit at the table.
    #[command(flatten)]
    pub roles: RoleCounts,
    /// How long each phase waits for the players.
    #[command(flatten)]
    pub limits: PhaseLimits,
    /// The model and where it is.
    #[command(flatten)]
    pub model: ModelOverrides,
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

/// The `[phases]` table: how long each phase waits for the players, for
/// the phases that say.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phases {
    /// How long the night waits for a player.
    #[serde(default, with = "humantime_serde")]
    pub night_limit: Option<Duration>,
    /// How long the day waits for a player.
    #[serde(default, with = "humantime_serde")]
    pub day_limit: Option<Duration>,
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
    /// line: each role count and phase limit from the command line if
    /// given there, otherwise from the file, otherwise from the code, and
    /// the model's provider and id from the command line if given there,
    /// otherwise from the file.
    pub fn settle(mut self, overrides: Overrides) -> Settings {
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
        Settings {
            table: overrides.roles.or(file.or(Table::default())),
            night_limit: overrides
                .limits
                .night_limit
                .or(self.phases.night_limit)
                .unwrap_or(rules.night_limit),
            day_limit: overrides
                .limits
                .day_limit
                .or(self.phases.day_limit)
                .unwrap_or(rules.day_limit),
            config: self,
        }
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

    /// The rules a game is played under: the variant's, with the phase
    /// limits that took effect.
    pub fn rules(&self) -> Rules {
        Rules {
            night_limit: self.night_limit,
            day_limit: self.day_limit,
            ..rules()
        }
    }
}

/// The default `request_timeout`, for `serde`.
fn request_timeout() -> Duration {
    REQUEST_TIMEOUT
}

/// The rules of the model-played variant: the uniform-random variant's,
/// played as "LLM".
pub fn rules() -> Rules {
    Rules {
        variation: "LLM".to_string(),
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
/// and a model player for each role, who may send to the environment. The
/// environment logs `configuration` right after the deal. Each player is
/// told its system prompt, which `configuration` has for every player in
/// `roles`, and makes each selection by asking `model` as the model
/// `configuration` names. The winning team is sent on `winner` when the
/// game ends.
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
}

// Nobody in this game requests, so the sides only receive and start.
#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Player(player) => player.context(),
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Player(player) => player.start().await,
        }
    }
}

/// What a player's two loops share: what it has been shown and what it has
/// done, what it is told, and whom it asks.
struct Mind {
    /// The history, in order, as the entries the environment logs about
    /// the player: an announcement sent to it, and a selection received
    /// from it. Never locked across an await.
    history: Mutex<Vec<Entry>>,
    /// Its system prompt, rendered for its seat and role.
    prompt: String,
    /// The model's id, as each request names it.
    model_id: String,
    /// The model asked for each selection.
    model: Arc<dyn model::Model>,
}

/// A player whose selections are made by a language model. Its perceive
/// loop and its think loop are each a `Player` with a context of its own
/// over one shared mind: the perceive loop remembers each announcement and
/// hands it to the think loop, which tells the model the history as a story
/// and sends the environment what the model selects. A variant's actor
/// enum holds the perceive loop's, built by [`Player::init`].
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
        let mind = Arc::new(Mind {
            history: Mutex::new(Vec::new()),
            prompt,
            model_id,
            model,
        });
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

    /// Add `entry` to the history.
    fn remember(&self, entry: Entry) {
        self.mind.history.lock().unwrap().push(entry);
    }

    /// The request for a selection from `candidates` in the phase
    /// `observation` shows: the system prompt, then the history as a story
    /// told to the player, a blank line, and where the game stands, with
    /// the [`select`] tool to answer with.
    fn request(
        &self,
        observation: &Observation,
        candidates: &[PlayerId],
    ) -> anyhow::Result<Request> {
        let me = &self.context.id;
        let mut narrator = Narrator::for_player(me.clone());
        let story: Vec<String> = self
            .mind
            .history
            .lock()
            .unwrap()
            .iter()
            .flat_map(|entry| narrator.narrate(entry))
            .collect();
        let asked = format!("{}\n\n{}", story.join("\n"), situation(me, observation)?);
        Ok(Request::new(
            &self.mind.model_id,
            vec![
                model::Message::system(&self.mind.prompt),
                model::Message::user(asked),
            ],
            select(candidates),
        ))
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Announced a phase, remember it and think about it. Anything else is
    /// ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::Announce { .. } = message {
            self.remember(Entry::Sent {
                to: self.context.id.clone(),
                message: message.clone(),
            });
            self.context.think(message.clone())?;
        }
        Ok(())
    }
}

#[async_trait]
impl Think for Player {
    type Message = Message;

    /// Announced a phase, select someone in it: ask the model, remember
    /// what it selects, and send the selection to the environment. With
    /// nobody to select, the model is not asked. An answer that is not a
    /// selection of a candidate is an error, and no selection. Nothing
    /// else is thought about.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        let Message::Announce { seq, observation } = message else {
            return Ok(());
        };
        let me = &self.context.id;
        let candidates = candidates(me, observation);
        if candidates.is_empty() {
            return Ok(());
        }
        let request = self.request(observation, &candidates)?;
        let response = self.mind.model.complete(&request).await?;
        let selection = Message::Select {
            seq: *seq,
            from: me.clone(),
            target: selected(&response, &candidates)?,
        };
        self.remember(Entry::Received {
            from: me.clone(),
            message: selection.clone(),
        });
        self.context
            .send(selection, HashSet::from([ENVIRONMENT.to_string()]))
    }
}

/// Where the game stands for `me` as `observation` shows it, and what is
/// asked of it: by night, what its role does, and by day, the vote. A
/// player awake by night is a werewolf, the doctor or the seer, and is
/// shown its own role.
fn situation(me: &PlayerId, observation: &Observation) -> anyhow::Result<String> {
    let (phase, asked) = match (&observation.phase, observation.roles.get(me)) {
        (Phase::Day, _) => ("day", "Choose whom to vote out."),
        (Phase::Night, Some(Role::Werewolf)) => {
            ("night", "Choose whom the werewolves should kill.")
        }
        (Phase::Night, Some(Role::Doctor)) => ("night", "Choose whom to protect."),
        (Phase::Night, Some(Role::Seer)) => ("night", "Choose whom to ask about."),
        (Phase::Night, Some(Role::Villager)) => bail!("{me} is a villager, who sleeps by night"),
        (Phase::Night, None) => bail!("{me} is not shown its own role"),
    };
    Ok(format!("It is {phase} {}. {asked}", observation.round))
}

/// The name of the one tool a player is offered.
const SELECT: &str = "select";

/// The tool a player selects with: `select`, taking a `target` that is one
/// of `candidates`.
fn select(candidates: &[PlayerId]) -> Tool {
    Tool::new(
        SELECT,
        "Knowing everything you know, select the best one.",
        json!({
            "type": "object",
            "properties": {"target": {"type": "string", "enum": candidates}},
            "required": ["target"],
            "additionalProperties": false,
        }),
    )
}

/// The arguments of a [`select`] call.
#[derive(Deserialize)]
struct Selection {
    target: PlayerId,
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
    let Selection { target } = serde_json::from_str(&call.arguments).with_context(|| {
        format!(
            "the arguments of the model's {SELECT} call: {}",
            call.arguments
        )
    })?;
    if !candidates.contains(&target) {
        bail!("the model selected {target}, who is not a candidate");
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::werewolf::tests::{
        Played, between_the_deal_and_the_end, one_wolf_against, received, run, seen, selection,
    };
    use clap::Parser;
    use secrecy::ExposeSecret;
    use std::num::NonZero;
    use tokio::sync::mpsc::unbounded_channel;

    /// A file naming only the model, which is all that is required.
    const MODEL: &str = r#"
[model]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
"#;

    /// The settings `file` and `overrides` produce under the code defaults.
    fn settle(file: &str, overrides: Overrides) -> Settings {
        Config::parse(file).unwrap().settle(overrides)
    }

    /// The command line of a variant that takes the overrides.
    #[derive(Parser)]
    struct Variant {
        #[command(flatten)]
        overrides: Overrides,
    }

    #[test]
    fn the_examples_parse_and_render_and_name_a_model_known_to_make_tool_calls() {
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
            let id = &config.model.id;
            assert!(crate::model::makes_tool_calls(id), "{name}: {id}");
            let prompts = config
                .settle(Overrides::default())
                .prompts()
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert!(!prompts.is_empty(), "{name}");
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
                    day_limit: Some(Duration::from_secs(7)),
                },
                ..Overrides::default()
            },
        );
        assert_eq!(settings.night_limit, Duration::from_secs(5));
        assert_eq!(settings.day_limit, Duration::from_secs(7));
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
    fn the_rules_are_the_variant_s_with_the_limits_that_took_effect() {
        let file = format!("{MODEL}\n[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"7s\"\n");
        let settings = settle(&file, Overrides::default());
        let rules = settings.rules();
        assert_eq!(rules.variation, "LLM");
        assert_eq!(rules.night_limit, settings.night_limit);
        assert_eq!(rules.day_limit, settings.day_limit);
    }

    #[test]
    fn the_command_line_takes_durations_in_humantime_form() {
        let variant = Variant::parse_from(["llm", "--night-limit", "30s", "--day-limit", "1m 30s"]);
        assert_eq!(
            variant.overrides.limits,
            PhaseLimits {
                night_limit: Some(Duration::from_secs(30)),
                day_limit: Some(Duration::from_secs(90)),
            }
        );
        assert!(Variant::try_parse_from(["llm", "--night-limit", "soon"]).is_err());
        assert!(Variant::try_parse_from(["llm", "--day-limit", "30"]).is_err());
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

    /// Short limits, so a phase that waits them out is told apart from
    /// one that does not.
    fn quick() -> Rules {
        Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(10),
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

    /// The answer of a model that is down.
    fn down(_: &Request) -> anyhow::Result<Response> {
        Err(anyhow!("the model is down"))
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
                        "You are wolf, the only werewolf.\nNight 1.\n\n\
                         It is night 1. Choose whom the werewolves should kill."
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
        let sent = serde_json::to_value(&requests[0]).unwrap();
        assert_eq!(
            sent["tool_choice"],
            json!({"type": "function", "function": {"name": "select"}})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_good_answer_is_the_player_s_selection_for_the_phase() {
        let model = answering(|_| Ok(Response::tool_call(SELECT, &json!({"target": "bob"}))));
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, took) = play(roles, quick(), model).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(received(&logged), [&selection(1, "wolf", "bob")]);
        assert!(took < quick().night_limit, "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn each_request_begins_with_the_history_of_the_one_before() {
        let model = answering(choosing_first);
        play(doctored(), rules(), Arc::clone(&model)).await.unwrap();
        let asked = asked_of(&model, "doc");
        assert_eq!(
            asked.last().unwrap(),
            "You are doc, the doctor.\n\
             Night 1.\n\
             You protect ann.\n\
             Nobody dies.\n\
             Day 1.\n\
             You vote against ann.\n\
             ann is voted out.\n\
             Night 2.\n\
             You protect bob.\n\
             Nobody dies.\n\
             Day 2.\n\
             \n\
             It is day 2. Choose whom to vote out."
        );
        assert_eq!(asked.len(), 4);
        for pair in asked.windows(2) {
            assert!(pair[1].starts_with(story(&pair[0])), "{pair:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_answer_by_night_is_no_selection_and_the_day_is_played_normally() {
        // Nobody dies the night the werewolf fails to select, which waits
        // out its limit. By day everyone selects its first candidate, and
        // ann is voted out, bringing the werewolf to parity.
        for by_night in [
            naming_itself as fn(&Request) -> anyhow::Result<Response>,
            down,
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
            assert_eq!(took, quick().night_limit);
            assert_eq!(model.requests().len(), 4);
            let mut received = received(&logged);
            received.sort_by_key(|message| format!("{message:?}"));
            assert_eq!(
                received,
                [
                    &selection(2, "ann", "bob"),
                    &selection(2, "bob", "ann"),
                    &selection(2, "wolf", "ann")
                ]
            );
        }
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
        assert!(received(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn the_log_opens_with_the_deal_then_the_configuration_then_the_first_night() {
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, _) = play(roles.clone(), rules(), answering(choosing_first))
            .await
            .unwrap();
        let between = between_the_deal_and_the_end(&logged, "LLM", &roles, winner);
        assert_eq!(
            between[0],
            Entry::Configuration(Box::new(configuration(&roles)))
        );
        assert_eq!(
            between[1],
            Entry::Sent {
                to: "wolf".to_string(),
                message: Message::Announce {
                    seq: 1,
                    observation: seen(&["wolf", "ann", "bob"], &[("wolf", Role::Werewolf)]),
                },
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
        let announced = between_the_deal_and_the_end(&logged, "LLM", &roles, winner)
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    Entry::Sent {
                        message: Message::Announce { .. },
                        ..
                    }
                )
            })
            .count();
        // Every announcement was asked about, and every answer selected.
        assert_eq!(announced, 11);
        assert_eq!(model.requests().len(), announced);
        assert_eq!(received(&logged).len(), announced);
        let situations: HashSet<String> = model
            .requests()
            .iter()
            .map(|request| asked(&request.messages[1].content).to_string())
            .collect();
        assert_eq!(
            situations,
            HashSet::from([
                "It is night 1. Choose whom the werewolves should kill.".to_string(),
                "It is night 1. Choose whom to protect.".to_string(),
                "It is day 1. Choose whom to vote out.".to_string(),
                "It is night 2. Choose whom the werewolves should kill.".to_string(),
                "It is night 2. Choose whom to protect.".to_string(),
                "It is day 2. Choose whom to vote out.".to_string(),
            ])
        );
    }

    #[test]
    fn the_situation_is_the_phase_and_what_the_role_does_in_it() {
        let night = |known: &[(&str, Role)]| seen(&["wolf", "doc", "seer", "ann"], known);
        let me = |name: &str| name.to_string();
        assert_eq!(
            situation(&me("wolf"), &night(&[("wolf", Role::Werewolf)])).unwrap(),
            "It is night 1. Choose whom the werewolves should kill."
        );
        assert_eq!(
            situation(&me("doc"), &night(&[("doc", Role::Doctor)])).unwrap(),
            "It is night 1. Choose whom to protect."
        );
        assert_eq!(
            situation(&me("seer"), &night(&[("seer", Role::Seer)])).unwrap(),
            "It is night 1. Choose whom to ask about."
        );
        // By day everyone votes, whatever its role.
        let mut day = night(&[("ann", Role::Villager)]);
        day.phase = Phase::Day;
        day.round = NonZero::new(2).unwrap();
        assert_eq!(
            situation(&me("ann"), &day).unwrap(),
            "It is day 2. Choose whom to vote out."
        );
        // A villager sleeps by night, and a player is shown its own role.
        assert!(situation(&me("ann"), &night(&[("ann", Role::Villager)])).is_err());
        assert!(situation(&me("ann"), &night(&[])).is_err());
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
        // A response as a provider sends it, for the shapes `tool_call`
        // cannot make.
        let answered = |said: serde_json::Value| serde_json::from_value::<Response>(said).unwrap();
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
}
