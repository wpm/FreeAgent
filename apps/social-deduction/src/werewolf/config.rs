//! The configuration of a game of Werewolf: the [`Config`] a TOML file is
//! parsed into, and the [`Settings`] that take effect once it is combined
//! with the defaults in the code. A setting comes from the file if it is
//! given there, otherwise from the code.
//!
//! The file defines named models, each a [`Spec`], and named
//! [`Strategy`]s, each a policy with its parameters, and allots each role's
//! seats among the strategies.
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
//! the environment variable that holds it, and [`Spec::api_key`] reads
//! that variable into a [`SecretString`](secrecy::SecretString), which is redacted wherever it is
//! shown.
//!
//! The log records the [`Configuration`] a game ran under right after the
//! deal, and [`players`] builds each seat's player from it.

use super::prompted;
use super::{Dealt, Role, Rules, Seat, Table, random, ratified};
use crate::config::check_common;
use crate::model::Provider;
use crate::model::spec::{self, Handles, Models, Spec};
use crate::model::window::{self, ContextWindow};
use crate::seat::PlayerId;
use crate::strategy::{RANDOM, Random, RoleConfig, Strategy};
use anyhow::{Context as _, anyhow, bail};
use humantime::format_duration;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, btree_map};
use std::sync::Arc;
use std::time::Duration;

/// The variables every template has besides the text blocks.
const VARIABLES: [&str; 2] = ["name", "role"];

/// The configuration file, as written. Every table may be left out, and an
/// unknown key anywhere is an error, so a misspelled setting fails loudly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The `game` key, which [`crate::config::load`] reads first and
    /// checks. A file parsed here may leave it out.
    #[serde(default)]
    #[expect(dead_code, reason = "config::load reads the game key itself")]
    game: Option<crate::config::Name>,
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
    /// Parse `file`, the text of a configuration file, and check what every
    /// game's file holds alike, with this game's template variables: a
    /// `[text]` block named like one of them is an error, and so is a
    /// model's context window of 0, a strategy named [`RANDOM`], a file's
    /// `strategy` that is not defined, and a prompted strategy naming a
    /// model the file does not define, or naming none when the file does
    /// not define exactly one.
    pub fn parse(file: &str) -> anyhow::Result<Self> {
        let mut config: Self = toml::from_str(file)?;
        check_common(
            &config.text,
            &config.models,
            config.strategy.as_ref(),
            &mut config.strategies,
            &VARIABLES,
        )?;
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
    /// an error naming the role. A ratify limit longer than the day limit
    /// is an error naming both, since the day would run out before any vote
    /// stood.
    pub fn settle(self) -> anyhow::Result<Settings> {
        let rules = Rules::default();
        let day_limit = self.phases.day_limit.unwrap_or(rules.day_limit);
        let ratify_limit = self.phases.ratify_limit.unwrap_or(rules.ratify_limit);
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
            let allotted = match (&file.strategies, &file.strategy) {
                (Some(table), filling) => {
                    let allots: usize = table.values().sum();
                    if let Some(count) = file.count
                        && (count < allots || filling.is_none() && count != allots)
                    {
                        bail!(
                            "[roles.{role}] count is {count}, but [roles.{role}.strategies] allots {allots}"
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
        Ok(Settings {
            table: Table {
                werewolves: seats(Role::Werewolf),
                villagers: seats(Role::Villager),
                doctors: seats(Role::Doctor),
                seers: seats(Role::Seer),
            },
            night_limit: self.phases.night_limit.unwrap_or(rules.night_limit),
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
        let needed = self.unstoried(name, prompts)?;
        window::settle(name, &self.config.models[name], reported, needed)
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
                let (day_limit, ratified) = (self.day_limit, ratified(self.ratify_limit));
                let needed = prompted::unstoried(id, text, seat, role, &alive, day_limit, ratified);
                Ok(most.max(needed?))
            })
    }

    /// Set up each model in play as [`spec::set_up`] sets it up, settling
    /// its context window for `prompts` and asking it to play as by night
    /// and as by day, which settles the tool choice. A model the file
    /// defines and no strategy at the table asks is not contacted.
    pub async fn set_up(&self, prompts: &[SystemPrompt]) -> anyhow::Result<(Models, Handles)> {
        let needed = |name: &str| self.unstoried(name, prompts);
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
/// `handles`, in a day that is ratified if the configuration's ratify
/// limit is above zero.
pub fn players(
    configuration: &Configuration,
    deal: &HashMap<PlayerId, Dealt>,
    handles: &Handles,
) -> impl FnMut(&PlayerId) -> Seat + use<> {
    let ratified = ratified(configuration.ratify_limit);
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
            ratified,
        ),
        None => random::Player::init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned::{self, playing};
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::model::spec::ModelInPlay;
    use crate::model::window::Source;
    use crate::model::{self, CONTEXT_WINDOW, REQUEST_TIMEOUT, ToolChoice};
    use crate::strategy::Prompted;
    use crate::werewolf::report::Narrator;
    use crate::werewolf::tests::{between_the_deal_and_the_end, logged, run};
    use crate::werewolf::{Entry, Message, game};
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

    /// The example file `name`, parsed.
    fn example(name: &str) -> Config {
        let path = format!("{}/configs/werewolf/{name}", env!("CARGO_MANIFEST_DIR"));
        Config::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_configs_parse_and_render_and_are_the_two_documented() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/configs/werewolf");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["prompted.toml", "random.toml"]);
        for name in names {
            let settings = example(&name)
                .settle()
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
    fn the_random_config_plays_the_code_defaults_with_no_models() {
        let example = example("random.toml").settle().unwrap();
        let none = Config::parse("").unwrap().settle().unwrap();
        let configured = |settings: &Settings| {
            let deal = settings.deal();
            settings.configuration(&deal, &[], Models::new()).unwrap()
        };
        assert_eq!(configured(&example), configured(&none));
        assert!(example.models_in_play().is_empty());
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
        let rules = Rules::default();
        assert_eq!(settings.night_limit, rules.night_limit);
        assert_eq!(settings.day_limit, rules.day_limit);
        assert_eq!(settings.ratify_limit, Duration::ZERO);
    }

    #[test]
    fn a_strategy_of_either_policy_parses_and_is_written_with_its_model() {
        let file = format!("{PLAIN}[strategies.dice]\npolicy = \"random\"\n");
        let config = Config::parse(&file).unwrap();
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
            let file = format!("{PLAIN}[strategies.odd]\n{strategy}");
            assert!(Config::parse(&file).is_err(), "{strategy}");
        }
        let file = format!("{PLAIN}[strategies.odd]\npolicy = \"prompted\"\n");
        let error = Config::parse(&file).unwrap_err();
        assert!(format!("{error:#}").contains("system"), "{error:#}");
    }

    #[test]
    fn a_model_named_empty_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace(
            "policy = \"prompted\"",
            "policy = \"prompted\"\nmodel = \"\"",
        );
        let shown = unsettled(&file);
        assert!(
            shown.contains("[strategies.plain]") && shown.contains("model is empty"),
            "{shown}"
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
        assert!(Config::parse("[phases]\nnight_limit = \"a while\"\n").is_err());
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
        let settings = settle(&file);
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
        let settings = settle(&file);
        assert_eq!(settings.allotments[&Role::Seer], allotted(&[("bold", 1)]));
        assert_eq!(
            settings.allotments[&Role::Werewolf],
            allotted(&[(RANDOM, 2)])
        );
    }

    #[test]
    fn a_role_s_strategy_fills_the_seats_its_strategies_table_leaves() {
        let file = |count: &str| {
            format!(
                "{}[roles.villager]\n{count}strategy = \"dice\"\n[roles.villager.strategies]\nbold = 2\n",
                three()
            )
        };
        let villagers = |file: &str| settle(file).allotments[&Role::Villager].clone();
        assert_eq!(
            villagers(&file("count = 5\n")),
            allotted(&[("bold", 2), ("dice", 3)])
        );
        // The code's count without the file's.
        assert_eq!(villagers(&file("")), allotted(&[("bold", 2), ("dice", 1)]));
        // A count the table fills leaves nothing to fill.
        assert_eq!(villagers(&file("count = 2\n")), allotted(&[("bold", 2)]));
        // A strategy in the table and filling adds up.
        let both = file("count = 4\n").replace("bold = 2", "bold = 1\ndice = 1");
        assert_eq!(villagers(&both), allotted(&[("bold", 1), ("dice", 3)]));
        // A count the table overfills is an error naming both numbers.
        assert_eq!(
            unsettled(&file("count = 1\n")),
            "[roles.villager] count is 1, but [roles.villager.strategies] allots 2"
        );
        // Without a count, a table that allots more than the code's count
        // has as many seats as it allots.
        let more = file("").replace("bold = 2", "bold = 4");
        assert_eq!(villagers(&more), allotted(&[("bold", 4)]));
        // A strategy that fills is checked like any other.
        assert_eq!(
            unsettled(&file("count = 5\n").replace("\"dice\"", "\"wild\"")),
            "the villager is allotted wild, which is not a strategy"
        );
    }

    #[test]
    fn a_strategies_table_s_seats_are_the_role_s_count_and_a_count_that_disagrees_is_an_error() {
        let file = format!(
            "{}[roles.werewolf.strategies]\ndice = 1\nbold = 2\n",
            three()
        );
        let agreeing = format!("{file}[roles.werewolf]\ncount = 3\n");
        assert_eq!(settle(&agreeing).table.werewolves, 3);
        let file = file.replace(
            "[roles.werewolf.strategies]",
            "[roles.werewolf]\ncount = 2\n[roles.werewolf.strategies]",
        );
        assert_eq!(
            unsettled(&file),
            "[roles.werewolf] count is 2, but [roles.werewolf.strategies] allots 3"
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
                unsettled(&format!("{PLAIN}{roles}")),
                format!("the {role} is allotted wild, which is not a strategy")
            );
        }
        // A strategy defined and never allotted is no error.
        settle(&three());
    }

    #[test]
    fn the_file_s_strategy_must_be_defined_even_when_every_role_is_allotted_otherwise() {
        let mut file = PLAIN.replace("strategy = \"plain\"", "strategy = \"cautous\"");
        for role in ["werewolf", "villager", "doctor", "seer"] {
            file.push_str(&format!("[roles.{role}]\nstrategy = \"plain\"\n"));
        }
        assert_eq!(
            unsettled(&file),
            "the file's strategy is cautous, which is not a strategy"
        );
        let file = PLAIN.replace("strategy = \"plain\"", &format!("strategy = \"{RANDOM}\""));
        settle(&file);
    }

    #[test]
    fn the_role_counts_fall_back_from_the_file_to_the_code() {
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
        assert_eq!(settle("").table, Table::default());
        // The file over the code.
        assert_eq!(
            settle(file).table,
            Table {
                werewolves: 5,
                villagers: 6,
                doctors: 7,
                seers: 8
            }
        );
    }

    #[test]
    fn the_phase_limits_fall_back_from_the_file_to_the_code() {
        let file = "[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"2m\"\n";
        // The file over the code.
        let settings = settle(file);
        assert_eq!(settings.night_limit, Duration::from_secs(90));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
        // The code without the file.
        let settings = settle("");
        assert_eq!(settings.night_limit, Rules::default().night_limit);
        assert_eq!(settings.day_limit, Rules::default().day_limit);
    }

    #[test]
    fn the_ratify_limit_falls_back_from_the_file_to_the_code() {
        let file = "[phases]\nratify_limit = \"15s\"\n";
        assert_eq!(settle(file).ratify_limit, Duration::from_secs(15));
        assert_eq!(settle("").ratify_limit, Rules::default().ratify_limit);
    }

    #[test]
    fn a_ratify_limit_longer_than_the_day_limit_is_an_error_naming_both() {
        let file = "[phases]\nday_limit = \"20s\"\nratify_limit = \"30s\"\n";
        assert_eq!(
            unsettled(file),
            "the ratify limit (30s) is longer than the day limit (20s)"
        );
        // Against the code's day limit when the file gives none.
        let day_limit = Rules::default().day_limit;
        let ratify_limit = format_duration(day_limit + Duration::from_secs(1));
        assert_eq!(
            unsettled(&format!("[phases]\nratify_limit = \"{ratify_limit}\"\n")),
            format!(
                "the ratify limit ({ratify_limit}) is longer than the day limit ({})",
                format_duration(day_limit)
            )
        );
        // One as long as the day is allowed.
        let file = "[phases]\nday_limit = \"20s\"\nratify_limit = \"20s\"\n";
        assert_eq!(settle(file).ratify_limit, Duration::from_secs(20));
    }

    #[test]
    fn the_rules_are_the_default_rules_with_the_limits_that_took_effect() {
        let file =
            "[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"7s\"\nratify_limit = \"3s\"\n";
        let settled = settle(file).rules();
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
        let seats = ["Player 1", "Player 2", "Player 3", "Player 4", "Player 5"];
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
            prompt(&prompts, "Player 2", Role::Werewolf, "bold"),
            "Bold Player 2."
        );
        assert_eq!(
            prompt(&prompts, "Player 5", Role::Villager, "plain"),
            "Player 5 the villager"
        );
        // A table with no prompted strategy renders nothing.
        assert!(settle("").prompts().unwrap().is_empty());
    }

    #[test]
    fn every_text_block_is_a_variable_and_a_persona_is_not() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{{ rules }} {{ wolf }}");
        let file = format!("{file}[text]\nrules = \"The rules.\"\nwolf = \"Howl.\"\n");
        let prompts = settle(&file).prompts().unwrap();
        assert_eq!(
            prompt(&prompts, "Player 1", Role::Villager, "plain"),
            "The rules. Howl."
        );
        let file = PLAIN.replace("{{ role }}", "{{ role }}[{{ persona }}]");
        let error = settle(&file).prompts().unwrap_err();
        assert!(format!("{error:#}").contains("persona"), "{error:#}");
    }

    #[test]
    fn a_template_that_does_not_render_is_an_error_naming_the_strategy_the_seat_and_the_role() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{{ rulez }}");
        let error = settle(&file).prompts().unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown
                .starts_with("rendering the prompt of strategy plain for Player 1 as the werewolf"),
            "{shown}"
        );
        assert!(shown.contains("rulez"), "{shown}");
    }

    #[test]
    fn a_template_that_does_not_compile_is_an_error_naming_the_strategy() {
        let file = PLAIN.replace("{{ name }} the {{ role }}", "{% if %}");
        let error = settle(&file).prompts().unwrap_err();
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
        let settings = settle(&two_models(""));
        let in_play: Vec<&str> = settings.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["qwen"]);
        let both = settle(&two_models("[roles.seer]\nstrategy = \"smart\"\n"));
        let in_play: Vec<&str> = both.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["claude", "qwen"]);
        // Allotted no seats, a strategy puts no model in play.
        let unseated = settle(&two_models(
            "[roles.seer]\ncount = 0\nstrategy = \"smart\"\n",
        ));
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
        let settings = settle(&three_models(&claude_url, &qwen_url));
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
        let settings = settle(&three_models(&claude_url, &qwen_url));
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
        let settings = settle(&two_models("[roles.seer]\nstrategy = \"smart\"\n"));
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
        let settings = settle(&file);
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
        let settings = settle(&file);
        let (models, handles) = settings.set_up(&[]).await.unwrap();
        assert!(models.is_empty() && handles.is_empty());
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
            seat: "Player 1".to_string(),
            role: Role::Werewolf,
            strategy: RANDOM.to_string(),
            text: "x".repeat(longest * 2),
        });
        prompts
    }

    #[test]
    fn a_context_window_with_no_room_for_a_story_is_an_error_naming_the_model_and_where_it_came_from()
     {
        let settings = settle(PLAIN);
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
        let error = settle(&openai)
            .context_window("qwen", None, &enormous)
            .unwrap_err();
        assert!(
            format!("{error:#}")
                .starts_with("the provider's default context window for qwen is 128000 tokens"),
            "{error:#}"
        );
        let file = PLAIN.replace("id = ", "context_window = 2000\nid = ");
        assert_eq!(
            refused(&settle(&file), None),
            format!("[models.qwen] context_window is 2000 tokens, {too_small}")
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
            ("Player 1".to_string(), dealt(Role::Villager, "dice")),
            ("Player 2".to_string(), dealt(Role::Werewolf, "bold")),
            ("Player 3".to_string(), dealt(Role::Seer, "plain")),
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
        // Each setting from the file or the code.
        let configuration = configuration_of(&settle(&configured()), &dealt()).unwrap();
        let prompted = |system: &str| {
            Strategy::Prompted(Prompted {
                system: system.to_string(),
                model: "qwen".to_string(),
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
                night_limit: Duration::from_secs(30),
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
                    ("Player 2".to_string(), "Bold Player 2.".to_string()),
                    (
                        "Player 3".to_string(),
                        "The rules. Player 3 the seer".to_string()
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
            "Player 3".to_string(),
            Dealt {
                role: Role::Doctor,
                strategy: "plain".to_string(),
            },
        );
        let error = configuration_of(&settle(&configured()), &deal).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "no system prompt rendered for Player 3 as the doctor playing plain"
        );
    }

    #[test]
    fn the_configuration_entry_writes_durations_as_humantime_does_and_survives_a_trip_through_json()
    {
        let configuration = configuration_of(&settle(&configured()), &dealt()).unwrap();
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
        let settings = settle("");
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
        let settings = settle(&configured());
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
        let settings = settle(&configured());
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
        let settings = settle(&file);
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
        let settings = settle(&file);
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
