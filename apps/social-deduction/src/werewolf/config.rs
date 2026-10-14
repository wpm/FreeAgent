//! The configuration of a game of Werewolf: the [`Config`] a TOML file is
//! parsed into, and the [`Settings`] that take effect once it is combined
//! with the defaults in the code. A setting comes from the file if it is
//! given there, otherwise from the code.
//!
//! The file is every game's [`crate::config::Config`], with Werewolf's
//! [`Phases`] and [`Roles`]. A prompted player's system prompt is rendered
//! for its seat and role, with the variables `name` and `role`, the role's
//! name.
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

use super::{Limits, Role, Rules, Table, Werewolf};
pub use crate::config::players;
use crate::strategy::RoleConfig;
use anyhow::bail;
use humantime::format_duration;
use serde::Deserialize;
use std::time::Duration;

/// The variables every template has besides the text blocks.
const VARIABLES: [&str; 2] = ["name", "role"];

/// The configuration file, as written.
pub type Config = crate::config::Config<Phases, Roles>;

/// Parse `file`, the text of a configuration file, as
/// [`crate::config::Config::parse`] parses it, with Werewolf's template
/// variables.
pub fn parse(file: &str) -> anyhow::Result<Config> {
    Config::parse(file, &VARIABLES)
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
pub type Allotments = crate::config::Allotments<Role>;

/// One player's system prompt, rendered for a seat dealt a role and a
/// prompted strategy.
pub type SystemPrompt = crate::config::SystemPrompt<Role>;

/// The settings that took effect.
pub type Settings = crate::config::Settings<Werewolf>;

impl Config {
    /// The settings that take effect: each limit from the file if given
    /// there, otherwise from the code; and each role's allotment, as
    /// [`crate::config::Config::allot`] allots it. A ratify limit longer
    /// than the day limit is an error naming both, since the day would run
    /// out before any vote stood.
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
        let lineup = self.allot(Table::default().counts(), Roles::get)?;
        Ok(Settings {
            limits: Limits {
                night_limit: self.phases.night_limit.unwrap_or(rules.night_limit),
                day_limit,
                ratify_limit,
            },
            lineup,
            models: self.models,
            text: self.text,
        })
    }
}

/// The configuration a game ran under.
pub type Configuration = crate::config::Configuration<Werewolf>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Lineup;
    use crate::game::Narrate;
    use crate::game::game;
    use crate::model::canned::{self, playing};
    use crate::model::fake::{FakeModel, choosing_first};
    use crate::model::spec::{Handles, ModelInPlay, Models};
    use crate::model::window::{ContextWindow, Source};
    use crate::model::{self, CONTEXT_WINDOW, REQUEST_TIMEOUT, ToolChoice};
    use crate::seat::PlayerId;
    use crate::strategy::Strategy;
    use crate::strategy::{Prompted, RANDOM, Random};
    use crate::werewolf::Dealt;
    use crate::werewolf::report::Narrator;
    use crate::werewolf::tests::{between_the_deal_and_the_end, logged, run};
    use crate::werewolf::{Entry, Message, Move, Outcome};
    use serde_json::json;
    use std::collections::HashSet;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;
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
        parse(file).unwrap().settle().unwrap()
    }

    /// The table `lineup` seats: how many of each role.
    fn table(lineup: &Lineup<Role>) -> Table {
        let counts = lineup.role_counts();
        let seats = |role| counts[&role];
        Table {
            werewolves: seats(Role::Werewolf),
            villagers: seats(Role::Villager),
            doctors: seats(Role::Doctor),
            seers: seats(Role::Seer),
        }
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
        let path = format!("{}/configs/werewolf/{name}", env!("CARGO_MANIFEST_DIR"));
        parse(&std::fs::read_to_string(path).unwrap()).unwrap()
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
            assert_eq!(table(&settings.lineup), Table::default(), "{name}");
            // The rules, where there are any, tell the players the ratify
            // limit, and that by day they may speak, vote, or wait, and
            // withdraw their vote.
            if let Some(rules) = settings.text.get("rules") {
                assert!(!prompts.is_empty(), "{name}");
                let ratify = format_duration(settings.limits.ratify_limit).to_string();
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
        let none = parse("").unwrap().settle().unwrap();
        let configured = |settings: &Settings| {
            let deal = settings.lineup.deal();
            settings.configuration(&deal, &[], Models::new()).unwrap()
        };
        assert_eq!(configured(&example), configured(&none));
        assert!(example.models_in_play().is_empty());
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
        let rules = Rules::default();
        assert_eq!(settings.limits.night_limit, rules.night_limit);
        assert_eq!(settings.limits.day_limit, rules.day_limit);
        assert_eq!(settings.limits.ratify_limit, Duration::ZERO);
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
            assert!(parse(&file).is_err(), "{file}");
        }
    }

    #[test]
    fn an_unknown_role_is_an_error() {
        let error = parse("[roles.hunter]\ncount = 1\n").unwrap_err();
        assert!(format!("{error:#}").contains("hunter"), "{error:#}");
    }

    #[test]
    fn a_duration_the_file_cannot_parse_is_an_error() {
        assert!(parse("[phases]\nnight_limit = \"a while\"\n").is_err());
    }

    #[test]
    fn a_text_block_named_like_a_variable_is_an_error_naming_it() {
        for variable in VARIABLES {
            let file = format!("{PLAIN}[text]\n{variable} = \"Taken.\"\n");
            let error = parse(&file).unwrap_err();
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
        assert_eq!(table(&settle("").lineup), Table::default());
        // The file over the code.
        assert_eq!(
            table(&settle(file).lineup),
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
        assert_eq!(settings.limits.night_limit, Duration::from_secs(90));
        assert_eq!(settings.limits.day_limit, Duration::from_secs(120));
        // The code without the file.
        let settings = settle("");
        assert_eq!(settings.limits.night_limit, Rules::default().night_limit);
        assert_eq!(settings.limits.day_limit, Rules::default().day_limit);
    }

    #[test]
    fn the_ratify_limit_falls_back_from_the_file_to_the_code() {
        let file = "[phases]\nratify_limit = \"15s\"\n";
        assert_eq!(settle(file).limits.ratify_limit, Duration::from_secs(15));
        assert_eq!(
            settle("").limits.ratify_limit,
            Rules::default().ratify_limit
        );
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
        assert_eq!(settle(file).limits.ratify_limit, Duration::from_secs(20));
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
            .lineup
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
        let needed = settings.needs("qwen", &long).unwrap();
        assert!(needed > settings.needs("qwen", &short).unwrap() + 3900);
        assert!(needed < settings.needs("qwen", &short).unwrap() + 4500);
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
                limits: Limits {
                    night_limit: Duration::from_secs(30),
                    day_limit: Duration::from_secs(90),
                    ratify_limit: Duration::from_secs(10)
                },
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
            .configuration(&settings.lineup.deal(), &[], Models::new())
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
        let deal = settings.lineup.deal();
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
                assert_eq!(settings.lineup.model_of(&deal[seat].strategy), Some(*name));
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
        let deal = settings.lineup.deal();
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
                message: Message::Move(Move::Select { .. }),
                seen,
                ..
            } = entry
            {
                assert_eq!(seen.is_some(), prompted.contains(from), "{from}");
                selected.insert(from);
            }
            if let Entry::Event(Outcome::Hunted { seq: 1, hunt }) = entry {
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
