//! The model-played variant: every player is a language model, set up by
//! a TOML configuration file.
//!
//! The game itself is not playable yet. What is here is its
//! configuration: the [`Config`] the file is parsed into, the
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

use super::{PhaseLimits, PlayerId, Role, RoleCounts, Rules, Table};
use crate::model::{Provider, REQUEST_TIMEOUT, api_key, key_variable};
use anyhow::{Context, anyhow, bail};
use clap::Args;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

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

impl fmt::Display for SystemPrompt {
    /// The prompt under a header naming its seat and role.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "--- {} as {} ---", self.seat, self.role)?;
        writeln!(f, "{}", self.text)
    }
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
}

impl fmt::Display for Settings {
    /// The settings a game would be played under, one per line, with the
    /// key's variable named and the key itself never shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let model = &self.config.model;
        let timeout = humantime::format_duration(model.request_timeout);
        let night = humantime::format_duration(self.night_limit);
        let day = humantime::format_duration(self.day_limit);
        writeln!(f, "model: {} at {}", model.id, model.base_url)?;
        writeln!(f, "request timeout: {timeout}")?;
        match model.key_variable() {
            Some(name) => writeln!(f, "API key: from {name}")?,
            None => writeln!(f, "API key: none")?,
        }
        writeln!(f, "roles: {}", self.table)?;
        writeln!(f, "night limit: {night}")?;
        writeln!(f, "day limit: {day}")
    }
}

/// The default `request_timeout`, for `serde`.
fn request_timeout() -> Duration {
    REQUEST_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use secrecy::ExposeSecret;

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
        assert!(
            settings
                .to_string()
                .starts_with("model: gpt-5.5 at https://api.openai.com/v1\n"),
            "{settings}"
        );
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
    fn a_prompt_is_shown_under_a_header_naming_its_seat_and_role() {
        let prompt = SystemPrompt {
            seat: "player3".into(),
            role: Role::Seer,
            text: "You see.\nSay so.".into(),
        };
        assert_eq!(
            prompt.to_string(),
            "--- player3 as seer ---\nYou see.\nSay so.\n"
        );
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
        let shown = format!("{key:?}\n{settings:?}\n{settings}");
        assert!(shown.contains(name), "{shown}");
        assert!(!shown.contains(&value), "{shown}");
    }

    #[test]
    fn the_settings_are_printed_one_per_line() {
        let file = format!(
            "{MODEL}api_key_env = \"OPENAI_API_KEY\"\n[phases]\nnight_limit = \"30s\"\nday_limit = \"1m 30s\"\n"
        );
        assert_eq!(
            settle(&file, Overrides::default()).to_string(),
            "\
model: qwen2.5-7b-instruct at http://localhost:1234/v1
request timeout: 1m
API key: from OPENAI_API_KEY
roles: 2 werewolves, 3 villagers, 1 doctor, 1 seer
night limit: 30s
day limit: 1m 30s
"
        );
        let settings = settle(MODEL, Overrides::default());
        assert!(
            settings.to_string().contains("API key: none\n"),
            "{settings}"
        );
    }

    #[test]
    fn a_known_provider_s_key_variable_is_filled_in_unless_the_file_names_one() {
        let openai = MODEL.replace("http://localhost:1234/v1", "https://api.openai.com/v1");
        let settings = settle(&openai, Overrides::default());
        assert_eq!(settings.config.model.key_variable(), Some("OPENAI_API_KEY"));
        assert!(
            settings
                .to_string()
                .contains("API key: from OPENAI_API_KEY\n")
        );
        let named = format!("{openai}api_key_env = \"MY_KEY\"\n");
        let settings = settle(&named, Overrides::default());
        assert_eq!(settings.config.model.key_variable(), Some("MY_KEY"));
    }
}
