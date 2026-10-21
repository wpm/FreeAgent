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
    Appearance, Check, FieldError, General, KeyStatus, Listed, Listing, ModelSettings, Policy,
    Probe, Progress, Provider, RANDOM, Settings, Strategies, StrategySettings, Trouble, Unchecked,
    host, is_part_name, is_remote, label, part_variable, provider_at, root, unique, whole,
};

use crate::{GameName, Role, Side};

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

impl Settings for AppSettings {
    fn general(&self) -> &General {
        &self.general
    }

    fn general_mut(&mut self) -> &mut General {
        &mut self.general
    }

    fn models(&self) -> &BTreeMap<String, ModelSettings> {
        &self.models
    }

    fn models_mut(&mut self) -> &mut BTreeMap<String, ModelSettings> {
        &mut self.models
    }

    fn providers(&self) -> &[Provider] {
        &self.providers
    }

    fn providers_mut(&mut self) -> &mut Vec<Provider> {
        &mut self.providers
    }

    /// The prompted strategies of each game, each as its game and its
    /// name: `Werewolf “everyone”`.
    fn strategies_asking(&self) -> Vec<(String, &str)> {
        GameName::ALL
            .into_iter()
            .flat_map(|game| {
                let settings = self.game(game);
                settings.strategies.keys().filter_map(move |strategy| {
                    let model = settings.model_of(strategy)?;
                    Some((format!("{} “{strategy}”", game_title(game)), model))
                })
            })
            .collect()
    }

    fn rename_in_strategies(&mut self, old: &str, new: &str) {
        for game in [&mut self.werewolf, &mut self.avalon] {
            for strategy in game.strategies.values_mut() {
                if strategy.model == old {
                    strategy.model = new.to_string();
                }
            }
        }
    }
}

impl Strategies for AppSettings {
    type Of = GameName;

    const RANDOM: bool = true;

    fn at(game: GameName) -> Vec<String> {
        vec![game_key(game).to_string()]
    }

    fn strategies(&self, game: GameName) -> &BTreeMap<String, StrategySettings> {
        &self.game(game).strategies
    }

    fn strategies_mut(&mut self, game: GameName) -> &mut BTreeMap<String, StrategySettings> {
        &mut self.game_mut(game).strategies
    }

    fn prompt_parts(&self, game: GameName) -> &BTreeMap<String, String> {
        &self.game(game).prompt_parts
    }

    fn prompt_parts_mut(&mut self, game: GameName) -> &mut BTreeMap<String, String> {
        &mut self.game_mut(game).prompt_parts
    }

    fn part_users(&self, game: GameName) -> BTreeMap<String, Vec<String>> {
        self.game(game)
            .part_users()
            .into_iter()
            .map(|(part, users)| {
                (
                    part.to_string(),
                    users.into_iter().map(String::from).collect(),
                )
            })
            .collect()
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

/// The settings as the backend holds them, and what is wrong with them.
pub type Snapshot = free_agent_gui_types::settings::Snapshot<AppSettings>;

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
    fn a_model_is_renamed_in_both_games() {
        let mut settings = two_strategies();
        settings
            .avalon
            .strategies
            .insert("all".to_string(), prompted("gpt"));
        assert_eq!(settings.rename_model("gpt", "opus"), Ok(()));
        assert!(settings.models.contains_key("opus") && !settings.models.contains_key("gpt"));
        assert_eq!(settings.werewolf.strategies["smart"].model, "opus");
        assert_eq!(settings.avalon.strategies["all"].model, "opus");
        assert_eq!(settings.werewolf.strategies["everyone"].model, "local");
    }

    #[test]
    fn the_look_and_the_providers_are_the_app_s_own() {
        let mut settings = two_strategies();
        settings.general_mut().appearance = Appearance::Dark;
        assert_eq!(settings.general().appearance, Appearance::Dark);
        let example = Provider {
            name: "Example".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key_env: None,
        };
        assert_eq!(settings.add_provider(example.clone()), Ok(()));
        assert_eq!(settings.providers(), std::slice::from_ref(&example));
        assert_eq!(settings.providers_shown(&[]), [example]);
        settings.remove_provider("https://api.example.com/v1");
        assert!(settings.providers.is_empty());
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
    fn each_game_has_its_own_strategies_and_prompt_parts() {
        let mut settings = two_strategies();
        assert_eq!(AppSettings::at(GameName::Avalon), ["avalon"]);
        settings
            .prompt_parts_mut(GameName::Werewolf)
            .insert("rules".to_string(), "Play.".to_string());
        settings
            .strategies_mut(GameName::Werewolf)
            .get_mut("smart")
            .unwrap()
            .system = "{{ rules }}".to_string();
        settings
            .strategies_mut(GameName::Avalon)
            .insert("all".to_string(), prompted("gpt"));
        assert_eq!(
            settings
                .strategies(GameName::Avalon)
                .keys()
                .collect::<Vec<_>>(),
            ["all"]
        );
        assert_eq!(settings.prompt_parts(GameName::Werewolf)["rules"], "Play.");
        assert!(settings.prompt_parts(GameName::Avalon).is_empty());
        assert_eq!(
            settings.part_users(GameName::Werewolf),
            BTreeMap::from([("rules".to_string(), vec!["smart".to_string()])])
        );
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
