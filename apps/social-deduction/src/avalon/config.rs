//! The configuration of a game of Avalon: the [`Config`] a TOML file is
//! parsed into, the [`Overrides`] the command line may give, and the
//! [`Settings`] that take effect once the two are combined with the
//! defaults in the code. A setting comes from the command line if it is
//! given there, otherwise from the file, otherwise from the code. With no
//! file, every player is random.
//!
//! The file defines named models, each a [`Spec`], and named
//! [`Strategy`]s, each a policy with its parameters, and allots each role's
//! seats among the strategies. The strategy [`RANDOM`] is always defined.
//! [`Settings::deal`] deals the (role, strategy) pairs the allotments make
//! to the seats at random. Every strategy at the table is random for now:
//! a prompted strategy may be defined, and allotted no seat.
//!
//! The log records the [`Configuration`] a game ran under right after the
//! deal.

use super::{Dealt, PhaseLimits, Role, RoleCounts, Rules, Table};
use crate::model::spec::{Models, Spec};
use crate::seat::PlayerId;
use crate::strategy::{RANDOM, Random, RoleConfig, Strategy};
use anyhow::{Context as _, bail};
use clap::Args;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

/// The variables every template has besides the text blocks.
const VARIABLES: [&str; 2] = ["name", "role"];

/// What the command line may say over the file: the role counts and the
/// phase limits. Each is unset unless given, so that the file can be
/// heard.
#[derive(Args, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// How many of each role sit at the table.
    #[command(flatten)]
    pub roles: RoleCounts,
    /// How long each phase waits for the players.
    #[command(flatten)]
    pub limits: PhaseLimits,
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
/// the command line and the code have a say in, decided.
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

    /// The settings that take effect with `overrides` from the command
    /// line: each limit from the command line if given there, otherwise
    /// from the file, otherwise from the code; and each role's allotment.
    /// A role's `strategies` table allots its seats first. With the role's
    /// own `strategy`, that strategy fills the rest of the role's count,
    /// from the command line if given there, otherwise from the file,
    /// otherwise from the code, and a count given that is less than the
    /// table allots is an error naming the role and both numbers. Without
    /// one, the role has as many seats as the table allots, and a count in
    /// the file or on the command line that disagrees is an error naming the
    /// role and both numbers. A role with no table plays one strategy, its
    /// own `strategy`, otherwise the file's, otherwise [`RANDOM`], and has
    /// the count from the command line if given there, otherwise from the
    /// file, otherwise from the code. A strategy allotted that is not
    /// defined is an error naming the role. The table the counts make is
    /// then checked as [`Table::check`] checks it, and a prompted strategy
    /// allotted a seat is an error naming the strategy and the role.
    pub fn settle(self, overrides: Overrides) -> anyhow::Result<Settings> {
        let code = Rules::default();
        let (given, file) = (&overrides.limits, &self.phases);
        let rules = Rules {
            propose_limit: (given.propose_limit)
                .or(file.propose_limit)
                .unwrap_or(code.propose_limit),
            vote_limit: (given.vote_limit)
                .or(file.vote_limit)
                .unwrap_or(code.vote_limit),
            quest_limit: (given.quest_limit)
                .or(file.quest_limit)
                .unwrap_or(code.quest_limit),
            assassinate_limit: (given.assassinate_limit)
                .or(file.assassinate_limit)
                .unwrap_or(code.assassinate_limit),
        };
        let mut strategies = self.strategies.clone();
        strategies.insert(RANDOM.to_string(), Strategy::Random(Random {}));
        let mut allotments = Allotments::new();
        for (role, default) in Table::default().counts() {
            let file = self.roles.get(role);
            let given = overrides.roles.get(role);
            let key = key(role);
            let file_count = (format!("[roles.{key}] count"), file.count);
            let flag_count = (RoleCounts::flag(role), given);
            let allotted = match (&file.strategies, &file.strategy) {
                (Some(table), Some(filling)) => {
                    let allots: usize = table.values().sum();
                    let (setting, count) = match given {
                        Some(_) => flag_count,
                        None => file_count,
                    };
                    if let Some(count) = count
                        && count < allots
                    {
                        bail!("{setting} is {count}, but [roles.{key}.strategies] allots {allots}");
                    }
                    let mut table = table.clone();
                    let left = count.unwrap_or(default).saturating_sub(allots);
                    if left > 0 {
                        *table.entry(filling.clone()).or_default() += left;
                    }
                    table
                }
                (Some(table), None) => {
                    let allots: usize = table.values().sum();
                    for (setting, count) in [file_count, flag_count] {
                        if let Some(count) = count
                            && count != allots
                        {
                            bail!(
                                "{setting} is {count}, but [roles.{key}.strategies] allots {allots}"
                            );
                        }
                    }
                    table.clone()
                }
                (None, _) => {
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
        let settings = Settings {
            table,
            rules,
            strategies,
            allotments,
            config: self,
        };
        if let Some((role, name, _, _)) = settings
            .at_the_table()
            .find(|(_, _, strategy, _)| matches!(strategy, Strategy::Prompted(_)))
        {
            bail!(
                "prompted players do not play Avalon yet; strategy {name} is allotted to the {role}"
            );
        }
        Ok(settings)
    }
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

    /// The configuration a game runs under, as the log records it right
    /// after the deal: every setting as it took effect, every strategy and
    /// the allotments, and the text blocks as written. Every player is
    /// random, so no model is in play and no prompt is rendered.
    pub fn configuration(&self) -> Configuration {
        Configuration {
            role_counts: self.table.counts().into_iter().collect(),
            propose_limit: self.rules.propose_limit,
            vote_limit: self.rules.vote_limit,
            quest_limit: self.rules.quest_limit,
            assassinate_limit: self.rules.assassinate_limit,
            strategies: self.strategies.clone(),
            allotments: self.allotments.clone(),
            models: Models::new(),
            prompts: BTreeMap::new(),
            text: self.config.text.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::tests::run;
    use crate::avalon::{Entry, game, random};
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

    /// The example file `name`.
    fn example(name: &str) -> String {
        format!("{}/examples/avalon/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn the_example_parses_and_is_the_one_documented() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/avalon");
        let names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["random.toml"]);
        let settings = Config::load(Path::new(&example("random.toml")))
            .unwrap()
            .settle(Overrides::default())
            .unwrap();
        assert_eq!(settings.table, Table::default());
    }

    #[test]
    fn the_random_example_plays_the_game_of_no_file() {
        let example = Config::load(Path::new(&example("random.toml")))
            .unwrap()
            .settle(Overrides::default())
            .unwrap();
        let none = Config::default().settle(Overrides::default()).unwrap();
        assert_eq!(example.configuration(), none.configuration());
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
            unsettled(&two, Overrides::default()),
            "strategy plain names no model, which it may leave out only when the file defines exactly one; it defines 2"
        );
        let none = "[strategies.plain]\npolicy = \"prompted\"\nsystem = \"Hi.\"\n";
        let shown = unsettled(none, Overrides::default());
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
        let shown = unsettled(&named(""), Overrides::default());
        assert!(
            shown.contains("[strategies.plain]") && shown.contains("model is empty"),
            "{shown}"
        );
        assert_eq!(
            unsettled(&named("claude"), Overrides::default()),
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
            unsettled(&file, Overrides::default()),
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
            let settings = settle(&file, Overrides::default());
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
        let settings = settle(&file, Overrides::default());
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
        let settings = settle(STRATEGIES, Overrides::default());
        assert_eq!(settings.allotments[&Role::Merlin], allotted(&[(RANDOM, 1)]));
    }

    #[test]
    fn a_role_s_strategy_fills_the_seats_its_strategies_table_leaves() {
        let file = |count: &str| {
            format!(
                "{STRATEGIES}[roles.servant]\n{count}strategy = \"dice\"\n[roles.servant.strategies]\ncoin = 1\n"
            )
        };
        let servants =
            |file: &str, overrides| settle(file, overrides).allotments[&Role::Servant].clone();
        assert_eq!(
            servants(&file("count = 5\n"), Overrides::default()),
            allotted(&[("coin", 1), ("dice", 4)])
        );
        // The count from the command line over the file's, and the code's
        // without either.
        let given = |servants| {
            counting(RoleCounts {
                servants: Some(servants),
                ..RoleCounts::default()
            })
        };
        assert_eq!(
            servants(&file("count = 5\n"), given(4)),
            allotted(&[("coin", 1), ("dice", 3)])
        );
        assert_eq!(
            servants(&file(""), Overrides::default()),
            allotted(&[("coin", 1), ("dice", 1)])
        );
        // A count the table fills leaves nothing to fill.
        let full = file("count = 2\n").replace("coin = 1", "coin = 2");
        assert_eq!(
            servants(&full, Overrides::default()),
            allotted(&[("coin", 2)])
        );
        // A strategy in the table and filling adds up.
        let both = file("count = 4\n").replace("coin = 1", "coin = 1\ndice = 1");
        assert_eq!(
            servants(&both, Overrides::default()),
            allotted(&[("coin", 1), ("dice", 3)])
        );
        // A count the table overfills is an error naming both numbers.
        assert_eq!(
            unsettled(&file("count = 0\n"), Overrides::default()),
            "[roles.servant] count is 0, but [roles.servant.strategies] allots 1"
        );
        assert_eq!(
            unsettled(&file("count = 5\n"), given(0)),
            "--servants is 0, but [roles.servant.strategies] allots 1"
        );
        // Without a count, a table that allots more than the code's count
        // has as many seats as it allots.
        let more = file("").replace("coin = 1", "coin = 4");
        assert_eq!(
            servants(&more, Overrides::default()),
            allotted(&[("coin", 4)])
        );
        // A strategy that fills is checked like any other.
        assert_eq!(
            unsettled(
                &file("count = 5\n").replace("\"dice\"", "\"wild\""),
                Overrides::default()
            ),
            "the Servant is allotted wild, which is not a strategy"
        );
    }

    #[test]
    fn a_strategies_table_s_seats_are_the_role_s_count_and_a_count_that_disagrees_is_an_error() {
        let file = format!("{STRATEGIES}[roles.minion.strategies]\ndice = 1\ncoin = 1\n");
        let agreeing = format!("{file}[roles.minion]\ncount = 2\n");
        let given = |minions| {
            counting(RoleCounts {
                minions: Some(minions),
                ..RoleCounts::default()
            })
        };
        assert_eq!(settle(&agreeing, given(2)).table.minions, 2);
        let disagreeing = format!("{file}[roles.minion]\ncount = 1\n");
        assert_eq!(
            unsettled(&disagreeing, Overrides::default()),
            "[roles.minion] count is 1, but [roles.minion.strategies] allots 2"
        );
        assert_eq!(
            unsettled(&agreeing, given(3)),
            "--minions is 3, but [roles.minion.strategies] allots 2"
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
                unsettled(&format!("{STRATEGIES}{roles}"), Overrides::default()),
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
            unsettled(&file, Overrides::default()),
            "the file's strategy is cautous, which is not a strategy"
        );
        settle(
            &format!("strategy = \"{RANDOM}\"\n{STRATEGIES}"),
            Overrides::default(),
        );
    }

    #[test]
    fn a_prompted_strategy_allotted_a_seat_is_an_error_naming_it_and_the_role() {
        let refused = [
            ("strategy = \"plain\"\n", "Merlin"),
            (
                "[roles.oberon]\ncount = 1\nstrategy = \"plain\"\n",
                "Oberon",
            ),
            ("[roles.minion.strategies]\nplain = 1\n", "Minion"),
        ];
        for (roles, role) in refused {
            let file = match roles.starts_with('[') {
                true => format!("{STRATEGIES}{roles}"),
                false => format!("{roles}{STRATEGIES}"),
            };
            assert_eq!(
                unsettled(&file, Overrides::default()),
                format!(
                    "prompted players do not play Avalon yet; strategy plain is allotted to the {role}"
                )
            );
        }
        // Defined, or allotted no seat, a prompted strategy is fine.
        settle(STRATEGIES, Overrides::default());
        settle(
            &format!("{STRATEGIES}[roles.percival]\nstrategy = \"plain\"\n"),
            Overrides::default(),
        );
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
    fn the_role_counts_fall_back_from_the_command_line_to_the_file_to_the_code() {
        // The code alone.
        assert_eq!(settle("", Overrides::default()).table, Table::default());
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
        assert_eq!(settle(COUNTED, Overrides::default()).table, ten);
        // The command line over the file, for every count.
        let counts = RoleCounts {
            merlin: Some(0),
            percival: Some(0),
            servants: Some(2),
            assassin: Some(0),
            morgana: Some(0),
            mordred: Some(0),
            oberon: Some(0),
            minions: Some(3),
        };
        assert_eq!(
            settle(COUNTED, counting(counts)).table,
            Table {
                merlin: 0,
                percival: 0,
                servants: 2,
                assassin: 0,
                morgana: 0,
                mordred: 0,
                oberon: 0,
                minions: 3,
            }
        );
        // A count the command line does not give still comes from the file.
        let counts = RoleCounts {
            servants: Some(1),
            minions: Some(0),
            ..RoleCounts::default()
        };
        assert_eq!(
            settle(COUNTED, counting(counts)).table,
            Table {
                servants: 1,
                minions: 0,
                ..ten
            }
        );
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
            let shown = unsettled(file, Overrides::default());
            assert!(shown.contains(error), "{shown}");
        }
        // From the command line too.
        let counts = RoleCounts {
            servants: Some(0),
            ..RoleCounts::default()
        };
        let shown = unsettled("", counting(counts));
        assert!(shown.contains("this table seats 3"), "{shown}");
    }

    #[test]
    fn the_phase_limits_fall_back_from_the_command_line_to_the_file_to_the_code() {
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
        assert_eq!(
            secs(&settle(file, Overrides::default()).rules),
            [90, 120, 180, 240]
        );
        // The command line over the file, for every limit.
        let limits = |propose: Option<u64>,
                      vote: Option<u64>,
                      quest: Option<u64>,
                      assassinate: Option<u64>| Overrides {
            limits: PhaseLimits {
                propose_limit: propose.map(Duration::from_secs),
                vote_limit: vote.map(Duration::from_secs),
                quest_limit: quest.map(Duration::from_secs),
                assassinate_limit: assassinate.map(Duration::from_secs),
            },
            ..Overrides::default()
        };
        assert_eq!(
            secs(&settle(file, limits(Some(1), Some(2), Some(3), Some(4))).rules),
            [1, 2, 3, 4]
        );
        // A limit the command line does not give still comes from the file.
        assert_eq!(
            secs(&settle(file, limits(Some(1), None, Some(3), None)).rules),
            [1, 120, 3, 240]
        );
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
        // The propose limit from the command line, everything else from
        // the file or the code.
        let overrides = Overrides {
            limits: PhaseLimits {
                propose_limit: Some(Duration::from_secs(5)),
                ..PhaseLimits::default()
            },
            ..Overrides::default()
        };
        let configuration = settle(&configured(), overrides).configuration();
        let random = || Strategy::Random(Random {});
        assert_eq!(
            configuration,
            Configuration {
                role_counts: Table::default().counts().into_iter().collect(),
                propose_limit: Duration::from_secs(5),
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
        let configuration = settle(&configured(), Overrides::default()).configuration();
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
    async fn the_configuration_is_logged_right_after_the_deal_with_no_file_and_with_the_example() {
        let none = Config::default();
        let example = Config::load(Path::new(&example("random.toml"))).unwrap();
        for config in [none, example] {
            let settings = config.settle(Overrides::default()).unwrap();
            let deal = settings.deal();
            let configuration = settings.configuration();
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
}
