//! The configuration of a game of Avalon: the [`Config`] a TOML file is
//! parsed into, and the [`Settings`] that take effect once it is combined
//! with the defaults in the code. A setting comes from the file if it is
//! given there, otherwise from the code.
//!
//! The file is every game's [`crate::config::Config`], with Avalon's
//! [`Phases`] and [`Roles`]. A prompted player's system prompt is rendered
//! for its seat and role, with the variables `name`, `role`, as
//! [`Role::told`] tells it, and `team`, `good` or `evil`.
//!
//! A model is in play when a prompted strategy that names it is allotted a
//! seat. [`Settings::set_up`] sets up each model in play before any actor
//! is built: it reads the key, settles the
//! [`ContextWindow`](crate::model::window::ContextWindow) and asks the
//! model to play. The API key never appears in the file. The file names
//! the environment variable that holds it, and
//! [`Spec::api_key`](crate::model::spec::Spec::api_key) reads that
//! variable into a [`SecretString`](secrecy::SecretString), which is
//! redacted wherever it is shown.
//!
//! The log records the [`Configuration`] a game ran under right after the
//! deal, and [`players`] builds each seat's player from it.

use super::{Avalon, Role, Rules, Table};
use crate::config::Lineup;
pub use crate::config::players;
use crate::strategy::RoleConfig;
use serde::Deserialize;
use std::time::Duration;

/// The variables every template has besides the text blocks.
const VARIABLES: [&str; 3] = ["name", "role", "team"];

/// The configuration file, as written.
pub type Config = crate::config::Config<Phases, Roles>;

/// Parse `file`, the text of a configuration file, as
/// [`crate::config::Config::parse`] parses it, with Avalon's template
/// variables.
pub fn parse(file: &str) -> anyhow::Result<Config> {
    Config::parse(file, &VARIABLES)
}

/// The configuration `table` holds, as
/// [`crate::config::Config::from_table`] makes it, with Avalon's
/// template variables.
pub fn from_table(table: toml::Table) -> anyhow::Result<Config> {
    Config::from_table(table, &VARIABLES)
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

/// The `[roles]` tables, one per role, each named as the role in lower
/// case. A role the file does not mention has nothing said about it.
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

/// How many seats of each role play each strategy, by name.
pub type Allotments = crate::config::Allotments<Role>;

/// One player's system prompt, rendered for a seat dealt a role and a
/// prompted strategy.
pub type SystemPrompt = crate::config::SystemPrompt<Role>;

/// The settings that took effect.
pub type Settings = crate::config::Settings<Avalon>;

impl Config {
    /// The settings that take effect: each limit from the file if given
    /// there, otherwise from the code; and each role's allotment, as
    /// [`crate::config::Config::allot`] allots it. The table the counts
    /// make is then checked as [`Table::check`] checks it.
    pub fn settle(self) -> anyhow::Result<Settings> {
        let (code, file) = (Rules::default(), &self.phases);
        let limits = Rules {
            propose_limit: file.propose_limit.unwrap_or(code.propose_limit),
            vote_limit: file.vote_limit.unwrap_or(code.vote_limit),
            quest_limit: file.quest_limit.unwrap_or(code.quest_limit),
            assassinate_limit: file.assassinate_limit.unwrap_or(code.assassinate_limit),
        };
        let lineup = self.allot(Table::default().counts(), Roles::get)?;
        table(&lineup).check()?;
        Ok(Settings {
            limits,
            lineup,
            models: self.models,
            text: self.text,
        })
    }
}

/// The table `lineup` seats: how many of each role.
fn table(lineup: &Lineup<Role>) -> Table {
    let counts = lineup.role_counts();
    let seats = |role| counts[&role];
    Table {
        merlin: seats(Role::Merlin),
        percival: seats(Role::Percival),
        servants: seats(Role::Servant),
        assassin: seats(Role::Assassin),
        morgana: seats(Role::Morgana),
        mordred: seats(Role::Mordred),
        oberon: seats(Role::Oberon),
        minions: seats(Role::Minion),
    }
}

/// The configuration a game ran under.
pub type Configuration = crate::config::Configuration<Avalon>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::Dealt;
    use crate::avalon::report::Narrator;
    use crate::avalon::tests::{logged, run};
    use crate::avalon::{Entry, Message, Outcome, Team, random};
    use crate::game::Narrate;
    use crate::game::game;
    use crate::model::canned::{self, playing};
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::model::spec::{Environment, Handles, ModelInPlay, Models};
    use crate::model::window::{ContextWindow, Source};
    use crate::model::{self, CONTEXT_WINDOW, REQUEST_TIMEOUT, ToolChoice};
    use crate::seat::PlayerId;
    use crate::strategy::Strategy;
    use crate::strategy::{Prompted, RANDOM, Random};
    use serde_json::json;
    use std::collections::HashSet;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;
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
        parse(file).unwrap().settle().unwrap()
    }

    /// The error `file` settles to, causes included.
    fn unsettled(file: &str) -> String {
        let error = parse(file).and_then(Config::settle).unwrap_err();
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
        let path = format!("{}/configs/avalon/{name}", env!("CARGO_MANIFEST_DIR"));
        parse(&std::fs::read_to_string(path).unwrap()).unwrap()
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
            let settings = example(name).settle().unwrap();
            assert_eq!(table(&settings.lineup), Table::default(), "{name}");
            settings.prompts().unwrap();
        }
        // The prompted example tells every player the rules, that it may
        // talk in every phase, and who it is.
        let settings = example("prompted.toml").settle().unwrap();
        let prompts = settings.prompts().unwrap();
        let in_play: Vec<&str> = settings.models_in_play().into_keys().collect();
        assert_eq!(in_play, ["qwen"]);
        let rules = &settings.text["rules"];
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
        let example = example("random.toml").settle().unwrap();
        let none = parse("").unwrap().settle().unwrap();
        let configured = |settings: &Settings| {
            let deal = settings.lineup.deal();
            settings.configuration(&deal, &[], Models::new()).unwrap()
        };
        assert_eq!(configured(&example), configured(&none));
        assert!(example.models_in_play().is_empty());
        assert!(example.prompts().unwrap().is_empty());
    }

    #[test]
    fn with_nothing_in_the_file_every_role_of_the_default_table_is_random() {
        let settings = settle("");
        assert_eq!(table(&settings.lineup), Table::default());
        assert_eq!(
            settings.lineup.allotments,
            Table::default()
                .counts()
                .into_iter()
                .map(|(role, seats)| (role, allotted(&[(RANDOM, seats)])))
                .collect::<Allotments>()
        );
        assert_eq!(
            settings.lineup.strategies,
            BTreeMap::from([(RANDOM.to_string(), Strategy::Random(Random {}))])
        );
        assert_eq!(settings.limits, Rules::default());
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
            assert!(parse(&file).is_err(), "{file}");
        }
    }

    #[test]
    fn werewolf_s_limits_are_unknown_keys() {
        for limit in ["night_limit", "day_limit", "ratify_limit"] {
            let error = parse(&format!("[phases]\n{limit} = \"1m\"\n")).unwrap_err();
            assert!(format!("{error:#}").contains(limit), "{error:#}");
        }
    }

    #[test]
    fn an_unknown_role_is_an_error() {
        for role in ["werewolf", "hunter", "servants", "minions", "Merlin"] {
            let error = parse(&format!("[roles.{role}]\ncount = 1\n")).unwrap_err();
            assert!(format!("{error:#}").contains(role), "{error:#}");
        }
    }

    #[test]
    fn a_duration_the_file_cannot_parse_is_an_error() {
        assert!(parse("[phases]\nvote_limit = \"a while\"\n").is_err());
    }

    #[test]
    fn a_text_block_named_like_a_variable_is_an_error_naming_it() {
        for variable in VARIABLES {
            let file = format!("{STRATEGIES}[text]\n{variable} = \"Taken.\"\n");
            let error = parse(&file).unwrap_err();
            assert!(format!("{error:#}").contains(variable), "{error:#}");
        }
    }

    /// The name of the table `[roles.<name>]` that speaks of `role`: the
    /// role's name in lower case, such as `servant`.
    fn key(role: Role) -> String {
        role.to_string().to_lowercase()
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
            for (other, allotted) in &settings.lineup.allotments {
                let strategy = if *other == role { "dice" } else { RANDOM };
                assert!(allotted.contains_key(strategy), "{role}: {allotted:?}");
            }
        }
    }

    #[test]
    fn an_allotment_error_names_the_role_s_table_in_lower_case_and_the_role_as_written() {
        let file = format!("{STRATEGIES}[roles.servant.strategies]\ncoin = 1\n[roles.servant]\n");
        assert_eq!(
            unsettled(&format!("{file}count = 0\n")),
            "[roles.servant] count is 0, but [roles.servant.strategies] allots 1"
        );
        assert_eq!(
            unsettled(&format!("{file}strategy = \"wild\"\n")),
            "the Servant is allotted wild, which is not a strategy"
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
    fn the_role_counts_fall_back_from_the_file_to_the_code() {
        // The code alone.
        assert_eq!(table(&settle("").lineup), Table::default());
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
        assert_eq!(table(&settle(COUNTED).lineup), ten);
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
        assert_eq!(secs(&settle(file).limits), [90, 120, 180, 240]);
        // The code without the file.
        let code = Rules::default();
        assert_eq!(secs(&settle("").limits), secs(&code));
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
            .configuration(&settings.lineup.deal(), &[], Models::new())
            .unwrap();
        let random = || Strategy::Random(Random {});
        assert_eq!(
            configuration,
            Configuration {
                version: env!("CARGO_PKG_VERSION").to_string(),
                role_counts: Table::default().counts().into_iter().collect(),
                limits: Rules {
                    propose_limit: Duration::from_secs(30),
                    vote_limit: Duration::from_secs(90),
                    quest_limit: Duration::from_secs(10),
                    assassinate_limit: Duration::from_secs(5)
                },
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
            .configuration(&settings.lineup.deal(), &[], Models::new())
            .unwrap();
        let entry = Entry::Configuration(Box::new(configuration));
        let json = serde_json::to_value(&entry).unwrap();
        let configuration = &json["Configuration"];
        assert_eq!(configuration["version"], json!(env!("CARGO_PKG_VERSION")));
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
        let none = parse("").unwrap();
        let example = example("random.toml");
        for config in [none, example] {
            let settings = config.settle().unwrap();
            let deal = settings.lineup.deal();
            let configuration = settings.configuration(&deal, &[], Models::new()).unwrap();
            let (winner, won) = oneshot::channel();
            let (logger, log) = unbounded_channel();
            let episode = game(
                deal.clone(),
                settings.rules(),
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
                matches!(entries.last(), Some(Entry::Event(Outcome::End { .. }))),
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
        let (models, handles) = settings.set_up(&prompts, &Environment).await.unwrap();
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
            .set_up(&settings.prompts().unwrap(), &Environment)
            .await
            .err()
            .unwrap();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(
                "setting up the model qwen: qwen2.5-7b-instruct did not play Avalon: did not answer as the leader"
            ),
            "{shown}"
        );
        // A table of random players sets up nothing.
        let file = two_models("").replace("strategy = \"plain\"", "strategy = \"random\"");
        let settings = settle(&file);
        let (models, handles) = settings.set_up(&[], &Environment).await.unwrap();
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
        let plain = settings.needs("qwen", &prompts).unwrap();
        let smart = settings.needs("claude", &prompts).unwrap();
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
        let needed = |settings: &Settings| {
            settings
                .needs("qwen", &settings.prompts().unwrap())
                .unwrap()
        };
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
        assert_eq!(configuration.version, env!("CARGO_PKG_VERSION"));
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
            settings.rules(),
            configuration.clone(),
            winner,
            players,
            logger,
        );
        let (winner, entries, _) = run(episode, won, log).await.unwrap();
        assert!(matches!(winner, Team::Good | Team::Evil));
        assert!(
            matches!(entries.last(), Some(Entry::Event(Outcome::End { .. }))),
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
