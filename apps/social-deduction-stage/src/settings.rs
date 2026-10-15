//! The desktop app's settings: the folder logs go in, the look, the models
//! both games share, and each game's table, phases, strategies and text.
//! The backend stores them and checks them, and the Settings window edits
//! them, so they cross between the two as they are. The providers a model
//! is chosen from, the models each one lists, what probing a model found
//! and where an API key is cross the same way. None of them ever carries a
//! key.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::GameName;

/// The strategy that is always defined, with the random policy.
pub const RANDOM: &str = "random";

/// Everything the app's settings hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// What belongs to neither game.
    pub general: General,
    /// The models, by name, which both games' strategies choose from.
    pub models: BTreeMap<String, ModelSettings>,
    /// Werewolf's settings.
    pub werewolf: GameSettings,
    /// Avalon's settings.
    pub avalon: GameSettings,
}

impl AppSettings {
    /// The settings of `game`.
    pub fn game(&self, game: GameName) -> &GameSettings {
        match game {
            GameName::Werewolf => &self.werewolf,
            GameName::Avalon => &self.avalon,
        }
    }

    /// The settings of `game`, to change.
    pub fn game_mut(&mut self, game: GameName) -> &mut GameSettings {
        match game {
            GameName::Werewolf => &mut self.werewolf,
            GameName::Avalon => &mut self.avalon,
        }
    }

    /// One line saying what a game of `game` is: `7 players · 1 strategy
    /// · gpt-4.1-mini`, with the ids of the models in play, if any.
    pub fn summary(&self, game: GameName) -> String {
        let settings = self.game(game);
        let players = settings.players();
        let at_the_table = settings.at_the_table();
        let mut parts = vec![
            plural(players, "player", "players"),
            plural(at_the_table.len(), "strategy", "strategies"),
        ];
        let ids: BTreeSet<&str> = at_the_table
            .iter()
            .filter_map(|name| settings.model_of(name))
            .map(|name| self.models.get(name).map_or(name, |model| &model.id))
            .collect();
        if !ids.is_empty() {
            parts.push(ids.into_iter().collect::<Vec<_>>().join(", "));
        }
        parts.join(" · ")
    }

    /// The errors among `errors` that stop a game of `game`: its own, and
    /// those of the models its strategies name.
    pub fn errors_of<'a>(
        &self,
        game: GameName,
        errors: &'a [FieldError],
    ) -> impl Iterator<Item = &'a FieldError> + use<'a> {
        let key = game_key(game);
        let settings = self.game(game);
        let models: BTreeSet<String> = settings
            .strategies
            .keys()
            .filter_map(|name| settings.model_of(name))
            .map(String::from)
            .collect();
        errors.iter().filter(move |error| match &error.path[..] {
            [first, ..] if first == key => true,
            [first, name, ..] if first == "models" => models.contains(name),
            _ => false,
        })
    }
}

/// `count` and the noun for it, `one` or `many`.
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The key a game's settings are under, and its errors' paths start
/// with: `werewolf` or `avalon`.
pub fn game_key(game: GameName) -> &'static str {
    match game {
        GameName::Werewolf => "werewolf",
        GameName::Avalon => "avalon",
    }
}

/// A game's name as the app shows it: `Werewolf` or `Avalon`.
pub fn game_title(game: GameName) -> &'static str {
    match game {
        GameName::Werewolf => "Werewolf",
        GameName::Avalon => "Avalon",
    }
}

/// What belongs to neither game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// The folder game logs are saved in, when it is not the app's own.
    pub logs: Option<String>,
    /// How the app looks.
    pub appearance: Appearance,
    /// The game chosen to play.
    pub game: GameName,
}

impl Default for General {
    /// Logs in the app's folder, the system's look, and Werewolf.
    fn default() -> Self {
        Self {
            logs: None,
            appearance: Appearance::System,
            game: GameName::Werewolf,
        }
    }
}

/// How the app looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the system looks, and Werewolf's game dark by night and light by
    /// day.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Appearance {
    /// The page's `data-theme`, which the look holds to whatever the game
    /// shows, or `None` for the system's.
    pub fn theme(self) -> Option<&'static str> {
        match self {
            Appearance::System => None,
            Appearance::Light => Some("light"),
            Appearance::Dark => Some("dark"),
        }
    }
}

/// One model from one provider, and how to reach it: a configuration
/// file's `[models.<name>]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettings {
    /// The root of an OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The model's id, exactly as the provider lists it.
    pub id: String,
    /// The variable holding the API key, over the one known for the
    /// provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// The model's context window, in tokens, over what the provider
    /// reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// How long a request may take, such as `60s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<String>,
}

/// One game's settings: everything its configuration holds but the
/// models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// The strategy every role plays that is not allotted otherwise.
    pub strategy: String,
    /// The game's roles, in the order they are dealt.
    pub roles: Vec<RoleSettings>,
    /// The game's phase limits, in the order the phases come.
    pub phases: Vec<PhaseSettings>,
    /// The strategies, by name, besides [`RANDOM`].
    pub strategies: BTreeMap<String, StrategySettings>,
    /// Named blocks of text for the prompts to use.
    pub text: BTreeMap<String, String>,
}

impl Default for GameSettings {
    /// No roles, no phases, and everyone random.
    fn default() -> Self {
        Self {
            strategy: RANDOM.to_string(),
            roles: Vec::new(),
            phases: Vec::new(),
            strategies: BTreeMap::new(),
            text: BTreeMap::new(),
        }
    }
}

impl GameSettings {
    /// How many players sit at the table.
    pub fn players(&self) -> usize {
        self.roles.iter().map(|role| role.count).sum()
    }

    /// The names of the strategies allotted a seat, as a game allots them:
    /// a role's `strategies` first, then its own strategy for the seats
    /// left, and a role with neither plays the game's strategy.
    pub fn at_the_table(&self) -> BTreeSet<&str> {
        let mut names = BTreeSet::new();
        for role in self.roles.iter().filter(|role| role.count > 0) {
            let allotted: usize = role.strategies.values().sum();
            names.extend(
                role.strategies
                    .iter()
                    .filter(|(_, seats)| **seats > 0)
                    .map(|(name, _)| name.as_str()),
            );
            match &role.strategy {
                Some(strategy) if role.count > allotted => {
                    names.insert(strategy.as_str());
                }
                None if role.strategies.is_empty() => {
                    names.insert(self.strategy.as_str());
                }
                _ => {}
            }
        }
        names
    }

    /// The name of the model the strategy `name` asks, if it is defined
    /// and prompted.
    pub fn model_of(&self, name: &str) -> Option<&str> {
        let strategy = self.strategies.get(name)?;
        (strategy.policy == Policy::Prompted).then_some(strategy.model.as_str())
    }
}

/// One role's seats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSettings {
    /// The role's key: `werewolf`, `merlin`.
    pub key: String,
    /// How many of the role sit at the table.
    pub count: usize,
    /// The strategy the role's seats play that `strategies` does not
    /// allot, over the game's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    /// How many seats of the role play each strategy, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub strategies: BTreeMap<String, usize>,
}

/// One phase's limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseSettings {
    /// The phase's key: `day_limit`.
    pub key: String,
    /// How long the phase waits, as written: `45s`, `2m`.
    pub limit: String,
}

/// A strategy: a policy and its parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategySettings {
    /// What the strategy's players do.
    pub policy: Policy,
    /// The name of the model a prompted strategy asks.
    pub model: String,
    /// A prompted strategy's system prompt template.
    pub system: String,
}

/// What a strategy's players do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Ask a language model.
    #[default]
    Prompted,
    /// Choose at random.
    Random,
}

/// What is wrong with one field of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// Where the field is: `["werewolf", "phases", "day_limit"]`,
    /// `["models", "gpt", "id"]`.
    pub path: Vec<String>,
    /// What is wrong, in a sentence that stands alone.
    pub message: String,
}

impl FieldError {
    /// The error `message` at `path`.
    pub fn new(path: &[&str], message: impl Into<String>) -> Self {
        Self {
            path: path.iter().map(ToString::to_string).collect(),
            message: message.into(),
        }
    }

    /// Whether the error is at `path`.
    pub fn is_at(&self, path: &[String]) -> bool {
        self.path == path
    }
}

/// The settings as the backend holds them, and what is wrong with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The settings.
    pub settings: AppSettings,
    /// Every field's error.
    pub errors: Vec<FieldError>,
    /// The folder game logs are saved in.
    pub logs: String,
}

/// A provider a model may be reached through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// What the app calls it: `OpenAI`, `Local (localhost:1234)`.
    pub name: String,
    /// The root of its OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The variable that conventionally holds its API key, if it wants one.
    pub api_key_env: Option<String>,
}

/// The models a provider lists, or why it could not list them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Listing {
    /// Each model the provider serves, in the order it lists them.
    pub models: Vec<Listed>,
    /// Why the provider could not list its models.
    pub error: Option<String>,
    /// What probing found of each model, by id.
    pub checks: BTreeMap<String, Check>,
}

/// One model a provider lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// Its id.
    pub id: String,
    /// Its context window, in tokens, when the provider reports one.
    pub context_window: Option<u64>,
    /// Whether its id says it is not a model that talks, such as an
    /// embedding model, which is never probed.
    pub hidden: bool,
}

/// What probing one model found, or `None` for a probe that could not be
/// run, sent as each probe ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// The provider's base URL.
    pub base_url: String,
    /// The model's id.
    pub id: String,
    /// What the probe found.
    pub check: Option<Check>,
}

/// How far probing a provider's models has got, sent as each probe ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The provider's base URL.
    pub base_url: String,
    /// How many models have been probed.
    pub checked: usize,
    /// How many models there are to probe.
    pub total: usize,
}

impl Progress {
    /// The line under a provider: `Checked 14 of 31`.
    pub fn told(&self) -> String {
        format!("Checked {} of {}", self.checked, self.total)
    }
}

/// Where a key variable's key is, never the key itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyStatus {
    /// In the OS keychain.
    InKeychain,
    /// In the environment variable of its name.
    InEnvironment,
    /// Nowhere.
    Missing,
    /// The keychain could not be asked, and why.
    Unavailable(String),
}

impl KeyStatus {
    /// What a provider says of its key.
    pub fn told(&self) -> String {
        match self {
            KeyStatus::InKeychain => "in the keychain".to_string(),
            KeyStatus::InEnvironment => "in the environment".to_string(),
            KeyStatus::Missing => "not set".to_string(),
            KeyStatus::Unavailable(why) => why.clone(),
        }
    }

    /// Whether there is a key to send.
    pub fn is_set(&self) -> bool {
        matches!(self, KeyStatus::InKeychain | KeyStatus::InEnvironment)
    }
}

/// What asking a model to play found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    /// It plays.
    Plays,
    /// It plays only when not made to call a tool.
    PlaysUnforced,
    /// It did not play, and why.
    DidNotPlay(String),
}

impl Check {
    /// The model's mark: `✓`, `~` or `✗`.
    pub fn mark(&self) -> &'static str {
        match self {
            Check::Plays => "✓",
            Check::PlaysUnforced => "~",
            Check::DidNotPlay(_) => "✗",
        }
    }

    /// What the mark means.
    pub fn told(&self) -> String {
        match self {
            Check::Plays => "plays".to_string(),
            Check::PlaysUnforced => "plays only when not made to call a tool".to_string(),
            Check::DidNotPlay(why) => format!("doesn't play: {why}"),
        }
    }

    /// Whether the model plays, made to call a tool or not.
    pub fn plays(&self) -> bool {
        !matches!(self, Check::DidNotPlay(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A role `key` of `count` seats, playing `strategy` when it names one.
    fn role(key: &str, count: usize, strategy: Option<&str>) -> RoleSettings {
        RoleSettings {
            key: key.to_string(),
            count,
            strategy: strategy.map(String::from),
            strategies: BTreeMap::new(),
        }
    }

    /// A prompted strategy asking `model`.
    fn prompted(model: &str) -> StrategySettings {
        StrategySettings {
            policy: Policy::Prompted,
            model: model.to_string(),
            system: "Play.".to_string(),
        }
    }

    /// Settings whose Werewolf seats five players, two playing `smart`
    /// and three `everyone`, both prompted, asking `gpt` and `local`.
    fn two_strategies() -> AppSettings {
        let model = |id: &str| ModelSettings {
            id: id.to_string(),
            ..ModelSettings::default()
        };
        AppSettings {
            models: BTreeMap::from([
                ("gpt".to_string(), model("gpt-4.1-mini")),
                ("local".to_string(), model("qwen")),
            ]),
            werewolf: GameSettings {
                strategy: "everyone".to_string(),
                roles: vec![
                    role("werewolf", 2, Some("smart")),
                    role("villager", 3, None),
                ],
                strategies: BTreeMap::from([
                    ("everyone".to_string(), prompted("local")),
                    ("smart".to_string(), prompted("gpt")),
                    ("idle".to_string(), prompted("missing")),
                ]),
                ..GameSettings::default()
            },
            ..AppSettings::default()
        }
    }

    #[test]
    fn a_game_s_summary_counts_its_players_and_strategies_and_names_its_models() {
        let settings = two_strategies();
        assert_eq!(
            settings.summary(GameName::Werewolf),
            "5 players · 2 strategies · gpt-4.1-mini, qwen"
        );
        let mut settings = AppSettings::default();
        settings.avalon.roles = vec![role("merlin", 1, None)];
        assert_eq!(settings.summary(GameName::Avalon), "1 player · 1 strategy");
        // A model that is not defined is named as the strategy names it.
        settings.avalon.strategy = "lost".to_string();
        settings
            .avalon
            .strategies
            .insert("lost".to_string(), prompted("ghost"));
        assert_eq!(
            settings.summary(GameName::Avalon),
            "1 player · 1 strategy · ghost"
        );
    }

    #[test]
    fn a_role_s_strategies_come_first_and_its_own_strategy_fills_the_rest() {
        let mut settings = GameSettings::default();
        let mut servant = role("servant", 3, Some("filler"));
        servant.strategies = BTreeMap::from([("table".to_string(), 2), ("none".to_string(), 0)]);
        let mut minion = role("minion", 1, Some("unused"));
        minion.strategies = BTreeMap::from([("table".to_string(), 1)]);
        let mut oberon = role("oberon", 1, None);
        oberon.strategies = BTreeMap::from([("only".to_string(), 1)]);
        settings.roles = vec![servant, minion, oberon, role("morgana", 0, Some("absent"))];
        let names: Vec<&str> = settings.at_the_table().into_iter().collect();
        assert_eq!(names, ["filler", "only", "table"]);
    }

    #[test]
    fn a_game_s_errors_are_its_own_and_those_of_the_models_it_names() {
        let settings = two_strategies();
        let errors = [
            FieldError::new(&["werewolf", "phases", "day_limit"], "a"),
            FieldError::new(&["avalon", "phases", "vote_limit"], "b"),
            FieldError::new(&["models", "gpt", "id"], "c"),
            FieldError::new(&["models", "missing", "id"], "d"),
            FieldError::new(&["models", "other", "id"], "e"),
            FieldError::new(&["general"], "f"),
        ];
        let messages = |game| -> Vec<&str> {
            settings
                .errors_of(game, &errors)
                .map(|error| error.message.as_str())
                .collect()
        };
        assert_eq!(messages(GameName::Werewolf), ["a", "c", "d"]);
        assert_eq!(messages(GameName::Avalon), ["b"]);
        assert!(errors[0].is_at(&["werewolf", "phases", "day_limit"].map(String::from)));
        assert!(!errors[0].is_at(&["werewolf".to_string()]));
    }

    #[test]
    fn a_random_strategy_asks_no_model() {
        let mut settings = GameSettings::default();
        settings.strategies.insert(
            "dice".to_string(),
            StrategySettings {
                policy: Policy::Random,
                ..StrategySettings::default()
            },
        );
        assert_eq!(settings.model_of("dice"), None);
        assert_eq!(settings.model_of("missing"), None);
    }

    #[test]
    fn games_are_keyed_and_titled() {
        assert_eq!(game_key(GameName::Werewolf), "werewolf");
        assert_eq!(game_key(GameName::Avalon), "avalon");
        assert_eq!(game_title(GameName::Werewolf), "Werewolf");
        assert_eq!(game_title(GameName::Avalon), "Avalon");
        let mut settings = AppSettings::default();
        settings.game_mut(GameName::Avalon).strategy = "a".to_string();
        settings.game_mut(GameName::Werewolf).strategy = "w".to_string();
        assert_eq!(settings.game(GameName::Avalon).strategy, "a");
        assert_eq!(settings.game(GameName::Werewolf).strategy, "w");
    }

    #[test]
    fn the_look_holds_the_theme_or_leaves_it_to_the_system() {
        assert_eq!(Appearance::System.theme(), None);
        assert_eq!(Appearance::Light.theme(), Some("light"));
        assert_eq!(Appearance::Dark.theme(), Some("dark"));
        assert_eq!(General::default().appearance, Appearance::System);
    }

    #[test]
    fn settings_left_out_are_their_defaults() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AppSettings::default());
        assert_eq!(settings.werewolf.strategy, RANDOM);
        assert_eq!(settings.general.game, GameName::Werewolf);
    }

    #[test]
    fn a_key_s_status_says_where_it_is() {
        assert_eq!(KeyStatus::InKeychain.told(), "in the keychain");
        assert_eq!(KeyStatus::InEnvironment.told(), "in the environment");
        assert_eq!(KeyStatus::Missing.told(), "not set");
        assert_eq!(
            KeyStatus::Unavailable("no store".to_string()).told(),
            "no store"
        );
        assert!(KeyStatus::InKeychain.is_set() && KeyStatus::InEnvironment.is_set());
        assert!(!KeyStatus::Missing.is_set());
    }

    #[test]
    fn a_check_is_marked_and_told() {
        let no = Check::DidNotPlay("no tools".to_string());
        assert_eq!(
            [Check::Plays.mark(), Check::PlaysUnforced.mark(), no.mark()],
            ["✓", "~", "✗"]
        );
        assert_eq!(Check::Plays.told(), "plays");
        assert_eq!(
            Check::PlaysUnforced.told(),
            "plays only when not made to call a tool"
        );
        assert_eq!(no.told(), "doesn't play: no tools");
        assert!(Check::Plays.plays() && Check::PlaysUnforced.plays() && !no.plays());
    }

    #[test]
    fn progress_is_told_as_a_count() {
        let progress = Progress {
            base_url: "https://api.openai.com/v1".to_string(),
            checked: 14,
            total: 31,
        };
        assert_eq!(progress.told(), "Checked 14 of 31");
    }
}
