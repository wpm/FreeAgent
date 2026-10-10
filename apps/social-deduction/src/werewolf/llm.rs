//! The model-played variant: every player is a language model, set up by
//! a TOML configuration file.
//!
//! The game itself is not playable yet. What is here is its
//! configuration: the [`Config`] the file is parsed into, the
//! [`Overrides`] the command line may give, and the [`Settings`] that take
//! effect once the two are combined with the defaults in the code. A
//! setting comes from the command line if it is given there, otherwise
//! from the file, otherwise from the code. The templates the file holds are
//! kept as written; rendering them comes later.
//!
//! The API key never appears in the file. The file names the environment
//! variable that holds it, and [`Model::api_key`] reads that variable into
//! a [`SecretString`], which is redacted wherever it is shown.

use super::{RoleCounts, Table};
use anyhow::Context;
use clap::Args;
use secrecy::SecretString;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

/// A minute: how long a phase waits and a request may take unless told
/// otherwise.
const MINUTE: Duration = Duration::from_secs(60);

/// What the command line may say over the file: the role counts and the
/// phase limits. Each is unset unless given, so that the file can be heard.
#[derive(Args, Debug, Default)]
pub struct Overrides {
    /// How many of each role sit at the table.
    #[command(flatten)]
    pub roles: RoleCounts,
    /// How long the night waits for a player [default: 1m]
    #[arg(long, value_parser = humantime::parse_duration)]
    pub night_limit: Option<Duration>,
    /// How long the day waits for a player [default: 1m]
    #[arg(long, value_parser = humantime::parse_duration)]
    pub day_limit: Option<Duration>,
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
    /// The environment variable holding the API key, for a provider that
    /// wants one.
    pub api_key_env: Option<String>,
    /// How long a request may take.
    #[serde(default = "minute", with = "humantime_serde")]
    pub request_timeout: Duration,
}

impl Model {
    /// The API key, read from the environment variable `api_key_env`
    /// names. A variable that is named but not set is an error. When none
    /// is named there is no key, which is what a local server expects.
    pub fn api_key(&self) -> anyhow::Result<Option<SecretString>> {
        self.api_key_env
            .as_deref()
            .map(|name| {
                std::env::var(name)
                    .map(SecretString::from)
                    .with_context(|| format!("api_key_env names {name}, which is not set"))
            })
            .transpose()
    }
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

/// The settings that took effect: the file with every choice made, and
/// the command line and the code heard where the file was silent.
#[derive(Debug)]
pub struct Settings {
    /// The model every player uses and how to reach it.
    pub model: Model,
    /// How many of each role sit at the table.
    pub table: Table,
    /// How long the night waits for a player.
    pub night_limit: Duration,
    /// How long the day waits for a player.
    pub day_limit: Duration,
    /// What each role is told, kept as written.
    pub roles: Roles,
    /// Named blocks of text for the templates to use, kept as written.
    pub text: BTreeMap<String, String>,
    /// The templates every role falls back on, kept as written.
    pub prompt: Prompt,
    /// What each seat is like, kept as written.
    pub personas: BTreeMap<String, String>,
}

impl Config {
    /// Read and parse the file at `path`. Reading it says nothing about
    /// the API key, so a file can be checked without the key being set.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&file).with_context(|| format!("in {}", path.display()))
    }

    /// Parse `file`, the text of a configuration file.
    pub fn parse(file: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(file)?)
    }

    /// The settings that take effect with `overrides` from the command
    /// line: each role count and phase limit from the command line if
    /// given there, otherwise from the file, otherwise from the code.
    pub fn settle(self, overrides: Overrides) -> Settings {
        let file = RoleCounts {
            werewolves: self.roles.werewolf.count,
            villagers: self.roles.villager.count,
            doctors: self.roles.doctor.count,
            seers: self.roles.seer.count,
        };
        Settings {
            model: self.model,
            table: overrides.roles.or(file.or(Table::default())),
            night_limit: overrides
                .night_limit
                .or(self.phases.night_limit)
                .unwrap_or(MINUTE),
            day_limit: overrides
                .day_limit
                .or(self.phases.day_limit)
                .unwrap_or(MINUTE),
            roles: self.roles,
            text: self.text,
            prompt: self.prompt,
            personas: self.personas,
        }
    }
}

impl fmt::Display for Settings {
    /// The settings a game would be played under, one per line, with the
    /// key's variable named and the key itself never shown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let model = &self.model;
        writeln!(f, "model: {} at {}", model.id, model.base_url)?;
        writeln!(
            f,
            "request timeout: {}",
            humantime::format_duration(model.request_timeout)
        )?;
        match &model.api_key_env {
            Some(name) => writeln!(f, "API key: from {name}")?,
            None => writeln!(f, "API key: none")?,
        }
        writeln!(f, "roles: {}", self.table)?;
        writeln!(
            f,
            "night limit: {}",
            humantime::format_duration(self.night_limit)
        )?;
        writeln!(
            f,
            "day limit: {}",
            humantime::format_duration(self.day_limit)
        )
    }
}

/// The default `request_timeout`, for `serde`.
fn minute() -> Duration {
    MINUTE
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
    fn the_examples_parse() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/werewolf/llm");
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();
        assert_eq!(files.len(), 2, "{files:?}");
        for file in files {
            Config::load(&file).unwrap_or_else(|e| panic!("{}: {e:#}", file.display()));
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
        // The command line over the file, one role at a time.
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
            ..RoleCounts::default()
        };
        assert_eq!(given(counts).werewolves, 1);
        let counts = RoleCounts {
            villagers: Some(2),
            ..RoleCounts::default()
        };
        assert_eq!(given(counts).villagers, 2);
        let counts = RoleCounts {
            doctors: Some(3),
            ..RoleCounts::default()
        };
        assert_eq!(given(counts).doctors, 3);
        let counts = RoleCounts {
            seers: Some(4),
            ..RoleCounts::default()
        };
        // A count the command line does not give still comes from the file.
        assert_eq!(
            given(counts),
            Table {
                werewolves: 5,
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
        assert_eq!(settings.roles.seer.system.as_deref(), Some("You see."));
    }

    #[test]
    fn the_phase_limits_fall_back_from_the_command_line_to_the_file_to_the_code() {
        let file = format!("{MODEL}\n[phases]\nnight_limit = \"1m 30s\"\nday_limit = \"2m\"\n");
        let minute = Duration::from_secs(60);
        // The code alone.
        let settings = settle(MODEL, Overrides::default());
        assert_eq!(settings.night_limit, minute);
        assert_eq!(settings.day_limit, minute);
        // The file over the code.
        let settings = settle(&file, Overrides::default());
        assert_eq!(settings.night_limit, Duration::from_secs(90));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
        // The command line over the file, one limit at a time.
        let settings = settle(
            &file,
            Overrides {
                night_limit: Some(Duration::from_secs(5)),
                ..Overrides::default()
            },
        );
        assert_eq!(settings.night_limit, Duration::from_secs(5));
        assert_eq!(settings.day_limit, Duration::from_secs(120));
        let settings = settle(
            &file,
            Overrides {
                day_limit: Some(Duration::from_secs(7)),
                ..Overrides::default()
            },
        );
        assert_eq!(settings.night_limit, Duration::from_secs(90));
        assert_eq!(settings.day_limit, Duration::from_secs(7));
    }

    #[test]
    fn the_command_line_takes_durations_in_humantime_form() {
        let variant = Variant::parse_from(["llm", "--night-limit", "30s", "--day-limit", "1m 30s"]);
        assert_eq!(variant.overrides.night_limit, Some(Duration::from_secs(30)));
        assert_eq!(variant.overrides.day_limit, Some(Duration::from_secs(90)));
        assert!(Variant::try_parse_from(["llm", "--night-limit", "soon"]).is_err());
        assert!(Variant::try_parse_from(["llm", "--day-limit", "30"]).is_err());
    }

    #[test]
    fn the_request_timeout_comes_from_the_file_or_the_code() {
        let minute = Duration::from_secs(60);
        assert_eq!(
            settle(MODEL, Overrides::default()).model.request_timeout,
            minute
        );
        let file = format!("{MODEL}request_timeout = \"10s\"\n");
        assert_eq!(
            settle(&file, Overrides::default()).model.request_timeout,
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
        let settings = settle(&file, Overrides::default());
        assert_eq!(settings.text["rules"], "The rules.");
        assert_eq!(
            settings.prompt.system.as_deref(),
            Some("{{ rules }} You are {{ name }}.")
        );
        assert_eq!(settings.roles.werewolf.system.as_deref(), Some("Howl."));
        assert_eq!(settings.roles.villager.system, None);
        assert_eq!(settings.personas["player1"], "Cautious.");
    }

    #[test]
    fn an_omitted_key_variable_means_no_key() {
        let settings = settle(MODEL, Overrides::default());
        assert!(settings.model.api_key().unwrap().is_none());
    }

    #[test]
    fn a_named_key_variable_that_is_not_set_is_an_error_naming_it() {
        let file = format!("{MODEL}api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"\n");
        let error = settle(&file, Overrides::default())
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
        // Any variable that is set will do; setting one in a test is unsafe.
        let (name, value) = std::env::vars()
            .find(|(name, value)| {
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && value.len() > 3
            })
            .unwrap();
        let file = format!("{MODEL}api_key_env = \"{name}\"\n");
        let settings = settle(&file, Overrides::default());
        let key = settings.model.api_key().unwrap().unwrap();
        assert_eq!(key.expose_secret(), value);
        let shown = format!("{key:?}\n{settings:?}\n{settings}");
        assert!(shown.contains(&name), "{shown}");
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
}
