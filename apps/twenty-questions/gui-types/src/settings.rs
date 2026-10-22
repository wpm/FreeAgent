//! The desktop app's settings: the look, the models, and the game: its
//! table, its host, its strategies and its prompt parts. The backend
//! stores them and checks them, and the Settings window edits them, so
//! they cross between the two as they are. What every app's settings
//! share, from the look to the providers, the models each one lists, what
//! probing a model found and where an API key is, comes from
//! [`free_agent_gui_types::settings`]. None of them ever carries a key.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use free_agent_gui_types::settings::{
    Appearance, Check, FieldError, General, KeyStatus, Listed, Listing, ModelSettings, Policy,
    Probe, Progress, Provider, Settings, Strategies, StrategySettings, Trouble, Unchecked, host,
    is_part_name, is_remote, label, part_variable, provider_at, root, unique, whole,
};

/// The key the game's settings are under, and its errors' paths start
/// with.
pub const GAME: &str = "game";

/// Everything the app's settings hold.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// What belongs to no game.
    pub general: General,
    /// The models, by name, which the host and the strategies choose
    /// from.
    pub models: BTreeMap<String, ModelSettings>,
    /// The game's settings.
    pub game: GameSettings,
    /// The providers added besides those known and found, in the order
    /// they were added.
    pub providers: Vec<Provider>,
}

impl AppSettings {
    /// One line saying what a game is: `4 players · qwen2.5-7b-instruct-1m
    /// · host picks`, with the ids of the models in play, if any, and the
    /// secret object when one is named.
    pub fn summary(&self) -> String {
        let game = &self.game;
        let players = if game.players == 1 {
            "1 player".to_string()
        } else {
            format!("{} players", game.players)
        };
        let mut parts = vec![players];
        let ids: BTreeSet<&str> = game
            .models_in_play()
            .into_iter()
            .map(|name| self.models.get(name).map_or(name, |model| &model.id))
            .collect();
        if !ids.is_empty() {
            parts.push(ids.into_iter().collect::<Vec<_>>().join(", "));
        }
        parts.push(match game.host.object.trim() {
            "" => "host picks".to_string(),
            object => format!("secret: {object}"),
        });
        parts.join(" · ")
    }

    /// The errors among `errors` that stop a game: the game's own, and
    /// those of the models it asks.
    pub fn errors_of<'a>(
        &self,
        errors: &'a [FieldError],
    ) -> impl Iterator<Item = &'a FieldError> + use<'a> {
        let models: BTreeSet<String> = self
            .game
            .models_named()
            .into_iter()
            .map(String::from)
            .collect();
        errors.iter().filter(move |error| match &error.path[..] {
            [first, ..] if first == GAME => true,
            [first, name, ..] if first == "models" => models.contains(name),
            _ => false,
        })
    }

    /// The first error among `errors` that stops a game, if there is one.
    pub fn first_error(&self, errors: &[FieldError]) -> Option<FieldError> {
        self.errors_of(errors).next().cloned()
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

    /// The host, then each strategy by its name: `“everyone”`.
    fn model_users(&self) -> Vec<(String, &str)> {
        let host = ("The host".to_string(), self.game.host.model.as_str());
        let strategies = self
            .game
            .strategies
            .iter()
            .map(|(name, strategy)| (format!("“{name}”"), strategy.model.as_str()));
        std::iter::once(host).chain(strategies).collect()
    }

    fn rename_model_users(&mut self, old: &str, new: &str) {
        let game = &mut self.game;
        let models = std::iter::once(&mut game.host.model).chain(
            game.strategies
                .values_mut()
                .map(|strategy| &mut strategy.model),
        );
        for model in models.filter(|model| *model == old) {
            *model = new.to_string();
        }
    }
}

impl Strategies for AppSettings {
    type Of = ();

    const MAY_BE_RANDOM: bool = false;

    fn at((): ()) -> Vec<String> {
        vec![GAME.to_string()]
    }

    fn strategies(&self, (): ()) -> &BTreeMap<String, StrategySettings> {
        &self.game.strategies
    }

    fn strategies_mut(&mut self, (): ()) -> &mut BTreeMap<String, StrategySettings> {
        &mut self.game.strategies
    }

    fn prompt_parts(&self, (): ()) -> &BTreeMap<String, String> {
        &self.game.prompt_parts
    }

    fn prompt_parts_mut(&mut self, (): ()) -> &mut BTreeMap<String, String> {
        &mut self.game.prompt_parts
    }

    fn part_users(&self, (): ()) -> BTreeMap<String, Vec<String>> {
        self.game.part_users()
    }
}

/// The game's settings: everything its configuration holds but the
/// models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// How many players sit at the table.
    pub players: usize,
    /// How many guesses the players have among them.
    pub guesses: u64,
    /// How long the game lasts, as written: `10m`.
    pub time_limit: String,
    /// The strategy every seat plays that `allotments` does not give
    /// another.
    pub strategy: String,
    /// How many seats play each other strategy, by name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub allotments: BTreeMap<String, usize>,
    /// The host.
    pub host: HostSettings,
    /// The strategies, by name, each prompted.
    pub strategies: BTreeMap<String, StrategySettings>,
    /// Named pieces of text the prompts pull in with their
    /// [`part_variable`].
    pub prompt_parts: BTreeMap<String, String>,
}

impl Default for GameSettings {
    /// Four players with twenty guesses and ten minutes, and nothing else.
    fn default() -> Self {
        Self {
            players: 4,
            guesses: 20,
            time_limit: "10m".to_string(),
            strategy: String::new(),
            allotments: BTreeMap::new(),
            host: HostSettings::default(),
            strategies: BTreeMap::new(),
            prompt_parts: BTreeMap::new(),
        }
    }
}

impl GameSettings {
    /// The names of the strategies allotted a seat: those `allotments`
    /// gives seats, then `strategy` for the seats left.
    pub fn at_the_table(&self) -> BTreeSet<&str> {
        let allotted: usize = self.allotments.values().sum();
        let mut names: BTreeSet<&str> = self
            .allotments
            .iter()
            .filter(|(_, seats)| **seats > 0)
            .map(|(name, _)| name.as_str())
            .collect();
        if self.players > allotted {
            names.insert(&self.strategy);
        }
        names
    }

    /// The names of the models in play: the host's, and those the
    /// strategies at the table ask.
    pub fn models_in_play(&self) -> BTreeSet<&str> {
        self.at_the_table()
            .into_iter()
            .filter_map(|name| self.strategies.get(name))
            .map(|strategy| strategy.model.as_str())
            .chain([self.host.model.as_str()])
            .filter(|name| !name.is_empty())
            .collect()
    }

    /// The names of the models the host and every strategy name.
    pub fn models_named(&self) -> BTreeSet<&str> {
        self.strategies
            .values()
            .map(|strategy| strategy.model.as_str())
            .chain([self.host.model.as_str()])
            .collect()
    }

    /// Each prompt part, with the names of what pulls it in: `host`, `pick`
    /// and each strategy by name. A prompt that doesn't compile pulls in
    /// none.
    pub fn part_users(&self) -> BTreeMap<String, Vec<String>> {
        let mut users: BTreeMap<String, Vec<String>> = self
            .prompt_parts
            .keys()
            .map(|part| (part.clone(), Vec::new()))
            .collect();
        let parts: BTreeMap<String, &str> = self
            .prompt_parts
            .keys()
            .map(|part| (part_variable(part), part.as_str()))
            .collect();
        let env = minijinja::Environment::new();
        let prompts = [("host", &self.host.system), ("pick", &self.host.pick)]
            .into_iter()
            .chain(
                self.strategies
                    .iter()
                    .map(|(name, strategy)| (name.as_str(), &strategy.system)),
            );
        for (user, prompt) in prompts {
            let Ok(template) = env.template_from_str(prompt) else {
                continue;
            };
            for variable in template.undeclared_variables(false) {
                if let Some(part) = parts.get(&variable) {
                    users.get_mut(*part).unwrap().push(user.to_string());
                }
            }
        }
        users
    }
}

/// The host's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    /// The name of the model the host asks.
    pub model: String,
    /// The secret object, or empty for the host to pick one.
    pub object: String,
    /// The temperature the host picks at.
    pub temperature: f64,
    /// The categories the pick draws one from, at random.
    pub categories: Vec<String>,
    /// The template the host is asked to pick an object with.
    pub pick: String,
    /// The host's system prompt template.
    pub system: String,
}

impl Default for HostSettings {
    /// No model, no object, no prompts, and the usual temperature.
    fn default() -> Self {
        Self {
            model: String::new(),
            object: String::new(),
            temperature: 1.2,
            categories: Vec::new(),
            pick: String::new(),
            system: String::new(),
        }
    }
}

/// The settings as the backend holds them, and what is wrong with them.
pub type Snapshot = free_agent_gui_types::settings::Snapshot<AppSettings>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A strategy asking `model`, told `system`.
    fn prompted(model: &str, system: &str) -> StrategySettings {
        StrategySettings {
            policy: Policy::Prompted,
            model: model.to_string(),
            system: system.to_string(),
        }
    }

    /// Settings with a host on `local` and three players, two playing
    /// `everyone` on `local` and one `skeptic` on `gpt`, and the strategy
    /// `idle`, at no seat, on `other`.
    fn table() -> AppSettings {
        let model = |id: &str| ModelSettings {
            id: id.to_string(),
            ..ModelSettings::default()
        };
        AppSettings {
            models: BTreeMap::from([
                ("gpt".to_string(), model("gpt-4.1-mini")),
                ("local".to_string(), model("qwen")),
            ]),
            game: GameSettings {
                players: 3,
                strategy: "everyone".to_string(),
                allotments: BTreeMap::from([("skeptic".to_string(), 1), ("idle".to_string(), 0)]),
                host: HostSettings {
                    model: "local".to_string(),
                    ..HostSettings::default()
                },
                strategies: BTreeMap::from([
                    ("everyone".to_string(), prompted("local", "Play.")),
                    ("skeptic".to_string(), prompted("gpt", "Doubt.")),
                    ("idle".to_string(), prompted("other", "Wait.")),
                ]),
                ..GameSettings::default()
            },
            ..AppSettings::default()
        }
    }

    #[test]
    fn the_summary_counts_the_players_names_the_models_in_play_and_the_secret() {
        let mut settings = table();
        assert_eq!(
            settings.summary(),
            "3 players · gpt-4.1-mini, qwen · host picks"
        );
        settings.game.players = 1;
        settings.game.allotments.clear();
        settings.game.host.object = " a teapot ".to_string();
        assert_eq!(settings.summary(), "1 player · qwen · secret: a teapot");
        settings.game.host.model = "ghost".to_string();
        settings.game.strategy = "missing".to_string();
        assert_eq!(settings.summary(), "1 player · ghost · secret: a teapot");
        settings.game.host.model.clear();
        assert_eq!(settings.summary(), "1 player · secret: a teapot");
    }

    #[test]
    fn the_strategy_everyone_plays_sits_only_where_seats_are_left() {
        let mut game = table().game;
        assert_eq!(
            game.at_the_table().into_iter().collect::<Vec<_>>(),
            ["everyone", "skeptic"]
        );
        game.players = 1;
        assert_eq!(
            game.at_the_table().into_iter().collect::<Vec<_>>(),
            ["skeptic"]
        );
        assert_eq!(
            game.models_in_play().into_iter().collect::<Vec<_>>(),
            ["gpt", "local"]
        );
        assert_eq!(
            game.models_named().into_iter().collect::<Vec<_>>(),
            ["gpt", "local", "other"]
        );
    }

    #[test]
    fn the_game_s_errors_are_its_own_and_those_of_the_models_it_names() {
        let settings = table();
        let errors = [
            FieldError::new(&["general"], "a"),
            FieldError::new(&["models", "unused", "id"], "b"),
            FieldError::new(&["models", "gpt", "id"], "c"),
            FieldError::new(&["game", "players"], "d"),
        ];
        let messages: Vec<&str> = settings
            .errors_of(&errors)
            .map(|error| error.message.as_str())
            .collect();
        assert_eq!(messages, ["c", "d"]);
        assert_eq!(settings.first_error(&errors), Some(errors[2].clone()));
        assert_eq!(settings.first_error(&errors[..2]), None);
    }

    #[test]
    fn the_models_users_are_the_host_and_the_strategies_and_follow_a_rename() {
        let mut settings = table();
        assert_eq!(settings.users_of_model("local"), ["The host", "“everyone”"]);
        assert_eq!(settings.rename_model("local", "qwen"), Ok(()));
        assert_eq!(settings.game.host.model, "qwen");
        assert_eq!(settings.game.strategies["everyone"].model, "qwen");
        assert_eq!(settings.game.strategies["skeptic"].model, "gpt");
        settings.general_mut().appearance = Appearance::Dark;
        assert_eq!(settings.general().appearance, Appearance::Dark);
        let example = Provider {
            name: "Example".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key_env: None,
        };
        assert_eq!(settings.add_provider(example.clone()), Ok(()));
        assert_eq!(settings.providers(), [example]);
    }

    #[test]
    fn the_game_has_one_set_of_strategies_and_prompt_parts_none_random() {
        let mut settings = table();
        const { assert!(!AppSettings::MAY_BE_RANDOM) };
        assert_eq!(AppSettings::at(()), ["game"]);
        settings
            .prompt_parts_mut(())
            .insert("rules".to_string(), "Ask.".to_string());
        settings.strategies_mut(()).get_mut("idle").unwrap().system = "{{ rules }}".to_string();
        assert_eq!(settings.strategies(()).len(), 3);
        assert_eq!(settings.prompt_parts(())["rules"], "Ask.");
        assert_eq!(
            settings.part_users(()),
            BTreeMap::from([("rules".to_string(), vec!["idle".to_string()])])
        );
    }

    #[test]
    fn a_prompt_part_is_used_by_the_prompts_that_pull_it_in() {
        let mut game = table().game;
        for part in ["rules", "tone", "House rules", "aside"] {
            game.prompt_parts.insert(part.to_string(), String::new());
        }
        game.host.system = "{{ rules }} The object is {{ object }}.".to_string();
        game.host.pick = "{{ House_rules }} Pick {{ category }}.".to_string();
        game.strategies.get_mut("everyone").unwrap().system =
            "{{ rules }} {% if tone %}{{ tone }}{% endif %}".to_string();
        game.strategies.get_mut("skeptic").unwrap().system = "{{ aside".to_string();
        let users = game.part_users();
        assert_eq!(users["rules"], ["host", "everyone"]);
        assert_eq!(users["House rules"], ["pick"]);
        assert_eq!(users["tone"], ["everyone"]);
        assert!(users["aside"].is_empty());
    }

    #[test]
    fn settings_left_out_are_their_defaults() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AppSettings::default());
        assert_eq!(settings.game.players, 4);
        assert_eq!(settings.game.guesses, 20);
        assert_eq!(settings.game.time_limit, "10m");
        assert_eq!(settings.game.host.temperature, 1.2);
    }
}
