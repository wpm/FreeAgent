//! A configuration file, of any game: its `game` key names the game, its
//! `format` key the [`Format`] it is written in, and the rest of it is that
//! game's configuration. [`load`] is the one place a configuration file is
//! read.
//!
//! The file's text is parsed twice: once for its `game` alone, and then
//! straight into that game's [`Config`], so that `toml` places every error
//! inside the game's settings at its line and column.
//!
//! Every game's file defines named models, each a [`Spec`], and named
//! [`Strategy`]s, each a policy with its parameters, and allots each
//! role's seats among the strategies. The strategy [`RANDOM`] is always
//! defined. The [`Lineup`] the allotments make deals the (role, strategy)
//! pairs to the seats at random, and renders every [`SystemPrompt`] a game
//! could call for, so that a broken template stops the program before any
//! game begins. A game's file settles into its [`Settings`], which set up
//! the models in play and make the [`Configuration`] a game runs under.

use crate::avalon::{self, Avalon};
use crate::game::{self, Game};
use crate::werewolf::{self, Werewolf};
use crate::{prompted, random};
use anyhow::{Context as _, anyhow, bail};
use free_agent_game::config::Program;
use free_agent_game::seat::{Dealt, PlayerId, numbered, seats};
use free_agent_game::strategy::{RANDOM, Random, RoleConfig, Strategy};
use free_agent_model::spec::{self, Handles, Keys, Models, Probed, Spec};
use free_agent_model::window::{self, ContextWindow};
use free_agent_model::{Model, Provider, ToolChoice};
use minijinja::value::merge_maps;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use rand::seq::SliceRandom;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use social_deduction_gui_types::settings::{is_part_name, part_variable};
use std::collections::{BTreeMap, HashMap, btree_map};
use std::fmt;
use std::hash::Hash;
use std::path::Path;
use std::sync::Arc;

/// One game's settled settings, of either kind, boxed, since the two
/// differ in size.
#[derive(Debug)]
pub enum Settled {
    /// A game of Werewolf.
    Werewolf(Box<Settings<Werewolf>>),
    /// A game of Avalon.
    Avalon(Box<Settings<Avalon>>),
}

/// A game, as a configuration file's `game` key names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Name {
    /// `"werewolf"`.
    Werewolf,
    /// `"avalon"`.
    Avalon,
}

impl Name {
    /// Every game, in order.
    pub const ALL: [Name; 2] = [Name::Werewolf, Name::Avalon];

    /// What the game's probe finds of the model `model_id`, asked first
    /// under `from`, as [`Game::check_plays`] probes it.
    pub async fn check_plays(
        self,
        model: &dyn Model,
        model_id: &str,
        from: ToolChoice,
    ) -> anyhow::Result<Probed> {
        match self {
            Name::Werewolf => Werewolf::check_plays(model, model_id, from).await,
            Name::Avalon => Avalon::check_plays(model, model_id, from).await,
        }
    }
}

impl fmt::Display for Name {
    /// The game as its `game` key names it.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Name::Werewolf => "werewolf",
            Name::Avalon => "avalon",
        })
    }
}

pub use free_agent_game::config::CONFIG_FORMAT;

/// Social Deduction, as a configuration file's format check names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocialDeduction;

impl Program for SocialDeduction {
    const NAME: &'static str = "social-deduction";
}

/// A configuration file's `format` key, as `social-deduction` reads it.
pub type Format = free_agent_game::config::Format<SocialDeduction>;

/// The keys [`load`] reads first, every other key ignored.
#[derive(Deserialize)]
struct Which {
    #[serde(default)]
    #[expect(dead_code, reason = "a format is checked as it is read")]
    format: Format,
    game: Name,
}

/// Parse `text`, a configuration file's, as the game its `game` key
/// names, and settle it. A document of a newer [`Format`], one that has no
/// `game` or names no game by it, that holds a setting the game it names does not have, or that
/// does not parse or settle for any other reason, is an error, which
/// `toml` places at its line and column. Parsing it says nothing about
/// the API keys, so a document can be checked without them.
pub fn parse(text: &str) -> anyhow::Result<Settled> {
    let Which { game, .. } = toml::from_str(text)?;
    Ok(match game {
        Name::Werewolf => Settled::Werewolf(Box::new(werewolf::config::parse(text)?.settle()?)),
        Name::Avalon => Settled::Avalon(Box::new(avalon::config::parse(text)?.settle()?)),
    })
}

/// Read the configuration file at `path` once, and [`parse`] it. A file
/// that cannot be read or parsed is an invalid configuration file, and
/// the error names it.
pub fn load(path: &Path) -> anyhow::Result<Settled> {
    std::fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|text| parse(&text))
        .with_context(|| format!("invalid configuration file {}", path.display()))
}

/// A game's configuration file, as written, its `[phases]` table a
/// `Phases` and its `[roles]` tables a `Roles`. Every table may be left
/// out, and an unknown key anywhere is an error, so a misspelled setting
/// fails loudly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config<Phases, Roles> {
    /// The `format` key, checked as it is read.
    #[serde(default)]
    #[expect(dead_code, reason = "a format is checked as it is read")]
    format: Format,
    /// The `game` key, which [`load`] reads first and checks. A file
    /// parsed here may leave it out.
    #[serde(default)]
    #[expect(dead_code, reason = "load reads the game key itself")]
    game: Option<Name>,
    /// The strategy every role not allotted otherwise plays, [`RANDOM`]
    /// unless it says.
    pub strategy: Option<String>,
    /// How long each phase waits for the players.
    #[serde(default)]
    pub phases: Phases,
    /// The models, by name, and how to reach each.
    #[serde(default)]
    pub models: BTreeMap<String, Spec>,
    /// Named pieces of text the templates pull in with `{{ name }}`.
    #[serde(default)]
    pub prompt_parts: BTreeMap<String, String>,
    /// The strategies, by name, besides [`RANDOM`].
    #[serde(default)]
    pub strategies: BTreeMap<String, Strategy>,
    /// How many of each role there are, and which strategies they play.
    #[serde(default)]
    pub roles: Roles,
}

impl<Phases, Roles> Config<Phases, Roles>
where
    Self: DeserializeOwned,
{
    /// Parse `file`, the text of a configuration file, and check what every
    /// game's file holds alike, with the game's template `variables`: a
    /// `[prompt_parts]` part named like one of them, by anything but words of
    /// letters, digits and underscores, or so that it is pulled in like
    /// another, is an error, and so is a
    /// model's context window of 0, a strategy named [`RANDOM`], a file's
    /// `strategy` that is not defined, and a prompted strategy naming a
    /// model the file does not define, or naming none when the file does
    /// not define exactly one. A prompted strategy that names no model, of
    /// a file that defines one, is given that one.
    pub fn parse(file: &str, variables: &[&str]) -> anyhow::Result<Self> {
        let mut config: Self = toml::from_str(file)?;
        check_common(&mut config, variables)?;
        Ok(config)
    }

    /// The configuration `table` holds, as the desktop app's settings
    /// make it, checked as [`Config::parse`] checks a file.
    pub fn from_table(table: toml::Table, variables: &[&str]) -> anyhow::Result<Self> {
        let mut config: Self = table.try_into()?;
        check_common(&mut config, variables)?;
        Ok(config)
    }
}

/// Check what `config` holds alike with every game's file, as
/// [`Config::parse`] says, filling in the model a prompted strategy leaves
/// out.
fn check_common<Phases, Roles>(
    config: &mut Config<Phases, Roles>,
    variables: &[&str],
) -> anyhow::Result<()> {
    let Config {
        prompt_parts,
        models,
        strategy,
        strategies,
        ..
    } = config;
    if let Some(name) = prompt_parts.keys().find(|name| !is_part_name(name)) {
        bail!(
            "[prompt_parts] has a part named {name:?}; a part's name is words of letters, \
             digits and underscores, not starting with a digit"
        );
    }
    let mut pulled = BTreeMap::new();
    for name in prompt_parts.keys() {
        let variable = part_variable(name);
        if variables.contains(&variable.as_str()) {
            bail!("[prompt_parts] has a part named {name:?}, which is a template variable");
        }
        if let Some(other) = pulled.insert(variable.clone(), name) {
            bail!(
                "[prompt_parts] has parts named {other:?} and {name:?}, which are both \
                 pulled in with {{{{ {variable} }}}}"
            );
        }
    }
    if let Some((name, _)) = models
        .iter()
        .find(|(_, model)| model.context_window == Some(0))
    {
        bail!("[models.{name}] context_window is 0; a context window holds at least 1 token");
    }
    if strategies.contains_key(RANDOM) {
        bail!("[strategies.{RANDOM}] is built in, and may not be defined");
    }
    if let Some(name) = strategy
        && name != RANDOM
        && !strategies.contains_key(name)
    {
        bail!("the file's strategy is {name}, which is not a strategy");
    }
    for (name, strategy) in strategies {
        let Strategy::Prompted(prompted) = strategy else {
            continue;
        };
        let model = &prompted.model;
        if model.is_empty() {
            let [only] = models.keys().collect::<Vec<_>>()[..] else {
                bail!(
                    "strategy {name} names no model, which it may leave out only when the file defines exactly one; it defines {}",
                    models.len()
                );
            };
            prompted.model = only.clone();
        } else if !models.contains_key(model) {
            bail!("strategy {name} names the model {model}, which is not defined");
        }
    }
    Ok(())
}

impl<Phases, Roles> Config<Phases, Roles> {
    /// The lineup the file makes of the roles in `counts`, each with its
    /// count in the code, reading what the file says about a role with
    /// `roles`: every strategy, the file's and [`RANDOM`], and each role's
    /// allotment. A role's `strategies` table allots its seats first. With
    /// the role's own `strategy`, that strategy fills the rest of the
    /// role's count, from the file if given there, otherwise from the code,
    /// and a count in the file that is less than the table allots is an
    /// error naming the role and both numbers. Without one, the role has as
    /// many seats as the table allots, and a count in the file that
    /// disagrees is an error naming the role and both numbers. A role with
    /// no table plays one strategy, its own `strategy`, otherwise the
    /// file's, otherwise [`RANDOM`], and has the count from the file if
    /// given there, otherwise from the code. A strategy allotted that is
    /// not defined is an error naming the role. An error names a role's
    /// table by the role's name in lower case, `[roles.servant]`, and the
    /// role as it is written, the Servant.
    pub fn allot<R: Copy + Ord + fmt::Display>(
        &self,
        counts: impl IntoIterator<Item = (R, usize)>,
        roles: impl Fn(&Roles, R) -> &RoleConfig,
    ) -> anyhow::Result<Lineup<R>> {
        let mut strategies = self.strategies.clone();
        strategies.insert(RANDOM.to_string(), Strategy::Random(Random {}));
        let mut allotments = Allotments::new();
        for (role, default) in counts {
            let file = roles(&self.roles, role);
            let key = role.to_string().to_lowercase();
            let allotted = match (&file.strategies, &file.strategy) {
                (Some(table), filling) => {
                    let allots: usize = table.values().sum();
                    if let Some(count) = file.count
                        && (count < allots || filling.is_none() && count != allots)
                    {
                        bail!(
                            "[roles.{key}] count is {count}, but [roles.{key}.strategies] allots {allots}"
                        );
                    }
                    let mut table = table.clone();
                    let left = file.count.unwrap_or(default).saturating_sub(allots);
                    if let Some(filling) = filling
                        && left > 0
                    {
                        *table.entry(filling.clone()).or_default() += left;
                    }
                    table
                }
                (None, _) => {
                    let strategy = file
                        .strategy
                        .as_ref()
                        .or(self.strategy.as_ref())
                        .map_or(RANDOM, String::as_str);
                    let count = file.count.unwrap_or(default);
                    BTreeMap::from([(strategy.to_string(), count)])
                }
            };
            if let Some(name) = allotted.keys().find(|name| !strategies.contains_key(*name)) {
                bail!("the {role} is allotted {name}, which is not a strategy");
            }
            allotments.insert(role, allotted);
        }
        Ok(Lineup {
            strategies,
            allotments,
        })
    }
}

/// How many seats of each role `R` play each strategy, by name.
pub type Allotments<R> = BTreeMap<R, BTreeMap<String, usize>>;

/// Who plays what at a table: every strategy, and how many seats of each
/// role play each.
#[derive(Debug)]
pub struct Lineup<R> {
    /// Every strategy, by name: the file's, and [`RANDOM`].
    pub strategies: BTreeMap<String, Strategy>,
    /// How many seats of each role play each strategy.
    pub allotments: Allotments<R>,
}

/// One player's system prompt, rendered for a seat dealt a role and a
/// prompted strategy.
#[derive(Debug)]
pub struct SystemPrompt<R> {
    /// The seat, `Player 1` onward.
    pub seat: PlayerId,
    /// The role the seat was dealt.
    pub role: R,
    /// The name of the strategy the seat was dealt.
    pub strategy: String,
    /// The rendered prompt.
    pub text: String,
}

impl<R: Copy + Ord + Hash + fmt::Display> Lineup<R> {
    /// Each (role, strategy) pair at the table, in the order the roles are
    /// dealt and then by the strategy's name, with the strategy and how
    /// many seats it is allotted.
    fn at_the_table(&self) -> impl Iterator<Item = (R, &str, &Strategy, usize)> {
        self.allotments.iter().flat_map(move |(role, allotted)| {
            allotted
                .iter()
                .filter(|(_, seats)| **seats > 0)
                .map(move |(name, seats)| (*role, name.as_str(), &self.strategies[name], *seats))
        })
    }

    /// The seats at the table, `Player 1` onward, one for each seat
    /// allotted.
    pub fn seats(&self) -> impl Iterator<Item = PlayerId> + use<R> {
        seats(
            self.allotments.values().flat_map(BTreeMap::values).sum(),
            numbered,
        )
    }

    /// How many seats of each role there are.
    pub fn role_counts(&self) -> BTreeMap<R, usize> {
        let seats =
            |(role, allotted): (&R, &BTreeMap<String, usize>)| (*role, allotted.values().sum());
        self.allotments.iter().map(seats).collect()
    }

    /// Deal the (role, strategy) pairs the allotments make, one per seat,
    /// at random to the seats `Player 1` onward.
    pub fn deal(&self) -> HashMap<PlayerId, Dealt<R>> {
        let mut pairs: Vec<Dealt<R>> = self
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
        self.seats().zip(pairs).collect()
    }

    /// The name of the model the strategy `name` asks, if it is prompted.
    pub fn model_of(&self, name: &str) -> Option<&str> {
        match &self.strategies[name] {
            Strategy::Prompted(prompted) => Some(&prompted.model),
            Strategy::Random(_) => None,
        }
    }

    /// The models in play, by name, each as `models` defines it: those a
    /// prompted strategy at the table asks.
    pub fn models_in_play<'a>(
        &'a self,
        models: &'a BTreeMap<String, Spec>,
    ) -> BTreeMap<&'a str, &'a Spec> {
        self.at_the_table()
            .filter_map(|(_, strategy, _, _)| self.model_of(strategy))
            .map(|name| (name, &models[name]))
            .collect()
    }

    /// Render the system prompt of every seat for every (role, strategy)
    /// pair at the table whose strategy is prompted, since any seat may be
    /// dealt any of them, in seat order and then the order of the pairs.
    /// A prompt is rendered from its strategy's `system` template, with the
    /// variable `name`, the variables `variables` gives for the role, and
    /// every part of `prompt_parts`, by its [`part_variable`]. An undefined variable is an error. A template
    /// that does not compile is an error naming the strategy; one that does
    /// not render, the strategy, the seat and the role.
    pub fn prompts(
        &self,
        prompt_parts: &BTreeMap<String, String>,
        variables: impl Fn(R) -> Value,
    ) -> anyhow::Result<Vec<SystemPrompt<R>>> {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        // Each strategy's template is compiled once, and rendered once a
        // seat for each role it is allotted.
        let mut templates = BTreeMap::new();
        // Each pair's variables, with the prompt parts, are the same for
        // every seat.
        let parts: BTreeMap<String, String> = prompt_parts
            .iter()
            .map(|(name, text)| (part_variable(name), text.clone()))
            .collect();
        let parts = Value::from(parts);
        let mut pairs = Vec::new();
        for (role, name, strategy, _) in self.at_the_table() {
            if let Strategy::Prompted(prompted) = strategy {
                if let btree_map::Entry::Vacant(vacant) = templates.entry(name) {
                    let template = env.template_from_str(&prompted.system).with_context(|| {
                        format!("compiling the prompt template of strategy {name}")
                    })?;
                    vacant.insert(template);
                }
                pairs.push((role, name, merge_maps([variables(role), parts.clone()])));
            }
        }
        let mut prompts = Vec::new();
        for seat in self.seats() {
            for (role, name, told) in &pairs {
                let (role, name) = (*role, *name);
                let text = templates[name]
                    .render(context! { name => &seat, ..told.clone() })
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

    /// The most a request to the model `name` takes of the context window
    /// before its story, by estimate, for any of `prompts` whose strategy
    /// asks it: the most `needed` gives for one of them, or 0 for none.
    pub fn unstoried(
        &self,
        name: &str,
        prompts: &[SystemPrompt<R>],
        needed: impl Fn(&SystemPrompt<R>) -> anyhow::Result<u64>,
    ) -> anyhow::Result<u64> {
        prompts
            .iter()
            .filter(|prompt| self.model_of(&prompt.strategy) == Some(name))
            .try_fold(0, |most, prompt| Ok(most.max(needed(prompt)?)))
    }

    /// Each prompted player's system prompt, by seat, as `deal` dealt it a
    /// role and a strategy, from `prompts`. A prompted player with no
    /// prompt rendered for its role and strategy is an error naming the
    /// seat, the role and the strategy.
    pub fn told(
        &self,
        deal: &HashMap<PlayerId, Dealt<R>>,
        prompts: &[SystemPrompt<R>],
    ) -> anyhow::Result<BTreeMap<PlayerId, String>> {
        let rendered: HashMap<(&PlayerId, R, &str), &str> = prompts
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
        Ok(told)
    }
}

/// The settings that took effect for a game of `G`: the models and the
/// prompt parts as the file defines them, and the limits and the lineup
/// the file and the code decided.
#[derive(Debug)]
pub struct Settings<G: game::Game> {
    /// The models, by name, and how to reach each.
    pub models: BTreeMap<String, Spec>,
    /// The `[prompt_parts]` as written.
    pub prompt_parts: BTreeMap<String, String>,
    /// How long each phase waits for the players.
    pub limits: G::Limits,
    /// Every strategy, and how many seats of each role play each.
    pub lineup: Lineup<G::Role>,
}

impl<G: game::Game> Settings<G> {
    /// The models in play, by name: those a prompted strategy at the table
    /// asks.
    pub fn models_in_play(&self) -> BTreeMap<&str, &Spec> {
        self.lineup.models_in_play(&self.models)
    }

    /// Render every system prompt a game could call for, as
    /// [`Lineup::prompts`] renders it, with the variables the game gives
    /// each role.
    pub fn prompts(&self) -> anyhow::Result<Vec<SystemPrompt<G::Role>>> {
        self.lineup.prompts(&self.prompt_parts, G::variables)
    }

    /// The rules a game is played under, with the limits that took effect.
    pub fn rules(&self) -> G::Rules {
        G::rules(&self.limits)
    }

    /// The most a request to the model `name` takes of the context window
    /// before its story, by estimate, for any of `prompts` whose strategy
    /// asks it, as the game estimates one at this table under these limits.
    pub fn needs(&self, name: &str, prompts: &[SystemPrompt<G::Role>]) -> anyhow::Result<u64> {
        let seats: Vec<PlayerId> = self.lineup.seats().collect();
        let id = &self.models[name].id;
        self.lineup.unstoried(name, prompts, |prompt| {
            let (text, seat, role) = (&prompt.text, &prompt.seat, prompt.role);
            G::unstoried(id, text, seat, role, &seats, &self.limits)
        })
    }

    /// The context window of the model `name`, settled as
    /// [`window::settle`] settles it, against what a request takes before
    /// its story, as [`Settings::needs`] estimates it.
    pub fn context_window(
        &self,
        name: &str,
        reported: Option<u64>,
        prompts: &[SystemPrompt<G::Role>],
    ) -> anyhow::Result<ContextWindow> {
        let needed = self.needs(name, prompts)?;
        window::settle(name, &self.models[name], reported, needed)
    }

    /// Set up each model in play as [`spec::set_up`] sets it up, settling
    /// its context window for `prompts` and asking it to play as the game
    /// probes it, which settles the tool choice, with its key looked up in
    /// `keys`. A model the file defines and no strategy at the table asks
    /// is not contacted.
    pub async fn set_up(
        &self,
        prompts: &[SystemPrompt<G::Role>],
        keys: &dyn Keys,
    ) -> anyhow::Result<(Models, Handles)> {
        let needed = |name: &str| self.needs(name, prompts);
        let probe = async |provider: &Provider, id: &str| {
            G::check_plays(provider, id, ToolChoice::Required).await
        };
        spec::set_up(self.models_in_play(), keys, needed, probe).await
    }

    /// The configuration a game of `deal` runs under, as the log records
    /// it right after the deal: the version playing it, how many of each
    /// role, the limits, every strategy and the allotments, `models`, the
    /// models in play as [`Settings::set_up`] set them up, each prompted
    /// player's system prompt from `prompts` as [`Lineup::told`] tells it,
    /// and the prompt parts as written.
    pub fn configuration(
        &self,
        deal: &HashMap<PlayerId, Dealt<G::Role>>,
        prompts: &[SystemPrompt<G::Role>],
        models: Models,
    ) -> anyhow::Result<Configuration<G>> {
        let lineup = &self.lineup;
        Ok(Configuration {
            version: env!("CARGO_PKG_VERSION").to_string(),
            role_counts: lineup.role_counts(),
            limits: self.limits.clone(),
            strategies: lineup.strategies.clone(),
            allotments: lineup.allotments.clone(),
            models,
            prompts: lineup.told(deal, prompts)?,
            prompt_parts: self.prompt_parts.clone(),
        })
    }
}

/// The configuration a game ran under, as it took effect once the file
/// and the defaults were combined, so that the log says by itself what was
/// played and under which prompts. Each duration is written as `humantime`
/// writes it, such as `"60s"`, and each map in key order. The API key is
/// never recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Configuration<G: game::Game> {
    /// The version of `social-deduction` that played this game, as its
    /// `Cargo.toml` says, so that whatever reads the log knows which shape
    /// it was written in.
    pub version: String,
    /// How many of each role were dealt.
    pub role_counts: BTreeMap<G::Role, usize>,
    /// How long each phase waited for the players.
    #[serde(flatten)]
    pub limits: G::Limits,
    /// Every strategy, by name, [`RANDOM`] included, each prompted one
    /// naming the model it asks.
    pub strategies: BTreeMap<String, Strategy>,
    /// How many seats of each role played each strategy.
    pub allotments: Allotments<G::Role>,
    /// The models in play, by name.
    pub models: Models,
    /// Each prompted player's rendered system prompt.
    pub prompts: BTreeMap<PlayerId, String>,
    /// The `[prompt_parts]` as written.
    pub prompt_parts: BTreeMap<String, String>,
}

/// Each seat's player, as `deal` dealt it a strategy and `configuration`
/// sets the strategy up: a random player, or a prompted player told its
/// system prompt and its game's limits, asking its strategy's model by
/// the model's id, under its tool choice and within its context window,
/// through that model's handle in `handles`.
pub fn players<G: game::Game>(
    configuration: &Configuration<G>,
    deal: &HashMap<PlayerId, Dealt<G::Role>>,
    handles: &Handles,
) -> impl FnMut(&PlayerId) -> game::Seat<G> + use<G> {
    let limits = configuration.limits.clone();
    let mut asking = HashMap::new();
    for (seat, Dealt { strategy, .. }) in deal {
        if let Strategy::Prompted(strategy) = &configuration.strategies[strategy] {
            let name = &strategy.model;
            let model = &configuration.models[name];
            let player = (
                configuration.prompts[seat].clone(),
                model.id.clone(),
                model.tool_choice,
                model.context_window.tokens,
                Arc::clone(&handles[name]),
            );
            asking.insert(seat.clone(), player);
        }
    }
    move |id: &PlayerId| match asking.remove(id) {
        Some((prompt, model_id, tool_choice, context_window, model)) => prompted::Player::init(
            id.clone(),
            prompt,
            model_id,
            tool_choice,
            context_window,
            model,
            limits.clone(),
        ),
        None => random::Player::init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// The example file `name` in `configs/`.
    fn example(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("configs")
            .join(name)
    }

    /// A file holding `text`, written for the test `test` at a path of its
    /// own so the tests do not collide.
    fn written(test: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "social-deduction-{}-{test}.toml",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The error loading `text` from a file of its own gives, causes
    /// included, and the file's path.
    fn refused(test: &str, text: &str) -> (String, PathBuf) {
        let path = written(test, text);
        let error = load(&path).unwrap_err();
        (format!("{error:#}"), path)
    }

    /// Assert that `shown` begins by calling the file at `path` invalid,
    /// and goes on to say `said`.
    fn assert_invalid(shown: &str, path: &Path, said: &str) {
        let invalid = format!("invalid configuration file {}: ", path.display());
        assert!(shown.starts_with(&invalid), "{shown}");
        assert!(shown.contains(said), "{shown}");
    }

    /// Assert that `shown` places the error at a line and a column, as
    /// `toml` does.
    fn assert_placed(shown: &str) {
        assert!(
            shown.contains("line ") && shown.contains("column "),
            "{shown}"
        );
    }

    #[test]
    fn each_example_loads_as_the_game_it_names() {
        for (name, named) in [
            ("werewolf/random.toml", Name::Werewolf),
            ("werewolf/prompted.toml", Name::Werewolf),
            ("avalon/random.toml", Name::Avalon),
            ("avalon/prompted.toml", Name::Avalon),
        ] {
            let game = load(&example(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let loaded = match game {
                Settled::Werewolf(_) => Name::Werewolf,
                Settled::Avalon(_) => Name::Avalon,
            };
            assert_eq!(loaded, named, "{name}");
        }
    }

    #[test]
    fn an_invalid_document_s_error_is_placed_and_names_no_file() {
        let shown = format!("{:#}", parse("game = \"werewolf\"\n[roles\n").unwrap_err());
        assert_placed(&shown);
        assert!(!shown.contains("invalid configuration file"), "{shown}");
    }

    #[test]
    fn every_game_is_named_once_in_order() {
        // A game added to `Name` stops this match compiling until it is
        // given its place here, and so in `Name::ALL`.
        let place = |name| match name {
            Name::Werewolf => 0,
            Name::Avalon => 1,
        };
        assert_eq!(Name::ALL.map(place), [0, 1]);
    }

    #[test]
    fn a_game_is_shown_as_its_key_names_it() {
        for name in Name::ALL {
            let key = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&name).unwrap(), key);
            let read: Name = serde_json::from_str(&key).unwrap();
            assert_eq!(read, name);
        }
    }

    #[test]
    fn a_file_with_no_game_or_one_that_names_no_game_is_invalid_and_placed() {
        for (test, text, said) in [
            ("no-game", "strategy = \"random\"\n", "missing field `game`"),
            (
                "chess",
                "game = \"chess\"\n",
                "unknown variant `chess`, expected `werewolf` or `avalon`",
            ),
            ("number", "game = 7\n", "wanted string or table"),
        ] {
            let (shown, path) = refused(test, text);
            assert_invalid(&shown, &path, said);
            assert_placed(&shown);
        }
    }

    #[test]
    fn a_file_without_a_format_is_format_1() {
        let random = include_str!("../configs/werewolf/random.toml");
        let without = random.replace(&format!("format = {CONFIG_FORMAT}\n"), "");
        assert_ne!(without, random);
        assert_eq!(
            format!("{:?}", parse(&without).unwrap()),
            format!("{:?}", parse(random).unwrap())
        );
    }

    #[test]
    fn a_file_of_a_newer_format_is_refused_at_the_key_s_line_and_column() {
        let (shown, path) = refused("format-2", "game = \"werewolf\"\nformat = 2\n");
        assert_invalid(
            &shown,
            &path,
            "This file is configuration format 2. \
             This version of social-deduction reads formats up to 1.",
        );
        assert!(shown.contains("line 2, column 10"), "{shown}");
        // Read straight into a game's configuration, as the app's settings
        // make it, the format is checked too.
        let table: toml::Table = toml::from_str("format = 2\n").unwrap();
        assert!(werewolf::config::from_table(table).is_err());
    }

    #[test]
    fn a_format_that_is_not_a_format_or_is_out_of_place_is_invalid_and_placed() {
        for (test, text, said) in [
            (
                "format-0",
                "game = \"werewolf\"\nformat = 0\n",
                "formats begin at 1",
            ),
            (
                "format-string",
                "game = \"avalon\"\nformat = \"1\"\n",
                "invalid type: string \"1\", expected u32",
            ),
            (
                "format-in-a-role",
                "game = \"werewolf\"\n[roles.werewolf]\nformat = 1\n",
                "unknown field `format`",
            ),
        ] {
            let (shown, path) = refused(test, text);
            assert_invalid(&shown, &path, said);
            assert_placed(&shown);
        }
    }

    #[test]
    fn every_example_names_its_format() {
        for entry in std::fs::read_dir(example("")).unwrap() {
            for file in std::fs::read_dir(entry.unwrap().path()).unwrap() {
                let path = file.unwrap().path();
                let text = std::fs::read_to_string(&path).unwrap();
                let table: toml::Table = toml::from_str(&text).unwrap();
                assert_eq!(
                    table.get("format"),
                    Some(&toml::Value::Integer(CONFIG_FORMAT.into())),
                    "{path:?}"
                );
            }
        }
    }

    #[test]
    fn a_file_that_is_not_toml_is_invalid_and_placed() {
        let (shown, path) = refused("not-toml", "game = \"werewolf\"\n[roles\n");
        assert_invalid(&shown, &path, "unclosed table, expected `]`");
        assert_placed(&shown);
    }

    #[test]
    fn another_game_s_setting_is_an_unknown_key_placed_at_its_line_and_column() {
        for (test, text, said) in [
            (
                "werewolf-merlin",
                "game = \"werewolf\"\n[roles.merlin]\ncount = 1\n",
                "unknown field `merlin`",
            ),
            (
                "werewolf-vote-limit",
                "game = \"werewolf\"\n[phases]\nvote_limit = \"60s\"\n",
                "unknown field `vote_limit`",
            ),
            (
                "avalon-werewolf",
                "game = \"avalon\"\n[roles.werewolf]\ncount = 1\n",
                "unknown field `werewolf`",
            ),
            (
                "avalon-night-limit",
                "game = \"avalon\"\n[phases]\nnight_limit = \"60s\"\n",
                "unknown field `night_limit`",
            ),
        ] {
            let (shown, path) = refused(test, text);
            assert_invalid(&shown, &path, said);
            assert_placed(&shown);
        }
    }

    #[test]
    fn a_file_that_does_not_settle_is_invalid() {
        let (shown, path) = refused(
            "werewolf-ratify",
            "game = \"werewolf\"\n[phases]\nday_limit = \"10s\"\nratify_limit = \"20s\"\n",
        );
        assert_invalid(
            &shown,
            &path,
            "the ratify limit (20s) is longer than the day limit (10s)",
        );
        let (shown, path) = refused(
            "avalon-no-merlin",
            "game = \"avalon\"\n[roles.merlin]\ncount = 0\n[roles.servant]\ncount = 3\n",
        );
        assert_invalid(&shown, &path, "Merlin exactly when it has an Assassin");
    }

    #[test]
    fn a_file_that_is_not_there_is_invalid_and_named() {
        let path = Path::new("no-such-file.toml");
        let shown = format!("{:#}", load(path).unwrap_err());
        assert_invalid(&shown, path, "No such file");
    }

    /// The roles of a small game the tests play: Knights, two in the
    /// code, and a Knave.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    enum Role {
        Knight,
        Knave,
    }

    impl fmt::Display for Role {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(match self {
                Role::Knight => "Knight",
                Role::Knave => "Knave",
            })
        }
    }

    /// The small game's `[roles]` tables.
    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Roles {
        #[serde(default)]
        knight: RoleConfig,
        #[serde(default)]
        knave: RoleConfig,
    }

    impl Roles {
        /// What the file says about `role`.
        fn get(&self, role: Role) -> &RoleConfig {
            match role {
                Role::Knight => &self.knight,
                Role::Knave => &self.knave,
            }
        }
    }

    /// The small game's `[phases]` table, which holds nothing.
    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Phases {}

    /// The small game's configuration file.
    type Small = Config<Phases, Roles>;

    /// The roles of the small game, with their counts in the code.
    const COUNTS: [(Role, usize); 2] = [(Role::Knight, 2), (Role::Knave, 1)];

    /// `file` parsed as the small game's, with the template variables
    /// `name` and `role`, or the error parsing it gives, causes included.
    fn parsed(file: &str) -> Result<Small, String> {
        Small::parse(file, &["name", "role"]).map_err(|error| format!("{error:#}"))
    }

    /// The lineup `file` allots, or the error parsing or allotting it
    /// gives, causes included.
    fn allotted(file: &str) -> Result<Lineup<Role>, String> {
        let config = parsed(file)?;
        config
            .allot(COUNTS, Roles::get)
            .map_err(|error| format!("{error:#}"))
    }

    /// The lineup `file` allots.
    fn lineup(file: &str) -> Lineup<Role> {
        allotted(file).unwrap()
    }

    /// The prompts `file` renders, `role` the role's name, or the error
    /// rendering them gives, causes included.
    fn rendered(file: &str) -> Result<Vec<SystemPrompt<Role>>, String> {
        let config = parsed(file)?;
        let lineup = config.allot(COUNTS, Roles::get).unwrap();
        lineup
            .prompts(
                &config.prompt_parts,
                |role| context! { role => role.to_string() },
            )
            .map_err(|error| format!("{error:#}"))
    }

    /// An allotment of `seats` to each named strategy.
    fn seats_of(seats: &[(&str, usize)]) -> BTreeMap<String, usize> {
        seats
            .iter()
            .map(|(name, seats)| (name.to_string(), *seats))
            .collect()
    }

    /// One model, `qwen`, and one prompted strategy, `plain`, naming none.
    const PLAIN: &str = r#"
[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
[strategies.plain]
policy = "prompted"
system = "{{ name }} the {{ role }}"
"#;

    /// [`PLAIN`], and two random strategies, `dice` and `coin`.
    fn three() -> String {
        format!(
            "{PLAIN}[strategies.dice]\npolicy = \"random\"\n[strategies.coin]\npolicy = \"random\"\n"
        )
    }

    /// Whether `plain`, of `file`'s strategies, is prompted and asks
    /// `model`.
    fn plain_asks(file: &str, model: &str) -> bool {
        matches!(&parsed(file).unwrap().strategies["plain"], Strategy::Prompted(prompted) if prompted.model == model)
    }

    #[test]
    fn with_nothing_in_the_file_every_role_plays_the_built_in_random_strategy() {
        let lineup = lineup("");
        assert_eq!(
            lineup.allotments,
            Allotments::from([
                (Role::Knight, seats_of(&[(RANDOM, 2)])),
                (Role::Knave, seats_of(&[(RANDOM, 1)])),
            ])
        );
        assert_eq!(
            lineup.strategies,
            BTreeMap::from([(RANDOM.to_string(), Strategy::Random(Random {}))])
        );
    }

    #[test]
    fn the_random_strategy_is_built_in_and_may_not_be_defined() {
        assert_eq!(
            parsed("[strategies.random]\npolicy = \"random\"\n").unwrap_err(),
            "[strategies.random] is built in, and may not be defined"
        );
    }

    #[test]
    fn a_context_window_of_0_is_an_error_naming_the_model() {
        let file = PLAIN.replace("id = ", "context_window = 0\nid = ");
        assert_eq!(
            parsed(&file).unwrap_err(),
            "[models.qwen] context_window is 0; a context window holds at least 1 token"
        );
    }

    #[test]
    fn a_prompt_part_named_like_a_variable_is_an_error_naming_it() {
        for variable in ["name", "role"] {
            let file = format!("{PLAIN}[prompt_parts]\n{variable} = \"Taken.\"\n");
            assert_eq!(
                parsed(&file).unwrap_err(),
                format!(
                    "[prompt_parts] has a part named \"{variable}\", which is a template variable"
                )
            );
        }
    }

    #[test]
    fn a_prompt_part_named_but_by_letters_digits_and_underscores_is_an_error() {
        let file = format!("{PLAIN}[prompt_parts]\n\"the-rules\" = \"Play.\"\n");
        assert_eq!(
            parsed(&file).unwrap_err(),
            "[prompt_parts] has a part named \"the-rules\"; a part's name is words of letters, \
             digits and underscores, not starting with a digit"
        );
    }

    #[test]
    fn a_prompt_part_pulled_in_like_another_is_an_error() {
        let file = format!("{PLAIN}[prompt_parts]\n\"Seer only\" = \"A.\"\nSeer_only = \"B.\"\n");
        assert_eq!(
            parsed(&file).unwrap_err(),
            "[prompt_parts] has parts named \"Seer only\" and \"Seer_only\", which are both \
             pulled in with {{ Seer_only }}"
        );
    }

    #[test]
    fn a_strategy_of_either_policy_parses_and_is_written_with_its_model() {
        let config = parsed(&three()).unwrap();
        assert_eq!(config.strategies["dice"], Strategy::Random(Random {}));
        assert_eq!(
            serde_json::to_value(&config.strategies).unwrap(),
            json!({
                "coin": {"policy": "random"},
                "dice": {"policy": "random"},
                "plain": {
                    "policy": "prompted",
                    "system": "{{ name }} the {{ role }}",
                    "model": "qwen",
                },
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
            assert!(
                parsed(&format!("{PLAIN}[strategies.odd]\n{strategy}")).is_err(),
                "{strategy}"
            );
        }
        let shown =
            parsed(&format!("{PLAIN}[strategies.odd]\npolicy = \"prompted\"\n")).unwrap_err();
        assert!(shown.contains("system"), "{shown}");
    }

    #[test]
    fn a_prompted_strategy_may_leave_its_model_out_only_when_the_file_defines_one() {
        assert!(plain_asks(PLAIN, "qwen"));
        let two = format!(
            "{PLAIN}[models.claude]\nbase_url = \"https://api.anthropic.com/v1\"\nid = \"claude\"\n"
        );
        assert_eq!(
            parsed(&two).unwrap_err(),
            "strategy plain names no model, which it may leave out only when the file defines exactly one; it defines 2"
        );
        let none = "[strategies.plain]\npolicy = \"prompted\"\nsystem = \"Hi.\"\n";
        let shown = parsed(none).unwrap_err();
        assert!(shown.ends_with("it defines 0"), "{shown}");
        // Named, either may be asked.
        let named = two.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"claude\"",
        );
        assert!(plain_asks(&named, "claude"));
    }

    #[test]
    fn a_model_named_empty_or_not_defined_is_an_error_naming_the_strategy() {
        let naming = |model: &str| {
            PLAIN.replace(
                "policy = \"prompted\"",
                &format!("policy = \"prompted\"\nmodel = \"{model}\""),
            )
        };
        let shown = parsed(&naming("")).unwrap_err();
        assert!(
            shown.contains("[strategies.plain]") && shown.contains("model is empty"),
            "{shown}"
        );
        assert_eq!(
            parsed(&naming("claude")).unwrap_err(),
            "strategy plain names the model claude, which is not defined"
        );
    }

    #[test]
    fn the_file_s_strategy_is_random_or_one_it_defines_even_when_every_role_is_allotted_otherwise()
    {
        for strategy in [RANDOM, "plain"] {
            lineup(&format!("strategy = \"{strategy}\"\n{PLAIN}"));
        }
        let file = format!(
            "strategy = \"cautious\"\n{PLAIN}[roles.knight]\nstrategy = \"plain\"\n[roles.knave]\nstrategy = \"plain\"\n"
        );
        assert_eq!(
            parsed(&file).unwrap_err(),
            "the file's strategy is cautious, which is not a strategy"
        );
    }

    #[test]
    fn a_role_is_allotted_by_its_strategies_its_strategy_or_the_file_s_in_that_order() {
        let file = format!(
            "strategy = \"coin\"\n{}[roles.knight]\nstrategy = \"plain\"\n[roles.knight.strategies]\ndice = 1\ncoin = 2\n",
            three()
        );
        assert_eq!(
            lineup(&file).allotments,
            Allotments::from([
                (Role::Knight, seats_of(&[("coin", 2), ("dice", 1)])),
                (Role::Knave, seats_of(&[("coin", 1)])),
            ])
        );
        // A role's own strategy over the file's.
        let file = format!(
            "strategy = \"coin\"\n{}[roles.knave]\nstrategy = \"dice\"\n",
            three()
        );
        assert_eq!(
            lineup(&file).allotments[&Role::Knave],
            seats_of(&[("dice", 1)])
        );
    }

    #[test]
    fn a_role_s_strategy_fills_the_seats_its_strategies_table_leaves() {
        let file = |count: &str| {
            format!(
                "{}[roles.knight]\n{count}strategy = \"dice\"\n[roles.knight.strategies]\ncoin = 1\n",
                three()
            )
        };
        let knights = |file: &str| lineup(file).allotments[&Role::Knight].clone();
        assert_eq!(
            knights(&file("count = 5\n")),
            seats_of(&[("coin", 1), ("dice", 4)])
        );
        // The code's count without the file's.
        assert_eq!(knights(&file("")), seats_of(&[("coin", 1), ("dice", 1)]));
        // A count the table fills leaves nothing to fill.
        let full = file("count = 2\n").replace("coin = 1", "coin = 2");
        assert_eq!(knights(&full), seats_of(&[("coin", 2)]));
        // A strategy in the table and filling adds up.
        let both = file("count = 4\n").replace("coin = 1", "coin = 1\ndice = 1");
        assert_eq!(knights(&both), seats_of(&[("coin", 1), ("dice", 3)]));
        // A count the table overfills is an error naming both numbers.
        assert_eq!(
            allotted(&file("count = 0\n")).unwrap_err(),
            "[roles.knight] count is 0, but [roles.knight.strategies] allots 1"
        );
        // Without a count, a table that allots more than the code's count
        // has as many seats as it allots.
        let more = file("").replace("coin = 1", "coin = 4");
        assert_eq!(knights(&more), seats_of(&[("coin", 4)]));
    }

    #[test]
    fn a_strategies_table_s_seats_are_the_role_s_count_and_a_count_that_disagrees_is_an_error() {
        let file = format!("{}[roles.knave.strategies]\ndice = 1\ncoin = 1\n", three());
        let agreeing = format!("{file}[roles.knave]\ncount = 2\n");
        assert_eq!(lineup(&agreeing).seats().count(), 4);
        let disagreeing = format!("{file}[roles.knave]\ncount = 1\n");
        assert_eq!(
            allotted(&disagreeing).unwrap_err(),
            "[roles.knave] count is 1, but [roles.knave.strategies] allots 2"
        );
    }

    #[test]
    fn a_strategy_allotted_that_is_not_defined_is_an_error_naming_the_role_as_written() {
        for roles in [
            "[roles.knave.strategies]\nwild = 1\n",
            "[roles.knave]\nstrategy = \"wild\"\n",
            "[roles.knave]\ncount = 3\nstrategy = \"wild\"\n[roles.knave.strategies]\ndice = 1\n",
        ] {
            assert_eq!(
                allotted(&format!("{}{roles}", three())).unwrap_err(),
                "the Knave is allotted wild, which is not a strategy"
            );
        }
        // A strategy defined and never allotted is no error.
        lineup(&three());
    }

    /// A table of two Knights, one random and one `plain`, and a Knave
    /// playing `dice`.
    fn mixed() -> Lineup<Role> {
        lineup(&format!(
            "{}[roles.knight.strategies]\nrandom = 1\nplain = 1\n[roles.knave]\nstrategy = \"dice\"\n",
            three()
        ))
    }

    #[test]
    fn every_pair_is_dealt_as_many_times_as_allotted_to_the_seats_from_player_1() {
        let deal = mixed().deal();
        let mut seats: Vec<&PlayerId> = deal.keys().collect();
        seats.sort();
        assert_eq!(seats, ["Player 1", "Player 2", "Player 3"]);
        let mut pairs: Vec<(Role, &str)> = deal
            .values()
            .map(|dealt| (dealt.role, dealt.strategy.as_str()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            [
                (Role::Knight, "plain"),
                (Role::Knight, RANDOM),
                (Role::Knave, "dice"),
            ]
        );
    }

    #[test]
    fn the_deal_is_shuffled_in_roles_and_in_strategies() {
        let lineup = mixed();
        let deals: Vec<HashMap<PlayerId, Dealt<Role>>> = (0..100).map(|_| lineup.deal()).collect();
        // Every seat is dealt the plain Knight in some deal, and the Knave
        // in another.
        for strategy in ["plain", "dice"] {
            let seats: HashSet<&PlayerId> = deals
                .iter()
                .flat_map(|deal| deal.iter())
                .filter(|(_, dealt)| dealt.strategy == strategy)
                .map(|(seat, _)| seat)
                .collect();
            assert_eq!(seats.len(), 3, "{strategy}: {seats:?}");
        }
    }

    /// The prompt `prompts` holds for `seat` dealt `role` and `strategy`.
    fn prompt<'a>(
        prompts: &'a [SystemPrompt<Role>],
        seat: &str,
        role: Role,
        strategy: &str,
    ) -> &'a str {
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
        let file = format!(
            "{PLAIN}[strategies.bold]\npolicy = \"prompted\"\nsystem = \"Bold {{{{ name }}}}.\"\n\
             [roles.knight]\nstrategy = \"plain\"\n[roles.knave]\nstrategy = \"bold\"\n"
        );
        let prompts = rendered(&file).unwrap();
        let pairs: Vec<(&str, Role, &str)> = prompts
            .iter()
            .map(|prompt| (prompt.seat.as_str(), prompt.role, prompt.strategy.as_str()))
            .collect();
        let expected: Vec<(&str, Role, &str)> = ["Player 1", "Player 2", "Player 3"]
            .into_iter()
            .flat_map(|seat| [(seat, Role::Knight, "plain"), (seat, Role::Knave, "bold")])
            .collect();
        assert_eq!(pairs, expected);
        assert_eq!(
            prompt(&prompts, "Player 2", Role::Knave, "bold"),
            "Bold Player 2."
        );
        assert_eq!(
            prompt(&prompts, "Player 3", Role::Knight, "plain"),
            "Player 3 the Knight"
        );
        // A table with no prompted strategy renders nothing.
        assert!(rendered(&three()).unwrap().is_empty());
    }

    #[test]
    fn every_prompt_part_is_a_variable_and_a_persona_is_not() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{{ rules }} {{ the_oath }}");
        let file = format!(
            "strategy = \"plain\"\n{file}[prompt_parts]\nrules = \"The rules.\"\n\"the oath\" = \"Serve.\"\n"
        );
        let prompts = rendered(&file).unwrap();
        assert_eq!(
            prompt(&prompts, "Player 1", Role::Knave, "plain"),
            "The rules. Serve."
        );
        let file = format!("strategy = \"plain\"\n{PLAIN}")
            .replace("{{ role }}", "{{ role }}[{{ persona }}]");
        let shown = rendered(&file).unwrap_err();
        assert!(shown.contains("persona"), "{shown}");
    }

    #[test]
    fn a_template_that_does_not_render_is_an_error_naming_the_strategy_the_seat_and_the_role() {
        let file = format!("strategy = \"plain\"\n{PLAIN}")
            .replace("{{ name }} the {{ role }}", "{{ rulez }}");
        let shown = rendered(&file).unwrap_err();
        assert!(
            shown.starts_with("rendering the prompt of strategy plain for Player 1 as the Knight"),
            "{shown}"
        );
        assert!(shown.contains("rulez"), "{shown}");
    }

    #[test]
    fn a_template_that_does_not_compile_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{% if %}");
        let shown = rendered(&format!("strategy = \"plain\"\n{file}")).unwrap_err();
        assert!(
            shown.starts_with("compiling the prompt template of strategy plain"),
            "{shown}"
        );
        assert!(shown.contains("syntax"), "{shown}");
        // A strategy at no seat of the table is not compiled.
        assert!(rendered(&file).unwrap().is_empty());
    }

    /// A file of two models, `qwen` and `claude`, and a strategy asking
    /// each, `plain` and `smart`, everyone playing `plain`, with `rest`
    /// after the strategies.
    fn two_models(rest: &str) -> String {
        format!(
            "strategy = \"plain\"
{PLAIN}
[models.claude]
base_url = \"https://api.anthropic.com/v1\"
id = \"claude-sonnet-4-5\"
[strategies.smart]
policy = \"prompted\"
model = \"claude\"
system = \"Smart {{{{ name }}}}.\"
{rest}"
        )
        .replace(
            "system = \"{{ name }} the",
            "model = \"qwen\"\nsystem = \"{{ name }} the",
        )
    }

    #[test]
    fn a_model_is_in_play_when_a_strategy_at_the_table_asks_it() {
        let in_play = |file: &str| -> Vec<String> {
            let config = parsed(file).unwrap();
            let lineup = config.allot(COUNTS, Roles::get).unwrap();
            let in_play = lineup.models_in_play(&config.models);
            in_play.into_keys().map(str::to_string).collect()
        };
        assert_eq!(in_play(&two_models("")), ["qwen"]);
        let both = two_models("[roles.knave]\nstrategy = \"smart\"\n");
        assert_eq!(in_play(&both), ["claude", "qwen"]);
        // Allotted no seats, a strategy puts no model in play.
        let unseated = two_models("[roles.knave]\ncount = 0\nstrategy = \"smart\"\n");
        assert_eq!(in_play(&unseated), ["qwen"]);
        // Nor does a random table.
        assert!(in_play(&three()).is_empty());
    }

    #[test]
    fn each_model_s_need_is_the_most_of_the_prompts_of_the_strategies_asking_it_alone() {
        let file = two_models("[roles.knave]\nstrategy = \"smart\"\n");
        let prompts = rendered(&file).unwrap();
        let lineup = lineup(&file);
        let length = |prompt: &SystemPrompt<Role>| Ok(prompt.text.len() as u64);
        // "Player 1 the Knight" asks qwen; "Smart Player 1." asks claude.
        assert_eq!(lineup.unstoried("qwen", &prompts, length).unwrap(), 19);
        assert_eq!(lineup.unstoried("claude", &prompts, length).unwrap(), 15);
        assert_eq!(lineup.unstoried("idle", &prompts, length).unwrap(), 0);
        let failing = |_: &SystemPrompt<Role>| Err(anyhow!("no estimate"));
        let error = lineup.unstoried("qwen", &prompts, failing).unwrap_err();
        assert_eq!(error.to_string(), "no estimate");
    }

    #[test]
    fn each_prompted_player_is_told_its_prompt_and_one_with_none_rendered_is_an_error() {
        let file = two_models("[roles.knave]\nstrategy = \"smart\"\n");
        let prompts = rendered(&file).unwrap();
        let lineup = lineup(&file);
        let dealt = |role, strategy: &str| Dealt {
            role,
            strategy: strategy.to_string(),
        };
        let mut deal = HashMap::from([
            ("Player 1".to_string(), dealt(Role::Knave, "smart")),
            ("Player 2".to_string(), dealt(Role::Knight, "plain")),
            ("Player 3".to_string(), dealt(Role::Knight, RANDOM)),
        ]);
        assert_eq!(
            lineup.told(&deal, &prompts).unwrap(),
            BTreeMap::from([
                ("Player 1".to_string(), "Smart Player 1.".to_string()),
                ("Player 2".to_string(), "Player 2 the Knight".to_string()),
            ])
        );
        deal.insert("Player 3".to_string(), dealt(Role::Knave, "plain"));
        assert_eq!(
            lineup.told(&deal, &prompts).unwrap_err().to_string(),
            "no system prompt rendered for Player 3 as the Knave playing plain"
        );
    }
}
