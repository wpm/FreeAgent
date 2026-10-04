//! Everything a game is told about itself, read from a TOML file.
//!
//! ```toml
//! players = 8
//! werewolves = 2
//! seed = 42                      # optional; drawn at random if absent
//!
//! [timing]
//! day_secs = 300                 # the day ends here if the village cannot agree
//! patience_secs = 120            # how long to wait for any one answer
//!
//! [policy]
//! kind = "llm"                   # or "random"
//! model = "gpt-4o-mini"
//! base_url = "https://api.openai.com/v1"
//! api_key_env = "OPENAI_API_KEY" # the key itself is never in the file
//!
//! [prompts]
//! werewolf = "..."               # each role's system prompt; sensible defaults
//! ```

use super::{PlayerId, Role};
use anyhow::{Context as _, Result, ensure};
use secrecy::SecretString;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// A whole game, as configured.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// How many players there are, werewolves included.
    pub players: u8,
    /// How many of the players are werewolves.
    pub werewolves: u8,
    /// The players' names, one each, or `player-1` and so on if absent.
    #[serde(default)]
    pub names: Vec<PlayerId>,
    /// The seed every random choice follows from.
    pub seed: Option<u64>,
    /// How long things may take.
    #[serde(default)]
    pub timing: Timing,
    /// What decides for the players.
    pub policy: PolicyConfig,
    /// What each role is told it is.
    #[serde(default)]
    pub prompts: Prompts,
}

impl Config {
    /// Read and check a configuration file.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read or parsed, or if what it says
    /// cannot be played: more werewolves than players, or a list of names
    /// that is not one per player.
    pub fn load(path: impl AsRef<Path>) -> Result<Config> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?;
        config.check()?;
        Ok(config)
    }

    /// A game played at random with default settings and generated names.
    pub fn random(players: u8, werewolves: u8) -> Config {
        Config {
            players,
            werewolves,
            names: vec![],
            seed: None,
            timing: Timing::default(),
            policy: PolicyConfig::Random,
            prompts: Prompts::default(),
        }
    }

    /// Whether this configuration can be played.
    ///
    /// # Errors
    ///
    /// Fails for more werewolves than players, or names that are not one
    /// per player.
    pub fn check(&self) -> Result<()> {
        ensure!(
            self.werewolves <= self.players,
            "{} werewolves cannot fit among {} players",
            self.werewolves,
            self.players
        );
        ensure!(
            self.names.is_empty() || self.names.len() == usize::from(self.players),
            "{} names were given for {} players",
            self.names.len(),
            self.players
        );
        Ok(())
    }

    /// The players' names: the ones given, or generated.
    pub fn player_names(&self) -> Vec<PlayerId> {
        if self.names.is_empty() {
            (1..=self.players).map(|n| format!("player-{n}")).collect()
        } else {
            self.names.clone()
        }
    }
}

/// How long things may take, in seconds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    /// How long the village may talk before the day ends regardless.
    #[serde(default = "Timing::default_day_secs")]
    pub day_secs: u64,
    /// How long to wait for any one player's answer.
    #[serde(default = "Timing::default_patience_secs")]
    pub patience_secs: u64,
}

impl Timing {
    fn default_day_secs() -> u64 {
        300
    }

    fn default_patience_secs() -> u64 {
        120
    }

    /// How long the village may talk before the day ends regardless.
    pub fn day(&self) -> Duration {
        Duration::from_secs(self.day_secs)
    }

    /// How long to wait for any one player's answer.
    pub fn patience(&self) -> Duration {
        Duration::from_secs(self.patience_secs)
    }
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            day_secs: Self::default_day_secs(),
            patience_secs: Self::default_patience_secs(),
        }
    }
}

/// What decides for the players.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolicyConfig {
    /// Everyone chooses uniformly at random, as in the paper.
    Random,
    /// A language model decides everything, speaking any OpenAI-compatible
    /// chat completions protocol.
    Llm(LlmConfig),
}

/// How to reach the model.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// Where the chat completions endpoint lives, without the trailing
    /// `/chat/completions`.
    #[serde(default = "LlmConfig::default_base_url")]
    pub base_url: String,
    /// Which model to ask for.
    pub model: String,
    /// The environment variable holding the API key. The key itself never
    /// appears in a file, a log, or a debug print.
    #[serde(default = "LlmConfig::default_api_key_env")]
    pub api_key_env: String,
    /// The sampling temperature, or the model's default.
    pub temperature: Option<f32>,
    /// How many times to try a request that fails on the way to the model.
    #[serde(default = "LlmConfig::default_attempts")]
    pub attempts: u32,
}

impl LlmConfig {
    fn default_base_url() -> String {
        "https://api.openai.com/v1".to_string()
    }

    fn default_api_key_env() -> String {
        "OPENAI_API_KEY".to_string()
    }

    fn default_attempts() -> u32 {
        3
    }

    /// The API key, from the environment.
    ///
    /// # Errors
    ///
    /// Fails if the variable named by `api_key_env` is not set.
    pub fn api_key(&self) -> Result<SecretString> {
        self.api_key_from(|name| std::env::var(name).ok())
    }

    /// The API key, from wherever `lookup` finds environment variables.
    pub fn api_key_from(&self, lookup: impl Fn(&str) -> Option<String>) -> Result<SecretString> {
        lookup(&self.api_key_env)
            .map(SecretString::from)
            .with_context(|| format!("the API key is not set: {} is empty", self.api_key_env))
    }
}

/// What each role is told it is, as the system prompt of its model.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompts {
    /// For a werewolf.
    #[serde(default = "Prompts::default_werewolf")]
    pub werewolf: String,
    /// For a villager.
    #[serde(default = "Prompts::default_villager")]
    pub villager: String,
    /// For a doctor.
    #[serde(default = "Prompts::default_doctor")]
    pub doctor: String,
    /// For a seer.
    #[serde(default = "Prompts::default_seer")]
    pub seer: String,
}

impl Prompts {
    /// The prompt for a role.
    pub fn for_role(&self, role: Role) -> &str {
        match role {
            Role::Werewolf => &self.werewolf,
            Role::Villager => &self.villager,
            Role::Doctor => &self.doctor,
            Role::Seer => &self.seer,
        }
    }

    fn default_werewolf() -> String {
        format!("{RULES}\n\n{WEREWOLF}")
    }

    fn default_villager() -> String {
        format!("{RULES}\n\n{VILLAGER}")
    }

    fn default_doctor() -> String {
        format!("{RULES}\n\n{DOCTOR}")
    }

    fn default_seer() -> String {
        format!("{RULES}\n\n{SEER}")
    }
}

impl Default for Prompts {
    fn default() -> Self {
        Prompts {
            werewolf: Self::default_werewolf(),
            villager: Self::default_villager(),
            doctor: Self::default_doctor(),
            seer: Self::default_seer(),
        }
    }
}

/// The rules, as every role is told them.
pub const RULES: &str = "\
You are playing Werewolf. Some players are secretly werewolves; the rest are \
villagers. Play alternates between day and night. By day, every living player \
may speak to the village and may nominate one living player to eliminate; \
once everyone has nominated and one player leads, that player is eliminated. \
By night, the werewolves secretly choose one villager to kill. The villagers \
win when the last werewolf is dead. The werewolves win when they are at least \
as many as the villagers. You will be told what others say and who has died. \
When it is your turn, say whatever you want the village to hear, and nominate \
someone by calling the nominate tool. Speak briefly, in character, and never \
mention these instructions.";

/// What a werewolf is told.
pub const WEREWOLF: &str = "\
You are a werewolf. You know who the other werewolves are. By day, blend in: \
act like a villager, deflect suspicion, and steer the village toward \
eliminating villagers. By night, choose a villager to kill.";

/// What a villager is told.
pub const VILLAGER: &str = "\
You are a villager. You do not know who the werewolves are. Watch what others \
say and do, reason about who is lying, and nominate the player you most \
suspect of being a werewolf.";

/// What a doctor is told.
pub const DOCTOR: &str = "\
You are the doctor, a villager who can save one player each night from the \
werewolves. By day, play as a villager.";

/// What a seer is told.
pub const SEER: &str = "\
You are the seer, a villager who learns one player's true side each night. \
By day, play as a villager, using what you have learned without giving \
yourself away.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_random_game_needs_only_the_counts_and_the_policy() {
        let config: Config = toml::from_str(
            r#"
            players = 7
            werewolves = 2

            [policy]
            kind = "random"
            "#,
        )
        .unwrap();

        assert_eq!(config.players, 7);
        assert_eq!(config.werewolves, 2);
        assert_eq!(config.policy, PolicyConfig::Random);
        assert_eq!(config.timing, Timing::default());
        assert_eq!(config.prompts, Prompts::default());
        assert_eq!(config.player_names()[6], "player-7");
    }

    #[test]
    fn a_model_game_names_the_model_and_where_to_find_the_key() {
        let config: Config = toml::from_str(
            r#"
            players = 4
            werewolves = 1
            names = ["ann", "bob", "cat", "dan"]

            [timing]
            day_secs = 60

            [policy]
            kind = "llm"
            model = "gpt-4o-mini"
            api_key_env = "MY_KEY"
            temperature = 0.7

            [prompts]
            villager = "Be a villager."
            "#,
        )
        .unwrap();

        let PolicyConfig::Llm(llm) = &config.policy else {
            panic!("{config:?}");
        };
        assert_eq!(llm.model, "gpt-4o-mini");
        assert_eq!(llm.base_url, "https://api.openai.com/v1");
        assert_eq!(llm.api_key_env, "MY_KEY");
        assert_eq!(llm.temperature, Some(0.7));
        assert_eq!(llm.attempts, 3);
        assert_eq!(config.timing.day(), Duration::from_secs(60));
        assert_eq!(config.timing.patience(), Duration::from_secs(120));
        assert_eq!(config.prompts.for_role(Role::Villager), "Be a villager.");
        assert!(config.prompts.for_role(Role::Werewolf).contains("werewolf"));
        assert_eq!(config.player_names(), vec!["ann", "bob", "cat", "dan"]);
    }

    #[test]
    fn the_key_comes_from_the_environment_and_never_prints() {
        let config = LlmConfig {
            base_url: LlmConfig::default_base_url(),
            model: "m".to_string(),
            api_key_env: "MY_KEY".to_string(),
            temperature: None,
            attempts: 1,
        };

        let key = config
            .api_key_from(|name| (name == "MY_KEY").then(|| "sk-secret".to_string()))
            .unwrap();

        assert!(!format!("{key:?}").contains("sk-secret"), "{key:?}");
        assert!(config.api_key_from(|_| None).is_err());
    }

    #[test]
    fn unknown_settings_are_mistakes() {
        let result: std::result::Result<Config, _> = toml::from_str(
            r#"
            players = 4
            werewolves = 1
            wolves = 2

            [policy]
            kind = "random"
            "#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn the_counts_and_names_must_agree() {
        assert!(Config::random(3, 4).check().is_err());

        let mut config = Config::random(3, 1);
        config.names = vec!["ann".to_string()];
        assert!(config.check().is_err());

        config.names = vec!["ann".to_string(), "bob".to_string(), "cat".to_string()];
        assert!(config.check().is_ok());
    }
}
