//! The desktop app's settings: the look, the models
//! both games share, and each game's table, phases, strategies and
//! prompt parts.
//! The backend stores them and checks them, and the Settings window edits
//! them, so they cross between the two as they are. What every app's
//! settings share, from the look to the providers, the models each one
//! lists, what probing a model found and where an API key is, comes from
//! [`free_agent_gui_types::settings`]. None of them ever carries a key.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use free_agent_gui_types::settings::{
    Appearance, Check, FieldError, General, KeyStatus, Listed, Listing, ModelSettings, Probe,
    Progress, Provider, Trouble, Unchecked, host, is_part_name, is_remote, label, part_variable,
    provider_at, root, unique, whole,
};

use crate::{GameName, Role, Side};

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
    /// The providers added besides those known and found, in the order
    /// they were added.
    pub providers: Vec<Provider>,
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
        let players = settings.seats().len();
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
        let models: BTreeSet<String> = self
            .game(game)
            .models_named()
            .into_iter()
            .map(String::from)
            .collect();
        errors.iter().filter(move |error| match &error.path[..] {
            [first, ..] if first == key => true,
            [first, name, ..] if first == "models" => models.contains(name),
            _ => false,
        })
    }

    /// Why `name` cannot name a model other than `except`: it is empty, or
    /// another model has it.
    fn name_problem(&self, name: &str, except: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("Give it a name.".to_string());
        }
        if name != except && self.models.contains_key(name) {
            return Err("Another model already has that name.".to_string());
        }
        Ok(())
    }

    /// Add `model` as `name`, trimmed, unless that cannot name it.
    ///
    /// # Errors
    ///
    /// When the name is empty or another model has it.
    pub fn add_model(&mut self, name: &str, model: ModelSettings) -> Result<(), String> {
        let name = name.trim();
        self.name_problem(name, "")?;
        self.models.insert(name.to_string(), model);
        Ok(())
    }

    /// Rename the model `old` to `new`, trimmed, and every strategy of
    /// either game that asks it with it, unless `new` cannot name it.
    ///
    /// # Errors
    ///
    /// When the name is empty or another model has it.
    pub fn rename_model(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        self.name_problem(new, old)?;
        let Some(model) = self.models.remove(old) else {
            return Ok(());
        };
        self.models.insert(new.to_string(), model);
        for game in [&mut self.werewolf, &mut self.avalon] {
            for strategy in game.strategies.values_mut() {
                if strategy.model == old {
                    strategy.model = new.to_string();
                }
            }
        }
        Ok(())
    }

    /// The prompted strategies that ask the model `name`, each as its game
    /// and its name: `Werewolf “everyone”`.
    pub fn users_of_model(&self, name: &str) -> Vec<String> {
        GameName::ALL
            .into_iter()
            .flat_map(|game| {
                let settings = self.game(game);
                settings
                    .strategies
                    .keys()
                    .filter(move |strategy| settings.model_of(strategy) == Some(name))
                    .map(move |strategy| format!("{} “{strategy}”", game_title(game)))
            })
            .collect()
    }

    /// Add `provider`, its name and base URL trimmed, the base URL's
    /// final slash too, unless it lacks a name or a whole base URL: an
    /// `http` or `https` URL ending in `/v1`.
    ///
    /// # Errors
    ///
    /// When it lacks a name or a whole base URL.
    pub fn add_provider(&mut self, provider: Provider) -> Result<(), String> {
        let name = provider.name.trim();
        let base_url = root(&provider.base_url);
        if name.is_empty() || !whole(base_url) {
            return Err("Give it a name and a base URL ending in /v1.".to_string());
        }
        self.providers.push(Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: provider.api_key_env,
        });
        Ok(())
    }

    /// The providers the Models tab shows: those `found`, known and local,
    /// then those added, then the provider of each model's base URL that
    /// is none of them, as a local server when it is one on this machine,
    /// whether or not it is up, and otherwise named for its host.
    pub fn providers_shown(&self, found: &[Provider]) -> Vec<Provider> {
        let mut shown = found.to_vec();
        for provider in &self.providers {
            if provider_at(&shown, &provider.base_url).is_none() {
                shown.push(provider.clone());
            }
        }
        for model in self.models.values() {
            if model.base_url.trim().is_empty() || provider_at(&shown, &model.base_url).is_some() {
                continue;
            }
            shown.push(match Provider::local_port(&model.base_url) {
                Some(port) => Provider::local(port),
                None => Provider {
                    name: host(&model.base_url).to_string(),
                    base_url: root(&model.base_url).to_string(),
                    api_key_env: model.api_key_env.clone(),
                },
            });
        }
        shown
    }

    /// Remove the provider added at `base_url`, and every model reached
    /// through it.
    pub fn remove_provider(&mut self, base_url: &str) {
        let at = |url: &str| root(url) == root(base_url);
        self.providers.retain(|provider| !at(&provider.base_url));
        self.models.retain(|_, model| !at(&model.base_url));
    }

    /// The first error among `errors` that stops a game of `game`, with
    /// the game's title before it, and where it is.
    pub fn first_error(&self, game: GameName, errors: &[FieldError]) -> Option<FieldError> {
        let error = self.errors_of(game, errors).next()?;
        Some(FieldError {
            path: error.path.clone(),
            message: format!("{}: {}", game_title(game), error.message),
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
    /// Named pieces of text the system prompts pull in with their
    /// [`part_variable`], read also from settings stored when they were called `text`.
    #[serde(alias = "text")]
    pub prompt_parts: BTreeMap<String, String>,
}

impl Default for GameSettings {
    /// No roles, no phases, and everyone random.
    fn default() -> Self {
        Self {
            strategy: RANDOM.to_string(),
            roles: Vec::new(),
            phases: Vec::new(),
            strategies: BTreeMap::new(),
            prompt_parts: BTreeMap::new(),
        }
    }
}

impl GameSettings {
    /// The side of each seat at the table, evil first. Loading lists only a
    /// game's own roles, so every key names a role.
    pub fn seats(&self) -> Vec<Side> {
        let mut seats: Vec<Side> = self
            .roles
            .iter()
            .flat_map(|role| {
                let known = Role::from_key(&role.key);
                let side = known.unwrap_or_else(|| panic!("{} is not a role", role.key));
                std::iter::repeat_n(side.side(), role.count)
            })
            .collect();
        seats.sort_by_key(|side| *side != Side::Evil);
        seats
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

    /// The names of the models the prompted strategies name.
    pub fn models_named(&self) -> BTreeSet<&str> {
        self.strategies
            .keys()
            .filter_map(|name| self.model_of(name))
            .collect()
    }

    /// Each prompt part, with the names of the prompted strategies whose
    /// system prompts use it. A system prompt that doesn't compile uses
    /// none.
    pub fn part_users(&self) -> BTreeMap<&str, Vec<&str>> {
        let mut users: BTreeMap<&str, Vec<&str>> = self
            .prompt_parts
            .keys()
            .map(|part| (part.as_str(), Vec::new()))
            .collect();
        let parts: BTreeMap<String, &str> = self
            .prompt_parts
            .keys()
            .map(|part| (part_variable(part), part.as_str()))
            .collect();
        let env = minijinja::Environment::new();
        for (name, strategy) in &self.strategies {
            if strategy.policy != Policy::Prompted {
                continue;
            }
            let Ok(template) = env.template_from_str(&strategy.system) else {
                continue;
            };
            for variable in template.undeclared_variables(false) {
                if let Some(part) = parts.get(&variable) {
                    users.get_mut(part).unwrap().push(name);
                }
            }
        }
        users
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

/// The settings as the backend holds them, and what is wrong with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The settings.
    pub settings: AppSettings,
    /// Every field's error.
    pub errors: Vec<FieldError>,
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
    fn a_game_s_seats_are_its_roles_sides_evil_first() {
        let settings = GameSettings {
            roles: vec![
                role("villager", 2, None),
                role("werewolf", 1, None),
                role("seer", 1, None),
                role("doctor", 0, None),
            ],
            ..GameSettings::default()
        };
        assert_eq!(
            settings.seats(),
            [Side::Evil, Side::Good, Side::Good, Side::Good]
        );
    }

    #[test]
    #[should_panic(expected = "wizard is not a role")]
    fn every_role_has_a_seat() {
        let settings = GameSettings {
            roles: vec![role("wizard", 1, None)],
            ..GameSettings::default()
        };
        settings.seats();
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
    fn settings_left_out_are_their_defaults() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AppSettings::default());
        assert_eq!(settings.werewolf.strategy, RANDOM);
    }

    #[test]
    fn a_model_is_renamed_and_the_strategies_that_ask_it_follow() {
        let mut settings = two_strategies();
        settings
            .avalon
            .strategies
            .insert("all".to_string(), prompted("gpt"));
        assert_eq!(settings.rename_model("gpt", " opus "), Ok(()));
        assert!(settings.models.contains_key("opus") && !settings.models.contains_key("gpt"));
        assert_eq!(settings.werewolf.strategies["smart"].model, "opus");
        assert_eq!(settings.avalon.strategies["all"].model, "opus");
        assert_eq!(settings.werewolf.strategies["everyone"].model, "local");
        assert_eq!(settings.rename_model("opus", "opus"), Ok(()));
        assert_eq!(
            settings.rename_model("opus", " "),
            Err("Give it a name.".to_string())
        );
        assert_eq!(
            settings.rename_model("opus", "local"),
            Err("Another model already has that name.".to_string())
        );
        assert_eq!(settings.rename_model("missing", "found"), Ok(()));
        assert!(!settings.models.contains_key("found"));
    }

    #[test]
    fn a_model_is_added_under_a_name_no_other_has() {
        let mut settings = two_strategies();
        let opus = ModelSettings {
            base_url: "https://api.anthropic.com/v1".to_string(),
            id: "claude-opus-5-5".to_string(),
            ..ModelSettings::default()
        };
        assert_eq!(
            settings.add_model("gpt", opus.clone()),
            Err("Another model already has that name.".to_string())
        );
        assert_eq!(
            settings.add_model("", opus.clone()),
            Err("Give it a name.".to_string())
        );
        assert_eq!(settings.add_model(" opus ", opus.clone()), Ok(()));
        assert_eq!(settings.models["opus"], opus);
    }

    #[test]
    fn a_model_s_users_are_the_prompted_strategies_that_ask_it() {
        let mut settings = two_strategies();
        settings
            .avalon
            .strategies
            .insert("all".to_string(), prompted("local"));
        assert_eq!(
            settings.users_of_model("local"),
            ["Werewolf “everyone”", "Avalon “all”"]
        );
        assert_eq!(settings.users_of_model("gpt"), ["Werewolf “smart”"]);
        assert!(settings.users_of_model("nobody").is_empty());
    }

    #[test]
    fn a_provider_added_needs_a_name_and_a_whole_base_url_and_takes_its_models_when_removed() {
        let mut settings = two_strategies();
        let example = |name: &str, base_url: &str| Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: Some("EXAMPLE_API_KEY".to_string()),
        };
        let wanted = "Give it a name and a base URL ending in /v1.";
        for (name, base_url) in [
            (" ", "https://api.example.com/v1"),
            ("Example", "api.example.com/v1"),
            ("Example", "https://api.example.com"),
        ] {
            assert_eq!(
                settings.add_provider(example(name, base_url)),
                Err(wanted.to_string())
            );
        }
        assert_eq!(
            settings.add_provider(example(" Example ", " https://api.example.com/v1/ ")),
            Ok(())
        );
        assert_eq!(
            settings.providers,
            [example("Example", "https://api.example.com/v1")]
        );
        settings.models.get_mut("gpt").unwrap().base_url =
            "https://api.example.com/v1/".to_string();
        settings.remove_provider("https://api.example.com/v1");
        assert!(settings.providers.is_empty());
        assert_eq!(settings.models.keys().collect::<Vec<_>>(), ["local"]);
    }

    #[test]
    fn the_providers_shown_are_those_found_then_added_then_each_other_a_model_names() {
        let openai = Provider {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
        };
        assert_eq!(
            provider_at(std::slice::from_ref(&openai), " https://api.openai.com/v1/")
                .map(|p| p.name.as_str()),
            Some("OpenAI")
        );
        let added = Provider {
            name: "Example".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key_env: None,
        };
        let model = |base_url: &str, api_key_env: Option<&str>| ModelSettings {
            base_url: base_url.to_string(),
            api_key_env: api_key_env.map(String::from),
            ..ModelSettings::default()
        };
        let settings = AppSettings {
            providers: vec![added.clone(), openai.clone()],
            models: BTreeMap::from([
                ("a".to_string(), model("https://api.openai.com/v1/", None)),
                ("b".to_string(), model("http://localhost:9999/v1", None)),
                (
                    "c".to_string(),
                    model("https://other.example.org/v1/", Some("OTHER_KEY")),
                ),
                ("d".to_string(), model("", None)),
                ("e".to_string(), model("https://api.example.com/v1", None)),
            ]),
            ..AppSettings::default()
        };
        assert_eq!(
            settings.providers_shown(std::slice::from_ref(&openai)),
            [
                openai,
                added,
                Provider::local(9999),
                Provider {
                    name: "other.example.org".to_string(),
                    base_url: "https://other.example.org/v1".to_string(),
                    api_key_env: Some("OTHER_KEY".to_string()),
                },
            ]
        );
        assert_eq!(host("localhost"), "localhost");
    }

    #[test]
    fn a_game_s_first_error_is_titled_with_the_game() {
        let settings = two_strategies();
        let errors = [FieldError::new(&["models", "gpt", "id"], "no id")];
        assert_eq!(
            settings.first_error(GameName::Werewolf, &errors),
            Some(FieldError::new(&["models", "gpt", "id"], "Werewolf: no id"))
        );
        assert_eq!(settings.first_error(GameName::Avalon, &errors), None);
    }

    #[test]
    fn a_prompt_part_is_used_by_the_prompted_strategies_that_pull_it_in() {
        let mut settings = two_strategies().werewolf;
        let system = |settings: &mut GameSettings, name: &str, system: &str| {
            settings.strategies.get_mut(name).unwrap().system = system.to_string();
        };
        for part in ["rules", "ruleset", "tone", "rule", "aside", "Seer only"] {
            settings
                .prompt_parts
                .insert(part.to_string(), String::new());
        }
        system(
            &mut settings,
            "everyone",
            "{{ rules }}\nYou are {{name}}.{{ Seer_only }}",
        );
        system(
            &mut settings,
            "smart",
            "{{rules|upper}} {% if tone %}{{ ruleset }}{% endif %}",
        );
        system(&mut settings, "idle", "{{ aside");
        settings.strategies.insert(
            "dumb".to_string(),
            StrategySettings {
                policy: Policy::Random,
                system: "{{ rules }}".to_string(),
                ..StrategySettings::default()
            },
        );
        let users = settings.part_users();
        let users: Vec<(&str, &[&str])> = users.iter().map(|(p, u)| (*p, &u[..])).collect();
        assert_eq!(
            users,
            [
                ("Seer only", &["everyone"][..]),
                ("aside", &[]),
                ("rule", &[]),
                ("rules", &["everyone", "smart"]),
                ("ruleset", &["smart"]),
                ("tone", &["smart"]),
            ]
        );
    }
}
