//! The configuration of a game of Avalon: the [`Config`] a TOML file is
//! parsed into, and the [`Settings`] that take effect once it is combined
//! with the defaults in the code. A setting comes from the file if it is
//! given there, otherwise from the code.
//!
//! The file defines named models, each a [`Spec`], and named
//! [`Strategy`]s, each a policy with its parameters, and allots each role's
//! seats among the strategies. The strategy [`RANDOM`] is always defined.
//! [`Settings::deal`] deals the (role, strategy) pairs the allotments make
//! to the seats at random.
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
//! the environment variable that holds it, and [`Spec::api_key`] reads
//! that variable into a [`SecretString`](secrecy::SecretString), which is redacted wherever it is
//! shown.
//!
//! The log records the [`Configuration`] a game ran under right after the
//! deal, and [`players`] builds each seat's player from it.

use super::{Dealt, Role, Rules, Seat, Table, prompted, random};
use crate::model::Provider;
use crate::model::spec::{self, Handles, Models, Spec};
use crate::model::window::{self, ContextWindow};
use crate::seat::PlayerId;
use crate::strategy::{RANDOM, Random, RoleConfig, Strategy};
use anyhow::{Context as _, anyhow, bail};
use minijinja::{Environment, UndefinedBehavior, Value, context};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, btree_map};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The variables every template has besides the text blocks.
const VARIABLES: [&str; 3] = ["name", "role", "team"];

/// The configuration file, as written. Every table may be left out, and an
/// unknown key anywhere is an error, so a misspelled setting fails loudly.
#[derive(Debug, Deserialize)]
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
    pub models: BTreeMap<String, Spec>,
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

/// The text of the configuration file at `path`.
fn read(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// The `[phases]` table: how long each phase waits for the players, for
/// those that say.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phases {
    /// How long a proposal waits for its leader.
    #[serde(default, with = "humantime_serde")]
    pub propose_limit: Option<Duration>,
    /// How long a vote waits for the players.
    #[serde(default, with = "humantime_serde")]
    pub vote_limit: Option<Duration>,
    /// How long a quest waits for its team.
    #[serde(default, with = "humantime_serde")]
    pub quest_limit: Option<Duration>,
    /// How long the assassination waits for the Assassin.
    #[serde(default, with = "humantime_serde")]
    pub assassinate_limit: Option<Duration>,
}

/// The `[roles]` tables, one per role. A role the file does not mention
/// has nothing said about it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    /// `[roles.merlin]`.
    #[serde(default)]
    pub merlin: RoleConfig,
    /// `[roles.percival]`.
    #[serde(default)]
    pub percival: RoleConfig,
    /// `[roles.servant]`.
    #[serde(default)]
    pub servant: RoleConfig,
    /// `[roles.assassin]`.
    #[serde(default)]
    pub assassin: RoleConfig,
    /// `[roles.morgana]`.
    #[serde(default)]
    pub morgana: RoleConfig,
    /// `[roles.mordred]`.
    #[serde(default)]
    pub mordred: RoleConfig,
    /// `[roles.oberon]`.
    #[serde(default)]
    pub oberon: RoleConfig,
    /// `[roles.minion]`.
    #[serde(default)]
    pub minion: RoleConfig,
}

impl Roles {
    /// What the file says about `role`.
    pub fn get(&self, role: Role) -> &RoleConfig {
        match role {
            Role::Merlin => &self.merlin,
            Role::Percival => &self.percival,
            Role::Servant => &self.servant,
            Role::Assassin => &self.assassin,
            Role::Morgana => &self.morgana,
            Role::Mordred => &self.mordred,
            Role::Oberon => &self.oberon,
            Role::Minion => &self.minion,
        }
    }
}

/// The name of the table `[roles.<name>]` that speaks of `role`: the
/// role's name in lower case, such as `servant`.
fn key(role: Role) -> String {
    role.to_string().to_lowercase()
}

/// How many seats of each role play each strategy, by name.
pub type Allotments = BTreeMap<Role, BTreeMap<String, usize>>;

/// The settings that took effect: the file as written, and the settings
/// the file and the code have a say in, decided.
#[derive(Debug)]
pub struct Settings {
    /// The file as written.
    pub config: Config,
    /// How many of each role sit at the table.
    pub table: Table,
    /// The rules, with the limits that took effect.
    pub rules: Rules,
    /// Every strategy, by name: the file's, and [`RANDOM`].
    pub strategies: BTreeMap<String, Strategy>,
    /// How many seats of each role play each strategy.
    pub allotments: Allotments,
}

impl Config {
    /// Read and parse the file at `path`.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&read(path)?).with_context(|| format!("in {}", path.display()))
    }

    /// Parse `file`, the text of a configuration file. A `[text]` block
    /// named like one of the template variables is an error, since the
    /// templates could not tell them apart, and so is a model's context
    /// window of 0, a strategy named [`RANDOM`], a file's `strategy` that
    /// is not defined, and a prompted strategy naming a model the file does
    /// not define, or naming none when the file does not define exactly
    /// one.
    pub fn parse(file: &str) -> anyhow::Result<Self> {
        let mut config: Self = toml::from_str(file)?;
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
        if let Some(name) = &config.strategy
            && name != RANDOM
            && !config.strategies.contains_key(name)
        {
            bail!("the file's strategy is {name}, which is not a strategy");
        }
        for (name, strategy) in &mut config.strategies {
            let Strategy::Prompted(prompted) = strategy else {
                continue;
            };
            let model = &prompted.model;
            if model.is_empty() {
                let [only] = config.models.keys().collect::<Vec<_>>()[..] else {
                    bail!(
                        "strategy {name} names no model, which it may leave out only when the file defines exactly one; it defines {}",
                        config.models.len()
                    );
                };
                prompted.model = only.clone();
            } else if !config.models.contains_key(model) {
                bail!("strategy {name} names the model {model}, which is not defined");
            }
        }
        Ok(config)
    }

    /// The settings that take effect: each limit from the file if given
    /// there, otherwise from the code; and each role's allotment. A role's
    /// `strategies` table allots its seats first. With the role's own
    /// `strategy`, that strategy fills the rest of the role's count, from
    /// the file if given there, otherwise from the code, and a count in the
    /// file that is less than the table allots is an error naming the role
    /// and both numbers. Without one, the role has as many seats as the
    /// table allots, and a count in the file that disagrees is an error
    /// naming the role and both numbers. A role with no table plays one
    /// strategy, its own `strategy`, otherwise the file's, otherwise
    /// [`RANDOM`], and has the count from the file if given there,
    /// otherwise from the code. A strategy allotted that is not defined is
    /// an error naming the role. The table the counts make is then checked
    /// as [`Table::check`] checks it, and a prompted strategy allotted a
    /// seat is an error naming the strategy and the role.
    pub fn settle(self) -> anyhow::Result<Settings> {
        let (code, file) = (Rules::default(), &self.phases);
        let rules = Rules {
            propose_limit: file.propose_limit.unwrap_or(code.propose_limit),
            vote_limit: file.vote_limit.unwrap_or(code.vote_limit),
            quest_limit: file.quest_limit.unwrap_or(code.quest_limit),
            assassinate_limit: file.assassinate_limit.unwrap_or(code.assassinate_limit),
        };
        let mut strategies = self.strategies.clone();
        strategies.insert(RANDOM.to_string(), Strategy::Random(Random {}));
        let mut allotments = Allotments::new();
        for (role, default) in Table::default().counts() {
            let file = self.roles.get(role);
            let key = key(role);
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
        let seats = |role| allotments[&role].values().sum();
        let table = Table {
            merlin: seats(Role::Merlin),
            percival: seats(Role::Percival),
            servants: seats(Role::Servant),
            assassin: seats(Role::Assassin),
            morgana: seats(Role::Morgana),
            mordred: seats(Role::Mordred),
            oberon: seats(Role::Oberon),
            minions: seats(Role::Minion),
        };
        table.check()?;
        Ok(Settings {
            table,
            rules,
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
    /// The seat, `Player 1` onward.
    pub seat: PlayerId,
    /// The role the seat was dealt.
    pub role: Role,
    /// The name of the strategy the seat was dealt.
    pub strategy: String,
    /// The rendered prompt.
    pub text: String,
}

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
    /// at random to the seats `Player 1` onward.
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
            Strategy::Prompted(prompted) => Some(&prompted.model),
            Strategy::Random(_) => None,
        }
    }

    /// The models in play, by name: those a prompted strategy at the table
    /// asks.
    pub fn models_in_play(&self) -> BTreeMap<&str, &Spec> {
        self.at_the_table()
            .filter_map(|(_, strategy, _, _)| self.model_of(strategy))
            .map(|name| (name, &self.config.models[name]))
            .collect()
    }

    /// Render the system prompt of every seat for every (role, strategy)
    /// pair at the table whose strategy is prompted, since any seat may be
    /// dealt any of them, in seat order and then the order of the pairs.
    /// A prompt is rendered from its strategy's `system` template, with the
    /// variables `name`, `role`, as [`Role::told`] tells it, `team`,
    /// `good` or `evil`, and every `[text]` block. An undefined variable is an error. A template that
    /// does not compile is an error naming the strategy; one that does not
    /// render, the strategy, the seat and the role.
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
                    .render(context! {
                        name => &seat,
                        role => role.told(),
                        team => role.team().to_string(),
                        ..blocks.clone()
                    })
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

    /// The context window of the model `name`, settled as
    /// [`window::settle`] settles it, against what a request takes before
    /// its story, by estimate, for the largest of `prompts` whose strategy
    /// asks the model.
    pub fn context_window(
        &self,
        name: &str,
        reported: Option<u64>,
        prompts: &[SystemPrompt],
    ) -> anyhow::Result<ContextWindow> {
        let needed = self.unstoried(name, prompts);
        window::settle(name, &self.config.models[name], reported, needed)
    }

    /// The most a request to the model `name` takes of the context window
    /// before its story, by estimate, for any of `prompts` whose strategy
    /// asks it.
    fn unstoried(&self, name: &str, prompts: &[SystemPrompt]) -> u64 {
        let seats: Vec<PlayerId> = self.table.seats().collect();
        let id = &self.config.models[name].id;
        prompts
            .iter()
            .filter(|prompt| self.model_of(&prompt.strategy) == Some(name))
            .map(|prompt| {
                let (text, seat, role) = (&prompt.text, &prompt.seat, prompt.role);
                prompted::unstoried(id, text, seat, role, &seats, &self.rules)
            })
            .max()
            .unwrap_or_default()
    }

    /// Set up each model in play as [`spec::set_up`] sets it up, settling
    /// its context window for `prompts` and asking it to play as the player
    /// each phase waits on, which settles the tool choice. A model the file
    /// defines and no strategy at the table asks is not contacted.
    pub async fn set_up(&self, prompts: &[SystemPrompt]) -> anyhow::Result<(Models, Handles)> {
        let needed = |name: &str| Ok(self.unstoried(name, prompts));
        let probe = async |provider: &Provider, id: &str| prompted::check_plays(provider, id).await;
        spec::set_up(self.models_in_play(), needed, probe).await
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
            propose_limit: self.rules.propose_limit,
            vote_limit: self.rules.vote_limit,
            quest_limit: self.rules.quest_limit,
            assassinate_limit: self.rules.assassinate_limit,
            strategies: self.strategies.clone(),
            allotments: self.allotments.clone(),
            models,
            prompts: told,
            text: self.config.text.clone(),
        })
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
    /// How long a proposal waited for its leader.
    #[serde(with = "humantime_serde")]
    pub propose_limit: Duration,
    /// How long a vote waited for the players.
    #[serde(with = "humantime_serde")]
    pub vote_limit: Duration,
    /// How long a quest waited for its team.
    #[serde(with = "humantime_serde")]
    pub quest_limit: Duration,
    /// How long the assassination waited for the Assassin.
    #[serde(with = "humantime_serde")]
    pub assassinate_limit: Duration,
    /// Every strategy, by name, [`RANDOM`] included, each prompted one
    /// naming the model it asks.
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
            let name = &strategy.model;
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
    use crate::avalon::report::Narrator;
    use crate::avalon::tests::{logged, run};
    use crate::avalon::{Entry, Message, Team, game};
    use crate::model::canned::{self, playing};
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::model::spec::ModelInPlay;
    use crate::model::window::Source;
    use crate::model::{self, CONTEXT_WINDOW, REQUEST_TIMEOUT, ToolChoice};
    use crate::strategy::Prompted;
    use serde_json::json;
    use std::collections::HashSet;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    /// A file defining one model, `qwen`, one prompted strategy asking it,
    /// `plain`, allotted to nobody, and two random strategies, `dice` and
    /// `coin`.
    const STRATEGIES: &str = r#"
[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
[strategies.plain]
policy = "prompted"
system = "{{ name }} the {{ role }}"
[strategies.dice]
policy = "random"
[strategies.coin]
policy = "random"
"#;

    /// The settings `file` produces under the code defaults.
    fn settle(file: &str) -> Settings {
        Config::parse(file).unwrap().settle().unwrap()
    }

    /// The error `file` settles to, causes included.
    fn unsettled(file: &str) -> String {
        let error = Config::parse(file).and_then(Config::settle).unwrap_err();
        format!("{error:#}")
    }

    /// An allotment of `seats` to each named strategy.
    fn allotted(seats: &[(&str, usize)]) -> BTreeMap<String, usize> {
        seats
            .iter()
            .map(|(name, seats)| (name.to_string(), *seats))
            .collect()
    }

    /// The example file `name`.
    fn example(name: &str) -> String {
        format!("{}/configs/avalon/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn the_configs_parse_and_render_and_are_the_two_documented() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/configs/avalon");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["prompted.toml", "random.toml"]);
        for name in &names {
            let settings = Config::load(Path::new(&example(name)))
                .unwrap()
                .settle()
                .unwrap();
            assert_eq!(settings.table, Table::default(), "{name}");
            settings.prompts().unwrap();
        }
        // The prompted example tells every player the rules, that it may
        // talk in every phase, and who it is.
        let settings = Config::load(Path::new(&example("prompted.toml")))
            .unwrap()
            .settle()
            .unwrap();
        let prompts = settings.prompts().unwrap();
        let in_play: Vec<&str> = settings.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["qwen"]);
        let rules = &settings.config.text["rules"];
        for said in ["every phase", "speak", "wait"] {
            assert!(rules.contains(said), "{said}: {rules}");
        }
        // They explain every role, since a table may seat any of them.
        for (role, _) in Table::default().counts() {
            let named = role
                .told()
                .trim_start_matches("a ")
                .trim_start_matches("the ");
            assert!(rules.contains(named), "{named}: {rules}");
        }
        let merlin = prompt(&prompts, "Player 3", Role::Merlin, "everyone");
        assert!(merlin.starts_with(rules.as_str()), "{merlin}");
        assert!(
            merlin.ends_with("\n\nYou are Player 3, Merlin, on the good side."),
            "{merlin}"
        );
        let servant = prompt(&prompts, "Player 1", Role::Servant, "everyone");
        assert!(
            servant.ends_with("\n\nYou are Player 1, a loyal servant of Arthur, on the good side."),
            "{servant}"
        );
    }

    #[test]
    fn the_random_config_plays_the_code_defaults_with_no_models() {
        let example = Config::load(Path::new(&example("random.toml")))
            .unwrap()
            .settle()
            .unwrap();
        let none = Config::parse("").unwrap().settle().unwrap();
        let configured = |settings: &Settings| {
            let deal = settings.deal();
            settings.configuration(&deal, &[], Models::new()).unwrap()
        };
        assert_eq!(configured(&example), configured(&none));
        assert!(example.models_in_play().is_empty());
        assert!(example.prompts().unwrap().is_empty());
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
    fn with_nothing_in_the_file_every_role_of_the_default_table_is_random() {
        let settings = settle("");
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
        assert_eq!(settings.rules, Rules::default());
    }

    #[test]
    fn a_strategy_of_either_policy_parses_and_is_written_with_its_model() {
        let config = Config::parse(STRATEGIES).unwrap();
        assert_eq!(config.strategies["dice"], Strategy::Random(Random {}));
        // The file's one model, filled in.
        assert_eq!(
            config.strategies["plain"],
            Strategy::Prompted(Prompted {
                system: "{{ name }} the {{ role }}".to_string(),
                model: "qwen".to_string(),
            })
        );
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
            let file = format!("{STRATEGIES}[strategies.odd]\n{strategy}");
            assert!(Config::parse(&file).is_err(), "{strategy}");
        }
    }

    #[test]
    fn a_prompted_strategy_may_leave_its_model_out_only_when_the_file_defines_one() {
        let two = format!(
            "{STRATEGIES}[models.claude]\nbase_url = \"https://api.anthropic.com/v1\"\nid = \"claude\"\n"
        );
        assert_eq!(
            unsettled(&two),
            "strategy plain names no model, which it may leave out only when the file defines exactly one; it defines 2"
        );
        let none = "[strategies.plain]\npolicy = \"prompted\"\nsystem = \"Hi.\"\n";
        let shown = unsettled(none);
        assert!(shown.ends_with("it defines 0"), "{shown}");
        let named = two.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"claude\"",
        );
        let config = Config::parse(&named).unwrap();
        assert!(
            matches!(&config.strategies["plain"], Strategy::Prompted(prompted) if prompted.model == "claude")
        );
    }

    #[test]
    fn a_model_named_empty_or_not_defined_is_an_error_naming_the_strategy() {
        let named = |model: &str| {
            STRATEGIES.replace(
                "policy = \"prompted\"",
                &format!("policy = \"prompted\"\nmodel = \"{model}\""),
            )
        };
        let shown = unsettled(&named(""));
        assert!(
            shown.contains("[strategies.plain]") && shown.contains("model is empty"),
            "{shown}"
        );
        assert_eq!(
            unsettled(&named("claude")),
            "strategy plain names the model claude, which is not defined"
        );
    }

    #[test]
    fn the_random_strategy_is_built_in_and_may_not_be_defined() {
        let file = "[strategies.random]\npolicy = \"random\"\n";
        assert_eq!(
            unsettled(file),
            "[strategies.random] is built in, and may not be defined"
        );
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        for file in [
            format!("{STRATEGIES}temperature = 0.5\n"),
            format!("{STRATEGIES}[phases]\npropose = \"1m\"\n"),
            format!("{STRATEGIES}[roles.merlin]\nsystem = \"\"\n"),
            format!("{STRATEGIES}[roles.merlin]\nprompt = \"\"\n"),
            format!("{STRATEGIES}[prompt]\nsystem = \"\"\n"),
            format!("{STRATEGIES}[model]\nbase_url = \"\"\nid = \"\"\n"),
        ] {
            assert!(Config::parse(&file).is_err(), "{file}");
        }
    }

    #[test]
    fn werewolf_s_limits_are_unknown_keys() {
        for limit in ["night_limit", "day_limit", "ratify_limit"] {
            let error = Config::parse(&format!("[phases]\n{limit} = \"1m\"\n")).unwrap_err();
            assert!(format!("{error:#}").contains(limit), "{error:#}");
        }
    }

    #[test]
    fn an_unknown_role_is_an_error() {
        for role in ["werewolf", "hunter", "servants", "minions", "Merlin"] {
            let error = Config::parse(&format!("[roles.{role}]\ncount = 1\n")).unwrap_err();
            assert!(format!("{error:#}").contains(role), "{error:#}");
        }
    }

    #[test]
    fn a_duration_the_file_cannot_parse_is_an_error() {
        assert!(Config::parse("[phases]\nvote_limit = \"a while\"\n").is_err());
    }

    #[test]
    fn a_context_window_of_0_is_an_error_naming_the_model() {
        let file = STRATEGIES.replace("id = ", "context_window = 0\nid = ");
        assert_eq!(
            unsettled(&file),
            "[models.qwen] context_window is 0; a context window holds at least 1 token"
        );
    }

    #[test]
    fn a_text_block_named_like_a_variable_is_an_error_naming_it() {
        for variable in VARIABLES {
            let file = format!("{STRATEGIES}[text]\n{variable} = \"Taken.\"\n");
            let error = Config::parse(&file).unwrap_err();
            assert!(format!("{error:#}").contains(variable), "{error:#}");
        }
    }

    #[test]
    fn each_role_has_a_table_of_its_own() {
        let keys: Vec<String> = Table::default().counts().map(|(role, _)| key(role)).into();
        assert_eq!(
            keys,
            [
                "merlin", "percival", "servant", "assassin", "morgana", "mordred", "oberon",
                "minion"
            ]
        );
        for (role, _) in Table::default().counts() {
            let file = format!("{STRATEGIES}[roles.{}]\nstrategy = \"dice\"\n", key(role));
            let settings = settle(&file);
            for (other, allotted) in &settings.allotments {
                let strategy = if *other == role { "dice" } else { RANDOM };
                assert!(allotted.contains_key(strategy), "{role}: {allotted:?}");
            }
        }
    }

    #[test]
    fn a_role_is_allotted_by_its_strategies_its_strategy_or_the_file_s_in_that_order() {
        let file = format!(
            "strategy = \"coin\"
{STRATEGIES}
[roles.servant.strategies]
dice = 1
coin = 2
[roles.minion]
strategy = \"dice\"
[roles.assassin]
strategy = \"coin\"
[roles.assassin.strategies]
dice = 1
"
        );
        let settings = settle(&file);
        assert_eq!(
            settings.allotments,
            Allotments::from([
                (Role::Merlin, allotted(&[("coin", 1)])),
                (Role::Percival, allotted(&[("coin", 0)])),
                (Role::Servant, allotted(&[("coin", 2), ("dice", 1)])),
                (Role::Assassin, allotted(&[("dice", 1)])),
                (Role::Morgana, allotted(&[("coin", 0)])),
                (Role::Mordred, allotted(&[("coin", 0)])),
                (Role::Oberon, allotted(&[("coin", 0)])),
                (Role::Minion, allotted(&[("dice", 1)])),
            ])
        );
        assert_eq!(
            settings.table,
            Table {
                servants: 3,
                ..Table::default()
            }
        );
        // Random without a strategy of its own or the file's.
        let settings = settle(STRATEGIES);
        assert_eq!(settings.allotments[&Role::Merlin], allotted(&[(RANDOM, 1)]));
    }

    #[test]
    fn a_role_s_strategy_fills_the_seats_its_strategies_table_leaves() {
        let file = |count: &str| {
            format!(
                "{STRATEGIES}[roles.servant]\n{count}strategy = \"dice\"\n[roles.servant.strategies]\ncoin = 1\n"
            )
        };
        let servants = |file: &str| settle(file).allotments[&Role::Servant].clone();
        assert_eq!(
            servants(&file("count = 5\n")),
            allotted(&[("coin", 1), ("dice", 4)])
        );
        // The code's count without the file's.
        assert_eq!(servants(&file("")), allotted(&[("coin", 1), ("dice", 1)]));
        // A count the table fills leaves nothing to fill.
        let full = file("count = 2\n").replace("coin = 1", "coin = 2");
        assert_eq!(servants(&full), allotted(&[("coin", 2)]));
        // A strategy in the table and filling adds up.
        let both = file("count = 4\n").replace("coin = 1", "coin = 1\ndice = 1");
        assert_eq!(servants(&both), allotted(&[("coin", 1), ("dice", 3)]));
        // A count the table overfills is an error naming both numbers.
        assert_eq!(
            unsettled(&file("count = 0\n")),
            "[roles.servant] count is 0, but [roles.servant.strategies] allots 1"
        );
        // Without a count, a table that allots more than the code's count
        // has as many seats as it allots.
        let more = file("").replace("coin = 1", "coin = 4");
        assert_eq!(servants(&more), allotted(&[("coin", 4)]));
        // A strategy that fills is checked like any other.
        assert_eq!(
            unsettled(&file("count = 5\n").replace("\"dice\"", "\"wild\"")),
            "the Servant is allotted wild, which is not a strategy"
        );
    }

    #[test]
    fn a_strategies_table_s_seats_are_the_role_s_count_and_a_count_that_disagrees_is_an_error() {
        let file = format!("{STRATEGIES}[roles.minion.strategies]\ndice = 1\ncoin = 1\n");
        let agreeing = format!("{file}[roles.minion]\ncount = 2\n");
        assert_eq!(settle(&agreeing).table.minions, 2);
        let disagreeing = format!("{file}[roles.minion]\ncount = 1\n");
        assert_eq!(
            unsettled(&disagreeing),
            "[roles.minion] count is 1, but [roles.minion.strategies] allots 2"
        );
    }

    #[test]
    fn a_strategy_allotted_that_is_not_defined_is_an_error_naming_the_role() {
        let files = [
            ("[roles.mordred.strategies]\nwild = 1\n", "Mordred"),
            ("[roles.percival]\nstrategy = \"wild\"\n", "Percival"),
        ];
        for (roles, role) in files {
            assert_eq!(
                unsettled(&format!("{STRATEGIES}{roles}")),
                format!("the {role} is allotted wild, which is not a strategy")
            );
        }
    }

    #[test]
    fn the_file_s_strategy_must_be_defined_even_when_every_role_is_allotted_otherwise() {
        let mut file = format!("strategy = \"cautous\"\n{STRATEGIES}");
        for (role, _) in Table::default().counts() {
            file.push_str(&format!("[roles.{}]\nstrategy = \"dice\"\n", key(role)));
        }
        assert_eq!(
            unsettled(&file),
            "the file's strategy is cautous, which is not a strategy"
        );
        settle(&format!("strategy = \"{RANDOM}\"\n{STRATEGIES}"));
    }

    #[test]
    fn a_prompted_strategy_may_be_allotted_to_any_role_beside_random_ones() {
        let allotted = [
            ("strategy = \"plain\"\n", Role::Merlin),
            (
                "[roles.oberon]\ncount = 1\nstrategy = \"plain\"\n",
                Role::Oberon,
            ),
            ("[roles.minion.strategies]\nplain = 1\n", Role::Minion),
        ];
        for (roles, role) in allotted {
            let file = match roles.starts_with('[') {
                true => format!("{STRATEGIES}{roles}"),
                false => format!("{roles}{STRATEGIES}"),
            };
            let settings = settle(&file);
            assert_eq!(settings.allotments[&role]["plain"], 1, "{role}");
            let in_play: Vec<&str> = settings.models_in_play().into_keys().collect();
            assert_eq!(in_play, ["qwen"], "{role}");
        }
        // Allotted no seat, a prompted strategy puts no model in play.
        let settings = settle(&format!(
            "{STRATEGIES}[roles.percival]\nstrategy = \"plain\"\n"
        ));
        assert!(settings.models_in_play().is_empty());
    }

    /// A file giving every role a count, for a table of ten: one of each
    /// role but three Servants.
    const COUNTED: &str = "
[roles.merlin]
count = 1
[roles.percival]
count = 1
[roles.servant]
count = 3
[roles.assassin]
count = 1
[roles.morgana]
count = 1
[roles.mordred]
count = 1
[roles.oberon]
count = 1
[roles.minion]
count = 1
";

    #[test]
    fn the_role_counts_fall_back_from_the_file_to_the_code() {
        // The code alone.
        assert_eq!(settle("").table, Table::default());
        // The file over the code.
        let ten = Table {
            merlin: 1,
            percival: 1,
            servants: 3,
            assassin: 1,
            morgana: 1,
            mordred: 1,
            oberon: 1,
            minions: 1,
        };
        assert_eq!(settle(COUNTED).table, ten);
    }

    #[test]
    fn the_table_the_counts_settle_to_is_checked() {
        let refused = [
            ("[roles.servant]\ncount = 1\n", "this table seats 4"),
            (
                &COUNTED.replace("count = 3", "count = 4"),
                "this table seats 11",
            ),
            (
                "[roles.merlin]\ncount = 2\n",
                "a table has at most one Merlin, and this one has 2",
            ),
            (
                "[roles.assassin]\ncount = 0\n[roles.servant]\ncount = 3\n",
                "this one has 1 Merlin and 0 Assassin",
            ),
            (
                "[roles.merlin]\ncount = 0\n[roles.servant]\ncount = 3\n",
                "this one has 0 Merlin and 1 Assassin",
            ),
        ];
        for (file, error) in refused {
            let shown = unsettled(file);
            assert!(shown.contains(error), "{shown}");
        }
    }

    #[test]
    fn the_phase_limits_fall_back_from_the_file_to_the_code() {
        let file = "[phases]
propose_limit = \"1m 30s\"
vote_limit = \"2m\"
quest_limit = \"3m\"
assassinate_limit = \"4m\"
";
        let secs = |rules: &Rules| {
            [
                rules.propose_limit,
                rules.vote_limit,
                rules.quest_limit,
                rules.assassinate_limit,
            ]
            .map(|limit| limit.as_secs())
        };
        // The file over the code.
        assert_eq!(secs(&settle(file).rules), [90, 120, 180, 240]);
        // The code without the file.
        let code = Rules::default();
        assert_eq!(secs(&settle("").rules), secs(&code));
    }

    /// A table of one Merlin, dice, and two Servants, one random and one
    /// coin, with the Assassin and a Minion random.
    fn mixed() -> Settings {
        let file = format!(
            "{STRATEGIES}
[roles.merlin]
strategy = \"dice\"
[roles.servant.strategies]
random = 1
coin = 1
"
        );
        settle(&file)
    }

    #[test]
    fn every_pair_is_dealt_as_many_times_as_allotted_to_the_seats_from_player_1() {
        let settings = mixed();
        let deal = settings.deal();
        let mut seats: Vec<&PlayerId> = deal.keys().collect();
        seats.sort();
        assert_eq!(
            seats,
            ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"]
        );
        let mut pairs: Vec<(Role, &str)> = deal
            .values()
            .map(|dealt| (dealt.role, dealt.strategy.as_str()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            [
                (Role::Merlin, "dice"),
                (Role::Servant, "coin"),
                (Role::Servant, RANDOM),
                (Role::Assassin, RANDOM),
                (Role::Minion, RANDOM),
            ]
        );
    }

    #[test]
    fn the_deal_is_shuffled_in_roles_and_in_strategies() {
        let settings = mixed();
        let deals: Vec<HashMap<PlayerId, Dealt>> = (0..100).map(|_| settings.deal()).collect();
        // Every seat is dealt Merlin playing dice in some deal, and a
        // Servant playing coin in another.
        for strategy in ["dice", "coin"] {
            let seats: HashSet<&PlayerId> = deals
                .iter()
                .flat_map(|deal| deal.iter())
                .filter(|(_, dealt)| dealt.strategy == strategy)
                .map(|(seat, _)| seat)
                .collect();
            assert_eq!(seats.len(), 5, "{strategy}: {seats:?}");
        }
    }

    /// A file saying something under every table, for a game of Merlin,
    /// the Assassin, two Servants and a Minion.
    fn configured() -> String {
        format!(
            "{STRATEGIES}
[phases]
propose_limit = \"30s\"
vote_limit = \"1m 30s\"
quest_limit = \"10s\"
assassinate_limit = \"5s\"
[text]
rules = \"The rules.\"
[roles.merlin]
strategy = \"dice\"
count = 1
[roles.servant.strategies]
coin = 2
[roles.minion]
count = 1
"
        )
    }

    #[test]
    fn the_configuration_carries_the_settings_as_they_took_effect() {
        // Each setting from the file or the code.
        let settings = settle(&configured());
        let configuration = settings
            .configuration(&settings.deal(), &[], Models::new())
            .unwrap();
        let random = || Strategy::Random(Random {});
        assert_eq!(
            configuration,
            Configuration {
                role_counts: Table::default().counts().into_iter().collect(),
                propose_limit: Duration::from_secs(30),
                vote_limit: Duration::from_secs(90),
                quest_limit: Duration::from_secs(10),
                assassinate_limit: Duration::from_secs(5),
                strategies: BTreeMap::from([
                    ("coin".to_string(), random()),
                    ("dice".to_string(), random()),
                    (
                        "plain".to_string(),
                        Strategy::Prompted(Prompted {
                            system: "{{ name }} the {{ role }}".to_string(),
                            model: "qwen".to_string(),
                        })
                    ),
                    (RANDOM.to_string(), random()),
                ]),
                allotments: Allotments::from([
                    (Role::Merlin, allotted(&[("dice", 1)])),
                    (Role::Percival, allotted(&[(RANDOM, 0)])),
                    (Role::Servant, allotted(&[("coin", 2)])),
                    (Role::Assassin, allotted(&[(RANDOM, 1)])),
                    (Role::Morgana, allotted(&[(RANDOM, 0)])),
                    (Role::Mordred, allotted(&[(RANDOM, 0)])),
                    (Role::Oberon, allotted(&[(RANDOM, 0)])),
                    (Role::Minion, allotted(&[(RANDOM, 1)])),
                ]),
                models: Models::new(),
                prompts: BTreeMap::new(),
                text: BTreeMap::from([("rules".to_string(), "The rules.".to_string())]),
            }
        );
    }

    #[test]
    fn the_configuration_entry_writes_durations_as_humantime_does_and_survives_a_trip_through_json()
    {
        let settings = settle(&configured());
        let configuration = settings
            .configuration(&settings.deal(), &[], Models::new())
            .unwrap();
        let entry = Entry::Configuration(Box::new(configuration));
        let json = serde_json::to_value(&entry).unwrap();
        let configuration = &json["Configuration"];
        assert_eq!(configuration["propose_limit"], json!("30s"));
        assert_eq!(configuration["vote_limit"], json!("1m 30s"));
        assert_eq!(configuration["quest_limit"], json!("10s"));
        assert_eq!(configuration["assassinate_limit"], json!("5s"));
        assert_eq!(
            configuration["strategies"]["dice"],
            json!({"policy": "random"})
        );
        // A role is a key by its name.
        assert_eq!(configuration["role_counts"]["Servant"], json!(2));
        assert_eq!(configuration["allotments"]["Servant"], json!({"coin": 2}));
        assert_eq!(configuration["models"], json!({}));
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, entry);
    }

    #[tokio::test(start_paused = true)]
    async fn the_configuration_is_logged_right_after_the_deal_with_an_empty_file_and_with_the_random_config()
     {
        let none = Config::parse("").unwrap();
        let example = Config::load(Path::new(&example("random.toml"))).unwrap();
        for config in [none, example] {
            let settings = config.settle().unwrap();
            let deal = settings.deal();
            let configuration = settings.configuration(&deal, &[], Models::new()).unwrap();
            let (winner, won) = oneshot::channel();
            let (logger, log) = unbounded_channel();
            let episode = game(
                deal.clone(),
                settings.rules.clone(),
                configuration.clone(),
                winner,
                |_| random::Player::init(),
                logger,
            );
            let (_, entries, _) = run(episode, won, log).await.unwrap();
            assert!(
                matches!(&entries[0], Entry::Start { strategies, .. }
                    if strategies.values().all(|strategy| strategy == RANDOM)
                        && strategies.len() == deal.len()),
                "{entries:?}"
            );
            assert_eq!(entries[1], Entry::Configuration(Box::new(configuration)));
            assert!(
                matches!(entries.last(), Some(Entry::End { .. })),
                "{entries:?}"
            );
        }
    }

    /// A file defining one model, `qwen`, and one prompted strategy for
    /// everyone, `plain`, asking it.
    const PLAIN: &str = r#"
strategy = "plain"
[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"
[strategies.plain]
policy = "prompted"
system = "{{ name }}, {{ role }}"
"#;

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
        // Merlin and the Minion play `plain`, the Assassin `bold`, and the
        // Servants `random`.
        let file = format!(
            "{PLAIN}[strategies.bold]\npolicy = \"prompted\"\nsystem = \"Bold {{{{ name }}}}.\"\n\
             [roles.assassin]\nstrategy = \"bold\"\n[roles.servant]\nstrategy = \"random\"\n"
        );
        let prompts = settle(&file).prompts().unwrap();
        let rendered: Vec<(&str, Role, &str)> = prompts
            .iter()
            .map(|prompt| (prompt.seat.as_str(), prompt.role, prompt.strategy.as_str()))
            .collect();
        let seats = ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"];
        let expected: Vec<(&str, Role, &str)> = seats
            .into_iter()
            .flat_map(|seat| {
                [
                    (seat, Role::Merlin, "plain"),
                    (seat, Role::Assassin, "bold"),
                    (seat, Role::Minion, "plain"),
                ]
            })
            .collect();
        assert_eq!(rendered, expected);
        assert_eq!(
            prompt(&prompts, "Player 2", Role::Assassin, "bold"),
            "Bold Player 2."
        );
        assert_eq!(
            prompt(&prompts, "Player 5", Role::Minion, "plain"),
            "Player 5, a minion of Mordred"
        );
    }

    #[test]
    fn every_role_is_rendered_as_a_sentence_names_it_with_its_team() {
        let mut file = PLAIN.replace("{{ role }}", "{{ role }}, on the {{ team }} side");
        file.push_str("[roles.percival]\ncount = 1\n[roles.servant]\ncount = 1\n");
        for role in ["morgana", "mordred", "oberon"] {
            file.push_str(&format!("[roles.{role}]\ncount = 1\n"));
        }
        let settings = settle(&file);
        let prompts = settings.prompts().unwrap();
        for (role, told) in [
            (Role::Merlin, "Merlin, on the good side"),
            (Role::Percival, "Percival, on the good side"),
            (Role::Servant, "a loyal servant of Arthur, on the good side"),
            (Role::Assassin, "the Assassin, on the evil side"),
            (Role::Morgana, "Morgana, on the evil side"),
            (Role::Mordred, "Mordred, on the evil side"),
            (Role::Oberon, "Oberon, on the evil side"),
            (Role::Minion, "a minion of Mordred, on the evil side"),
        ] {
            assert_eq!(
                prompt(&prompts, "Player 1", role, "plain"),
                format!("Player 1, {told}")
            );
        }
    }

    #[test]
    fn every_text_block_is_a_variable_and_a_persona_is_not() {
        let file = PLAIN.replace("{{ name }}, {{ role }}", "{{ rules }} {{ merlin }}");
        let file = format!("{file}[text]\nrules = \"The rules.\"\nmerlin = \"Hide.\"\n");
        let prompts = settle(&file).prompts().unwrap();
        assert_eq!(
            prompt(&prompts, "Player 1", Role::Servant, "plain"),
            "The rules. Hide."
        );
        let file = PLAIN.replace("{{ role }}", "{{ role }}[{{ persona }}]");
        let error = settle(&file).prompts().unwrap_err();
        assert!(format!("{error:#}").contains("persona"), "{error:#}");
    }

    #[test]
    fn a_template_that_does_not_render_is_an_error_naming_the_strategy_the_seat_and_the_role() {
        let file = PLAIN.replace("{{ name }}, {{ role }}", "{{ rulez }}");
        let error = settle(&file).prompts().unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("rendering the prompt of strategy plain for Player 1 as the Merlin"),
            "{shown}"
        );
        assert!(shown.contains("rulez"), "{shown}");
    }

    #[test]
    fn a_template_that_does_not_compile_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace("{{ name }}, {{ role }}", "{% if %}");
        let error = settle(&file).prompts().unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("compiling the prompt template of strategy plain"),
            "{shown}"
        );
        assert!(shown.contains("syntax"), "{shown}");
        // A strategy at no seat of the table is not compiled.
        let file = file.replace("strategy = \"plain\"\n[models", "[models");
        assert!(settle(&file).prompts().unwrap().is_empty());
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
system = \"Smart {{{{ name }}}}.\"
{rest}"
        )
        .replace(
            "system = \"{{ name }},",
            "model = \"qwen\"\nsystem = \"{{ name }},",
        )
    }

    #[test]
    fn a_model_is_in_play_when_a_strategy_at_the_table_asks_it() {
        let settings = settle(&two_models(""));
        let in_play: Vec<&str> = settings.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["qwen"]);
        let both = settle(&two_models("[roles.minion]\nstrategy = \"smart\"\n"));
        let in_play: Vec<&str> = both.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["claude", "qwen"]);
    }

    /// The file [`two_models`] makes, with `claude` at `claude_url` asked
    /// by the Minion, its key not needed, and `qwen` at `qwen_url` with a
    /// context window of 8192.
    fn two_served(claude_url: &str, qwen_url: &str) -> String {
        two_models("[roles.minion]\nstrategy = \"smart\"\n")
            .replace("https://api.anthropic.com/v1", claude_url)
            .replace("api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"\n", "")
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
        let settings = settle(&two_served(&claude_url, &qwen_url));
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
        // Probed once: asked as the leader, a voter, a member of a team and
        // the Assassin, each offered speak, its phase's tool and wait, and
        // made to call a tool.
        let asked = asked.lock().unwrap();
        let offered: Vec<String> = asked
            .iter()
            .map(|request| request["tools"][1]["function"]["name"].to_string())
            .collect();
        assert_eq!(
            offered,
            ["\"propose\"", "\"vote\"", "\"perform\"", "\"select\""]
        );
        assert!(
            asked
                .iter()
                .all(|request| request["tool_choice"] == "required")
        );
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_set_up_is_an_error_naming_it() {
        let claude_url = playing(&["claude-sonnet-4-5"]);
        let qwen_url = canned::refusing(&["qwen2.5-7b-instruct"]);
        let settings = settle(&two_served(&claude_url, &qwen_url));
        let error = settings
            .set_up(&settings.prompts().unwrap())
            .await
            .err()
            .unwrap();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(
                "setting up the model qwen: qwen2.5-7b-instruct did not answer as the leader"
            ),
            "{shown}"
        );
        // A table of random players sets up nothing.
        let file = two_models("").replace("strategy = \"plain\"", "strategy = \"random\"");
        let settings = settle(&file);
        let (models, handles) = settings.set_up(&[]).await.unwrap();
        assert!(models.is_empty() && handles.is_empty());
    }

    #[test]
    fn each_model_s_window_is_checked_against_the_strategies_asking_it_alone() {
        let settings = settle(&two_models("[roles.minion]\nstrategy = \"smart\"\n"));
        // A long prompt for every seat as the Minion, playing `smart`.
        let mut prompts = settings.prompts().unwrap();
        for prompt in &mut prompts {
            if prompt.strategy == "smart" {
                prompt.text = "x".repeat(12000);
            }
        }
        let plain = settings.unstoried("qwen", &prompts);
        let smart = settings.unstoried("claude", &prompts);
        assert!(smart > plain + 3900, "{smart} {plain}");
        assert!(smart < plain + 4500, "{smart} {plain}");
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

    #[test]
    fn the_request_before_its_story_is_sized_for_the_largest_table_and_the_longest_limit() {
        let file = |rest: &str| format!("{PLAIN}{rest}");
        let five = settle(&file(""));
        let ten = settle(&file("[roles.servant]\ncount = 7\n"));
        let mut phases = "[phases]\n".to_string();
        for phase in ["propose", "vote", "quest", "assassinate"] {
            phases.push_str(&format!("{phase}_limit = \"1h 59m 59s\"\n"));
        }
        let slow = settle(&file(&phases));
        let needed = |settings: &Settings| settings.unstoried("qwen", &settings.prompts().unwrap());
        assert!(needed(&ten) > needed(&five));
        assert!(needed(&slow) > needed(&five));
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

    /// A deal of the default table, Merlin and the Servants playing
    /// `plain` and the evil players `random`.
    fn mixed_deal() -> HashMap<PlayerId, Dealt> {
        let dealt = |role, strategy: &str| Dealt {
            role,
            strategy: strategy.to_string(),
        };
        HashMap::from([
            ("Player 1".to_string(), dealt(Role::Servant, "plain")),
            ("Player 2".to_string(), dealt(Role::Assassin, RANDOM)),
            ("Player 3".to_string(), dealt(Role::Merlin, "plain")),
            ("Player 4".to_string(), dealt(Role::Minion, RANDOM)),
            ("Player 5".to_string(), dealt(Role::Servant, "plain")),
        ])
    }

    /// The settings of the table [`mixed_deal`] deals.
    fn mixed_settings() -> Settings {
        let file = format!(
            "{PLAIN}[roles.assassin]\nstrategy = \"random\"\n[roles.minion]\nstrategy = \"random\"\n"
        );
        settle(&file)
    }

    #[test]
    fn the_configuration_carries_each_prompted_player_s_prompt_and_the_models_in_play() {
        let settings = mixed_settings();
        let prompts = settings.prompts().unwrap();
        let models = Models::from([("qwen".to_string(), qwen())]);
        let configuration = settings
            .configuration(&mixed_deal(), &prompts, models.clone())
            .unwrap();
        assert_eq!(configuration.models, models);
        assert_eq!(
            configuration.prompts,
            BTreeMap::from([
                (
                    "Player 1".to_string(),
                    "Player 1, a loyal servant of Arthur".to_string()
                ),
                ("Player 3".to_string(), "Player 3, Merlin".to_string()),
                (
                    "Player 5".to_string(),
                    "Player 5, a loyal servant of Arthur".to_string()
                ),
            ])
        );
        let entry = Entry::Configuration(Box::new(configuration));
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            json["Configuration"]["models"]["qwen"]["context_window"],
            json!({"tokens": 8192, "source": "file"})
        );
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, entry);
        assert_eq!(
            Narrator::default().narrate(&logged(entry)),
            ["qwen's context window is 8192 tokens, as the configuration file says."]
        );
    }

    #[test]
    fn a_prompted_player_with_no_prompt_rendered_is_an_error_naming_the_seat_the_role_and_the_strategy()
     {
        let settings = mixed_settings();
        let mut deal = mixed_deal();
        deal.insert(
            "Player 4".to_string(),
            Dealt {
                role: Role::Minion,
                strategy: "plain".to_string(),
            },
        );
        let error = settings
            .configuration(&deal, &settings.prompts().unwrap(), Models::new())
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "no system prompt rendered for Player 4 as the Minion playing plain"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_table_of_random_evil_and_prompted_good_plays_to_the_end_each_player_by_its_policy() {
        let settings = mixed_settings();
        let deal = mixed_deal();
        let prompts = settings.prompts().unwrap();
        let models = Models::from([("qwen".to_string(), qwen())]);
        let configuration = settings.configuration(&deal, &prompts, models).unwrap();
        let fake = Arc::new(FakeModel::answering(choosing_first));
        let handles = Handles::from([(
            "qwen".to_string(),
            Arc::clone(&fake) as Arc<dyn model::Model>,
        )]);
        let players = players(&configuration, &deal, &handles);
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let episode = game(
            deal.clone(),
            settings.rules.clone(),
            configuration.clone(),
            winner,
            players,
            logger,
        );
        let (winner, entries, _) = run(episode, won, log).await.unwrap();
        assert!(matches!(winner, Team::Good | Team::Evil));
        assert!(
            matches!(entries.last(), Some(Entry::End { .. })),
            "{entries:?}"
        );
        // Each choice is made as its player's policy makes it: a random
        // player's with no model to have been shown anything, a prompted
        // player's by a model shown its memories. Only prompted players
        // talk.
        let prompted: HashSet<&str> = ["Player 1", "Player 3", "Player 5"].into();
        let mut chose = HashSet::new();
        for entry in &entries {
            if let Entry::Action {
                from,
                message,
                seen,
                ..
            } = entry
                && from.as_str() != crate::seat::ENVIRONMENT
            {
                assert_eq!(seen.is_some(), prompted.contains(from.as_str()), "{from}");
                if let Message::Say { .. } = message {
                    assert!(prompted.contains(from.as_str()), "{from}");
                }
                chose.insert(from.as_str());
            }
        }
        assert!(chose.is_superset(&prompted), "{chose:?}");
        // The model was asked by the prompted players alone, as their
        // configuration says.
        for request in fake.requests() {
            assert_eq!(request.model, "qwen2.5-7b-instruct");
            assert_eq!(request.tool_choice, ToolChoice::Required);
            assert!(model::estimated(&request) <= 8192);
            let system = &request.messages[0].content;
            assert!(
                configuration
                    .prompts
                    .values()
                    .any(|prompt| prompt == system),
                "{system}"
            );
        }
    }
}
