//! A game's configuration file: the [`Config`] its TOML is parsed into,
//! and the [`Settings`] that take effect once it is checked and combined
//! with the defaults in the code. [`load`] is the one place a
//! configuration file is read.
//!
//! The file names the players' count and how many seats play each
//! strategy, the guesses they have and the game's time limit, the host's
//! model, its secret object or how it picks one, and its prompts, the
//! models, the prompt parts, and the strategies, each a prompted policy
//! with a system template and a model. An unknown key anywhere is an
//! error, so a misspelled setting fails loudly, and every other error
//! names the key it is about.
//!
//! Every prompt is rendered before any game begins, so that a broken
//! template stops the program first: each player's for every seat and
//! every strategy at the table, with `name`, `others` and `guesses`; the
//! host's with `object` and `players`; and the pick with `category`, for
//! each category. The prompt parts are pulled into each. `object` is never
//! a player's variable, so no player's prompt can name it.

use crate::host;
use crate::model::plays;
use crate::player;
use anyhow::{Context as _, bail};
use free_agent_game::config::Program;
use free_agent_game::report::in_prose;
use free_agent_game::seat::{PlayerId, seats};
use free_agent_game::strategy::Prompted;
use free_agent_gui_types::settings::{is_part_name, part_variable};
use free_agent_model::Provider;
use free_agent_model::spec::{self, Handles, Keys, Models, Spec};
use minijinja::value::merge_maps;
use minijinja::{Environment, UndefinedBehavior, Value, context};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// Twenty Questions, as a configuration file's format check names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TwentyQuestions;

impl Program for TwentyQuestions {
    const NAME: &'static str = "twenty-questions";
}

/// A configuration file's `format` key, as `twenty-questions` reads it.
pub type Format = free_agent_game::config::Format<TwentyQuestions>;

pub use twenty_questions_gui_types::settings::{GUESSES, NAMES};

/// How long a game lasts unless the file says.
pub const TIME_LIMIT: Duration = Duration::from_secs(600);

/// The shortest game.
pub const SHORTEST: Duration = Duration::from_secs(30);

/// How many players sit at the table unless the file says.
pub const PLAYERS: usize = 4;

/// The temperature the host picks at unless the file says.
pub const TEMPERATURE: f64 = 1.2;

/// The variables a player's prompt is rendered with besides the parts.
const PLAYER_VARIABLES: [&str; 3] = ["name", "others", "guesses"];

/// The variables the host's prompt is rendered with besides the parts.
const HOST_VARIABLES: [&str; 2] = ["object", "players"];

/// The variable the pick is rendered with besides the parts.
const PICK_VARIABLE: &str = "category";

/// What the host's prompt is checked with when the file names no object.
const PLACEHOLDER: &str = "a teapot";

/// What an error is about when it is about one prompt or one prompt part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum About {
    /// The system prompt of the strategy named.
    Strategy(String),
    /// The host's system prompt.
    Host,
    /// The host's pick.
    Pick,
    /// The prompt part named.
    Part(String),
    /// The seats allotted to strategies.
    Allotments,
}

/// An error, or an error's context, that says what it is [`About`], so
/// that it can be shown at that prompt or part.
#[derive(Debug)]
pub struct Placed {
    /// What it is about.
    pub about: About,
    /// What it says.
    said: String,
}

impl Placed {
    /// Saying `said` about `about`.
    fn new(about: About, said: String) -> Self {
        Placed { about, said }
    }
}

impl std::fmt::Display for Placed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.said)
    }
}

impl std::error::Error for Placed {}

/// The seat numbered `number`, from 1, named as [`NAMES`] names it.
pub fn named(number: usize) -> PlayerId {
    NAMES[number - 1].to_string()
}

/// The configuration file, as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The `format` key, checked as it is read.
    #[serde(default)]
    #[expect(dead_code, reason = "a format is checked as it is read")]
    format: Format,
    /// How many guesses the players have.
    #[serde(default = "guesses")]
    pub guesses: u64,
    /// How long the game lasts.
    #[serde(default = "time_limit", with = "humantime_serde")]
    pub time_limit: Duration,
    /// Who plays.
    #[serde(default)]
    pub players: Players,
    /// The host.
    pub host: HostConfig,
    /// The models, by name, and how to reach each.
    #[serde(default)]
    pub models: BTreeMap<String, Spec>,
    /// Named pieces of text the templates pull in with `{{ name }}`.
    #[serde(default)]
    pub prompt_parts: BTreeMap<String, String>,
    /// The strategies, by name.
    #[serde(default)]
    pub strategies: BTreeMap<String, Prompted>,
}

/// The default `guesses`, for `serde`.
fn guesses() -> u64 {
    GUESSES
}

/// The default `time_limit`, for `serde`.
fn time_limit() -> Duration {
    TIME_LIMIT
}

/// The `[players]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Players {
    /// How many play.
    #[serde(default = "players")]
    pub count: usize,
    /// The strategy every seat plays that `strategies` does not allot.
    pub strategy: Option<String>,
    /// How many seats play each other strategy, by name.
    #[serde(default)]
    pub strategies: BTreeMap<String, usize>,
}

impl Default for Players {
    fn default() -> Self {
        Players {
            count: PLAYERS,
            strategy: None,
            strategies: BTreeMap::new(),
        }
    }
}

/// The default `count`, for `serde`.
fn players() -> usize {
    PLAYERS
}

/// The `[host]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// The name of the model the host asks. The file may leave it out when
    /// it defines exactly one model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The secret object. Left out, the host picks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// The temperature the pick is asked at.
    #[serde(default = "temperature")]
    pub temperature: f64,
    /// The categories the pick draws one from, at random.
    #[serde(default)]
    pub categories: Vec<String>,
    /// The template the host is asked to pick an object with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<String>,
    /// The host's system prompt template.
    pub system: String,
}

/// The default `temperature`, for `serde`.
fn temperature() -> f64 {
    TEMPERATURE
}

/// Parse `text`, a configuration file's, and settle it. A document of a
/// newer [`Format`], one with a setting it does not have, or one that does
/// not parse or settle for any other reason, is an error, which `toml`
/// places at its line and column when it is the parser's. Parsing it says
/// nothing about the API keys, so a document can be checked without them.
pub fn parse(text: &str) -> anyhow::Result<Settings> {
    toml::from_str::<Config>(text)?.settle()
}

/// Read the configuration file at `path` once, and [`parse`] it. A file
/// that cannot be read or parsed is an invalid configuration file, and the
/// error names it.
pub fn load(path: &Path) -> anyhow::Result<Settings> {
    std::fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|text| parse(&text))
        .with_context(|| format!("invalid configuration file {}", path.display()))
}

impl Config {
    /// The settings that take effect, once the file is checked: the counts
    /// in range, the parts named as templates can pull them in and unlike
    /// any variable, every model a strategy or the host names defined, or
    /// left out of a file that defines exactly one, and every strategy
    /// allotted defined, with no more seats allotted than there are.
    pub fn settle(self) -> anyhow::Result<Settings> {
        let Config {
            guesses,
            time_limit,
            players,
            mut host,
            models,
            prompt_parts,
            mut strategies,
            ..
        } = self;
        if !(1..=GUESSES).contains(&guesses) {
            bail!("guesses is {guesses}; the players have 1 to {GUESSES} guesses");
        }
        if time_limit < SHORTEST {
            bail!(
                "time_limit is {}; a game lasts at least {}",
                humantime::format_duration(time_limit),
                humantime::format_duration(SHORTEST)
            );
        }
        if !(1..=NAMES.len()).contains(&players.count) {
            bail!(
                "[players] count is {}; a table seats 1 to {} players",
                players.count,
                NAMES.len()
            );
        }
        if !(0.0..=2.0).contains(&host.temperature) {
            bail!(
                "[host] temperature is {}; a temperature is from 0 to 2",
                host.temperature
            );
        }
        check_parts(&prompt_parts)?;
        if let Some((name, _)) = models
            .iter()
            .find(|(_, model)| model.context_window == Some(0))
        {
            bail!("[models.{name}] context_window is 0; a context window holds at least 1 token");
        }
        let only = || match models.keys().collect::<Vec<_>>()[..] {
            [only] => Some(only.clone()),
            _ => None,
        };
        let model = |named: &str, by: &str| -> anyhow::Result<String> {
            match named {
                "" => only().with_context(|| {
                    format!(
                        "{by} names no model, which it may leave out only when the file defines \
                         exactly one; it defines {}",
                        models.len()
                    )
                }),
                named if models.contains_key(named) => Ok(named.to_string()),
                named => bail!("{by} names the model {named}, which is not defined"),
            }
        };
        for (name, strategy) in &mut strategies {
            strategy.model = model(&strategy.model, &format!("[strategies.{name}]"))?;
        }
        host.model = Some(model(host.model.as_deref().unwrap_or_default(), "[host]")?);
        if host
            .object
            .as_deref()
            .is_some_and(|object| object.trim().is_empty())
        {
            bail!("[host] object is empty; name an object or leave it out");
        }
        if host.object.is_none() && host.pick.is_none() {
            bail!("[host] has neither an object nor a pick; the host needs one to know its object");
        }
        let allotments = allot(&players, &strategies)?;
        Ok(Settings {
            guesses,
            time_limit,
            seats: seats(players.count, named).collect(),
            allotments,
            strategies,
            host,
            models,
            prompt_parts,
        })
    }
}

/// Check that each of `parts` is named as a template can pull it in, and
/// unlike any variable and any other part.
fn check_parts(parts: &BTreeMap<String, String>) -> anyhow::Result<()> {
    let variables: Vec<&str> = PLAYER_VARIABLES
        .iter()
        .chain(&HOST_VARIABLES)
        .chain([&PICK_VARIABLE])
        .copied()
        .collect();
    let mut pulled = BTreeMap::new();
    for name in parts.keys() {
        let refused = |said: String| Placed::new(About::Part(name.clone()), said);
        if !is_part_name(name) {
            bail!(refused(format!(
                "[prompt_parts] has a part named {name:?}; a part's name is words of letters, \
                 digits and underscores, not starting with a digit"
            )));
        }
        let variable = part_variable(name);
        if variables.contains(&variable.as_str()) {
            bail!(refused(format!(
                "[prompt_parts] has a part named {name:?}, which is a template variable"
            )));
        }
        if let Some(other) = pulled.insert(variable.clone(), name) {
            bail!(refused(format!(
                "[prompt_parts] has parts named {other:?} and {name:?}, which are both pulled \
                 in with {{{{ {variable} }}}}"
            )));
        }
    }
    Ok(())
}

/// How many seats play each strategy, by name, as `players` allots them:
/// its `strategies` first, then its `strategy` for every seat left, which
/// may be left out when the file defines exactly one strategy, of
/// `strategies`. A strategy allotted that is not defined, more seats
/// allotted than there are, or seats left with no strategy to fill them,
/// is an error naming the key.
fn allot(
    players: &Players,
    strategies: &BTreeMap<String, Prompted>,
) -> anyhow::Result<BTreeMap<String, usize>> {
    let defined = |name: &String, key: &str| -> anyhow::Result<()> {
        if !strategies.contains_key(name) {
            bail!("{key} names the strategy {name}, which is not defined");
        }
        Ok(())
    };
    let mut allotted = BTreeMap::new();
    for (name, seats) in &players.strategies {
        defined(name, "[players] strategies")?;
        if *seats > 0 {
            allotted.insert(name.clone(), *seats);
        }
    }
    let taken: usize = allotted.values().sum();
    if taken > players.count {
        bail!(Placed::new(
            About::Allotments,
            format!(
                "[players] strategies allots {taken} seats, more than the {} players",
                players.count
            )
        ));
    }
    let left = players.count - taken;
    if left > 0 {
        let filling = match (
            &players.strategy,
            &strategies.keys().collect::<Vec<_>>()[..],
        ) {
            (Some(name), _) => name.clone(),
            (None, [only]) => (*only).clone(),
            (None, _) => bail!(
                "[players] strategy names no strategy, which it may leave out only when the \
                 file defines exactly one; it defines {}",
                strategies.len()
            ),
        };
        defined(&filling, "[players] strategy")?;
        *allotted.entry(filling).or_default() += left;
    }
    Ok(allotted)
}

/// The settings that took effect.
#[derive(Debug)]
pub struct Settings {
    /// How many guesses the players have.
    pub guesses: u64,
    /// How long the game lasts.
    pub time_limit: Duration,
    /// Every seat, in order.
    pub seats: Vec<PlayerId>,
    /// How many seats play each strategy.
    pub allotments: BTreeMap<String, usize>,
    /// Every strategy, each naming its model.
    pub strategies: BTreeMap<String, Prompted>,
    /// The host, naming its model.
    pub host: HostConfig,
    /// The models, by name.
    pub models: BTreeMap<String, Spec>,
    /// The `[prompt_parts]` as written.
    pub prompt_parts: BTreeMap<String, String>,
}

/// The host's system prompt, ready to be rendered for its object.
#[derive(Debug, Clone)]
pub struct HostPrompt {
    template: String,
    variables: Value,
}

impl HostPrompt {
    /// The host's prompt `template`, at a table of `players`, pulling in
    /// `parts`, each by the variable that pulls it in.
    pub fn new(template: &str, players: &[PlayerId], parts: &BTreeMap<String, String>) -> Self {
        HostPrompt {
            template: template.to_string(),
            variables: merge_maps([context! { players => in_prose(players) }, pulled(parts)]),
        }
    }

    /// The prompt, rendered for `object`.
    ///
    /// # Errors
    ///
    /// A template that does not compile or render.
    pub fn render(&self, object: &str) -> anyhow::Result<String> {
        render(
            &self.template,
            merge_maps([context! { object => object }, self.variables.clone()]),
        )
        .context(Placed::new(
            About::Host,
            "rendering the host's prompt".to_string(),
        ))
    }
}

/// Every prompt a game could call for, rendered: each player's for every
/// seat and every strategy at the table, the host's ready for its object,
/// and the pick for each category.
#[derive(Debug)]
pub struct Prompts {
    /// Each player's prompt, by seat and strategy.
    pub(crate) players: BTreeMap<(PlayerId, String), String>,
    /// The host's.
    pub(crate) host: HostPrompt,
    /// The pick for each category, or with none when there are no
    /// categories, if the file has a pick.
    pub(crate) picks: Vec<(Option<String>, String)>,
}

/// `parts`, each by the variable that pulls it in.
fn pulled(parts: &BTreeMap<String, String>) -> Value {
    let parts: BTreeMap<String, String> = parts
        .iter()
        .map(|(name, text)| (part_variable(name), text.clone()))
        .collect();
    Value::from(parts)
}

/// `template` rendered with `variables`, any undefined variable an error.
fn render(template: &str, variables: Value) -> anyhow::Result<String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    Ok(env.render_str(template, variables)?)
}

impl Settings {
    /// Every strategy at the table, with how many seats play it.
    fn at_the_table(&self) -> impl Iterator<Item = (&String, &Prompted)> {
        self.allotments
            .keys()
            .map(|name| (name, &self.strategies[name]))
    }

    /// Render every prompt a game could call for. A template that does not
    /// render is an error naming it, and a player's the seat too.
    pub fn prompts(&self) -> anyhow::Result<Prompts> {
        let parts = pulled(&self.prompt_parts);
        let mut players = BTreeMap::new();
        for seat in &self.seats {
            let others: Vec<&PlayerId> = self.seats.iter().filter(|other| *other != seat).collect();
            for (name, strategy) in self.at_the_table() {
                let variables = context! {
                    name => seat,
                    others => in_prose(others.clone()),
                    guesses => self.guesses,
                };
                let text = render(&strategy.system, merge_maps([variables, parts.clone()]))
                    .with_context(|| {
                        let said = format!("rendering the prompt of strategy {name} for {seat}");
                        Placed::new(About::Strategy(name.clone()), said)
                    })?;
                players.insert((seat.clone(), name.clone()), text);
            }
        }
        let host = HostPrompt::new(&self.host.system, &self.seats, &self.prompt_parts);
        host.render(self.host.object.as_deref().unwrap_or(PLACEHOLDER))?;
        let mut picks = Vec::new();
        if let Some(pick) = &self.host.pick {
            let categories: Vec<Option<&String>> = match self.host.categories.is_empty() {
                true => vec![None],
                false => self.host.categories.iter().map(Some).collect(),
            };
            for category in categories {
                let variables = match category {
                    Some(category) => context! { category => category },
                    None => context! {},
                };
                let text =
                    render(pick, merge_maps([variables, parts.clone()])).with_context(|| {
                        Placed::new(About::Pick, "rendering the host's pick".to_string())
                    })?;
                picks.push((category.cloned(), text));
            }
        }
        Ok(Prompts {
            players,
            host,
            picks,
        })
    }

    /// The models in play, by name: those a strategy at the table asks,
    /// and the host's.
    pub fn models_in_play(&self) -> BTreeMap<&str, &Spec> {
        self.at_the_table()
            .map(|(_, strategy)| strategy.model.as_str())
            .chain(self.host.model.as_deref())
            .map(|name| (name, &self.models[name]))
            .collect()
    }

    /// The most a request to the model `name` takes of the context window
    /// before its story, by estimate: the most a player's asking it takes,
    /// for any seat and strategy, and the host's if it asks it.
    pub fn needs(&self, name: &str, prompts: &Prompts) -> u64 {
        let id = &self.models[name].id;
        let players = prompts
            .players
            .iter()
            .filter(|((_, strategy), _)| self.strategies[strategy].model == name)
            .map(|(_, prompt)| player::unstoried(id, prompt, self.guesses, self.time_limit));
        let host = (self.host.model.as_deref() == Some(name)).then(|| {
            let object = self.host.object.as_deref().unwrap_or(PLACEHOLDER);
            let prompt = prompts.host.render(object).unwrap_or_default();
            host::unstoried(id, &prompt, &prompts.picks, self.guesses, self.time_limit)
        });
        players.chain(host).max().unwrap_or_default()
    }

    /// Set up each model in play as [`spec::set_up`] sets it up, settling
    /// its context window for `prompts` and asking it to play as
    /// [`plays`] probes it, which settles the tool choice, with its key
    /// looked up in `keys`. A model the file defines and nobody asks is not
    /// contacted.
    pub async fn set_up(
        &self,
        prompts: &Prompts,
        keys: &dyn Keys,
    ) -> anyhow::Result<(Models, Handles)> {
        let needed = |name: &str| Ok(self.needs(name, prompts));
        let probe = async |provider: &Provider, id: &str| plays(provider, id).await;
        spec::set_up(self.models_in_play(), keys, needed, probe).await
    }

    /// A fresh deal of the strategies to the seats, in a new random order,
    /// so that a name says nothing about a strategy.
    pub fn deal(&self) -> BTreeMap<PlayerId, String> {
        let mut strategies: Vec<String> = self
            .allotments
            .iter()
            .flat_map(|(name, seats)| std::iter::repeat_n(name.clone(), *seats))
            .collect();
        strategies.shuffle(&mut rand::rng());
        self.seats.iter().cloned().zip(strategies).collect()
    }

    /// The configuration a game of `deal` runs under, as the log records
    /// it: the version playing it, the guesses and the time limit, the
    /// host as the file has it, every strategy and the allotments,
    /// `models`, the models in play as they were set up, each player's
    /// system prompt from `prompts`, and the prompt parts as written.
    pub fn configuration(
        &self,
        deal: &BTreeMap<PlayerId, String>,
        prompts: &Prompts,
        models: Models,
    ) -> Configuration {
        Configuration {
            version: env!("CARGO_PKG_VERSION").to_string(),
            guesses: self.guesses,
            time_limit: self.time_limit,
            host: self.host.clone(),
            strategies: self.strategies.clone(),
            allotments: self.allotments.clone(),
            models,
            prompts: deal
                .iter()
                .map(|(seat, strategy)| {
                    let prompt = prompts.players[&(seat.clone(), strategy.clone())].clone();
                    (seat.clone(), prompt)
                })
                .collect(),
            prompt_parts: self.prompt_parts.clone(),
        }
    }
}

/// The configuration a game ran under, as it took effect once the file and
/// the defaults were combined, so that a log says by itself what was played
/// and under which prompts. The API key is never recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Configuration {
    /// The version of `twenty-questions` that played the game.
    pub version: String,
    /// How many guesses the players had.
    pub guesses: u64,
    /// How long the game could last.
    #[serde(with = "humantime_serde")]
    pub time_limit: Duration,
    /// The host, naming its model, with its prompts as written.
    pub host: HostConfig,
    /// Every strategy, by name, naming its model.
    pub strategies: BTreeMap<String, Prompted>,
    /// How many seats played each strategy.
    pub allotments: BTreeMap<String, usize>,
    /// The models in play, by name.
    pub models: Models,
    /// Each player's rendered system prompt.
    pub prompts: BTreeMap<PlayerId, String>,
    /// The `[prompt_parts]` as written.
    pub prompt_parts: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example configuration's text.
    fn example() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/configs/prompted.toml"
        ))
        .unwrap()
    }

    /// A small configuration: two models, one strategy each, and the host
    /// on the first, with `rest` after it.
    const SMALL: &str = r#"
[players]
count = 3
strategy = "plain"
strategies = { chatty = 1 }

[host]
model = "qwen"
object = "a teapot"
system = "{{ intro }} The object is {{ object }}. The players are {{ players }}."

[models.qwen]
base_url = "http://localhost:1234/v1"
id = "qwen2.5-7b-instruct"

[models.llama]
base_url = "http://localhost:1234/v1"
id = "llama-3.2-3b-instruct"

[prompt_parts]
intro = "Twenty Questions."

[strategies.plain]
model = "qwen"
system = "{{ intro }} You are {{ name }}, with {{ others }}, and {{ guesses }} guesses."

[strategies.chatty]
model = "llama"
system = "You are {{ name }}. Talk a lot."
"#;

    /// The settings `text` settles into.
    fn settled(text: &str) -> Settings {
        parse(text).unwrap()
    }

    /// The error `text` is refused with, causes included.
    fn refused(text: &str) -> String {
        format!("{:#}", parse(text).unwrap_err())
    }

    /// [`SMALL`] with `edit` made to it as a table.
    fn edited(edit: impl FnOnce(&mut toml::Table)) -> String {
        let mut table: toml::Table = toml::from_str(SMALL).unwrap();
        edit(&mut table);
        toml::to_string(&table).unwrap()
    }

    /// The table `key` names in `table`.
    fn at<'a>(table: &'a mut toml::Table, key: &str) -> &'a mut toml::Table {
        table[key].as_table_mut().unwrap()
    }

    #[test]
    fn the_example_settles_with_the_defaults_and_renders_every_prompt() {
        let settings = settled(&example());
        assert_eq!(settings.guesses, GUESSES);
        assert_eq!(settings.time_limit, TIME_LIMIT);
        assert_eq!(settings.seats, ["Alice", "Bob", "Carol"]);
        assert_eq!(
            settings.allotments,
            BTreeMap::from(["closer", "keeper", "scout"].map(|part| (part.to_string(), 1)))
        );
        assert_eq!(settings.host.model.as_deref(), Some("qwen"));
        assert_eq!(settings.host.object, None);
        assert_eq!(settings.host.temperature, TEMPERATURE);
        let prompts = settings.prompts().unwrap();
        assert_eq!(prompts.picks.len(), settings.host.categories.len());
        assert_eq!(prompts.picks[0].0.as_deref(), Some("a farm animal"));
        assert!(
            prompts.picks[0]
                .1
                .contains("Twenty Questions: a farm animal.")
        );
        let alice = &prompts.players[&("Alice".to_string(), "scout".to_string())];
        assert!(
            alice.contains("You are Alice, a player, with Bob and Carol."),
            "{alice}"
        );
        let host = prompts.host.render("a banjo").unwrap();
        assert!(host.contains("The secret object is a banjo."), "{host}");
    }

    #[test]
    fn a_file_with_no_players_table_seats_four_and_one_strategy_fills_every_seat() {
        let settings = settled(&edited(|table| {
            table.remove("players");
            at(table, "strategies").remove("chatty");
        }));
        assert_eq!(settings.seats.len(), PLAYERS);
        assert_eq!(
            settings.allotments,
            BTreeMap::from([("plain".to_string(), 4)])
        );
    }

    #[test]
    fn a_players_table_with_no_count_seats_four() {
        let settings = settled(&edited(|table| {
            at(table, "players").remove("count");
        }));
        assert_eq!(settings.seats, ["Alice", "Bob", "Carol", "Dave"]);
        assert_eq!(settings.allotments["plain"], 3);
    }

    #[test]
    fn every_seat_is_rendered_for_every_strategy_and_the_deal_shuffles_the_strategies() {
        let settings = settled(SMALL);
        let prompts = settings.prompts().unwrap();
        assert_eq!(prompts.players.len(), 6);
        assert_eq!(
            prompts.players[&("Bob".to_string(), "plain".to_string())],
            "Twenty Questions. You are Bob, with Alice and Carol, and 20 guesses."
        );
        assert_eq!(
            prompts.host.render("a teapot").unwrap(),
            "Twenty Questions. The object is a teapot. The players are Alice, Bob and Carol."
        );
        assert!(prompts.picks.is_empty());
        let mut dealt = std::collections::BTreeSet::new();
        for _ in 0..100 {
            let deal = settings.deal();
            let chatty: Vec<&String> = deal
                .iter()
                .filter(|(_, strategy)| *strategy == "chatty")
                .map(|(seat, _)| seat)
                .collect();
            assert_eq!(chatty.len(), 1);
            dealt.insert(chatty[0].clone());
        }
        assert_eq!(dealt.len(), 3, "{dealt:?}");
        let deal = settings.deal();
        let configuration = settings.configuration(&deal, &prompts, Models::new());
        assert_eq!(configuration.prompts.len(), 3);
        assert_eq!(configuration.allotments, settings.allotments);
        assert_eq!(configuration.host, settings.host);
        let written = serde_json::to_string(&configuration).unwrap();
        assert_eq!(
            serde_json::from_str::<Configuration>(&written).unwrap(),
            configuration
        );
    }

    #[test]
    fn the_models_in_play_are_those_a_strategy_at_the_table_or_the_host_asks() {
        let settings = settled(SMALL);
        let names: Vec<&str> = settings.models_in_play().into_keys().collect();
        assert_eq!(names, ["llama", "qwen"]);
        let alone = settled(&edited(|table| {
            at(table, "players").remove("strategies");
        }));
        let names: Vec<&str> = alone.models_in_play().into_keys().collect();
        assert_eq!(names, ["qwen"]);
        let prompts = settings.prompts().unwrap();
        // The host and a player ask qwen; only a player asks llama.
        assert!(settings.needs("qwen", &prompts) > settings.needs("llama", &prompts));
    }

    #[test]
    fn a_model_left_out_is_the_only_one_the_file_defines() {
        let settings = settled(&edited(|table| {
            at(table, "models").remove("llama");
            at(table, "host").remove("model");
            at(at(table, "strategies"), "chatty").remove("model");
            at(at(table, "strategies"), "plain").remove("model");
        }));
        assert_eq!(settings.host.model.as_deref(), Some("qwen"));
        assert_eq!(settings.strategies["chatty"].model, "qwen");
    }

    #[test]
    fn a_setting_out_of_range_is_refused_naming_its_key() {
        for (key, value, error) in [
            (
                "guesses",
                toml::Value::from(0),
                "guesses is 0; the players have 1 to 20 guesses",
            ),
            (
                "guesses",
                toml::Value::from(21),
                "guesses is 21; the players have 1 to 20 guesses",
            ),
            (
                "time_limit",
                toml::Value::from("29s"),
                "time_limit is 29s; a game lasts at least 30s",
            ),
        ] {
            let text = edited(|table| {
                table.insert(key.to_string(), value);
            });
            assert_eq!(refused(&text), error);
        }
        for count in [0, 13] {
            let text = edited(|table| {
                at(table, "players").insert("count".to_string(), toml::Value::from(count));
            });
            assert_eq!(
                refused(&text),
                format!("[players] count is {count}; a table seats 1 to 12 players")
            );
        }
        for temperature in [-0.1, 2.1] {
            let text = edited(|table| {
                at(table, "host").insert("temperature".to_string(), toml::Value::from(temperature));
            });
            assert_eq!(
                refused(&text),
                format!("[host] temperature is {temperature}; a temperature is from 0 to 2")
            );
        }
    }

    #[test]
    fn a_part_named_so_no_template_can_pull_it_in_or_like_a_variable_is_refused() {
        let with_part = |name: &str| {
            edited(|table| {
                at(table, "prompt_parts").insert(name.to_string(), toml::Value::from("text"));
            })
        };
        assert!(refused(&with_part("2nd")).contains("a part's name is words of letters"));
        for variable in ["name", "others", "guesses", "object", "players", "category"] {
            assert_eq!(
                refused(&with_part(variable)),
                format!(
                    "[prompt_parts] has a part named {variable:?}, which is a template variable"
                )
            );
        }
        assert_eq!(
            refused(&with_part("intro ")),
            "[prompt_parts] has a part named \"intro \"; a part's name is words of letters, \
             digits and underscores, not starting with a digit"
        );
        let both = edited(|table| {
            let parts = at(table, "prompt_parts");
            parts.insert("House rules".to_string(), toml::Value::from("a"));
            parts.insert("House_rules".to_string(), toml::Value::from("b"));
        });
        assert_eq!(
            refused(&both),
            "[prompt_parts] has parts named \"House rules\" and \"House_rules\", which are both \
             pulled in with {{ House_rules }}"
        );
    }

    #[test]
    fn a_model_that_is_not_defined_or_has_no_room_is_refused_naming_who_names_it() {
        let text = edited(|table| {
            at(at(table, "models"), "qwen")
                .insert("context_window".to_string(), toml::Value::from(0));
        });
        assert_eq!(
            refused(&text),
            "[models.qwen] context_window is 0; a context window holds at least 1 token"
        );
        let text = edited(|table| {
            at(table, "host").insert("model".to_string(), toml::Value::from("gpt"));
        });
        assert_eq!(
            refused(&text),
            "[host] names the model gpt, which is not defined"
        );
        let text = edited(|table| {
            at(table, "host").remove("model");
        });
        assert_eq!(
            refused(&text),
            "[host] names no model, which it may leave out only when the file defines exactly \
             one; it defines 2"
        );
        let text = edited(|table| {
            at(at(table, "strategies"), "chatty")
                .insert("model".to_string(), toml::Value::from("gpt"));
        });
        assert_eq!(
            refused(&text),
            "[strategies.chatty] names the model gpt, which is not defined"
        );
    }

    #[test]
    fn the_host_needs_an_object_or_a_pick() {
        let text = edited(|table| {
            at(table, "host").insert("object".to_string(), toml::Value::from(" "));
        });
        assert_eq!(
            refused(&text),
            "[host] object is empty; name an object or leave it out"
        );
        let text = edited(|table| {
            at(table, "host").remove("object");
        });
        assert_eq!(
            refused(&text),
            "[host] has neither an object nor a pick; the host needs one to know its object"
        );
        let picking = settled(&edited(|table| {
            let host = at(table, "host");
            host.remove("object");
            host.insert("pick".to_string(), toml::Value::from("Pick {{ intro }}"));
        }));
        let prompts = picking.prompts().unwrap();
        assert_eq!(
            prompts.picks,
            [(None, "Pick Twenty Questions.".to_string())]
        );
    }

    #[test]
    fn seats_allotted_to_strategies_that_are_not_there_or_that_do_not_fit_are_refused() {
        let text = edited(|table| {
            let players = at(table, "players");
            players.insert(
                "strategies".to_string(),
                toml::Value::from(toml::Table::from_iter([(
                    "loud".to_string(),
                    toml::Value::from(1),
                )])),
            );
        });
        assert_eq!(
            refused(&text),
            "[players] strategies names the strategy loud, which is not defined"
        );
        let text = edited(|table| {
            let players = at(table, "players");
            players.insert(
                "strategies".to_string(),
                toml::Value::from(toml::Table::from_iter([(
                    "chatty".to_string(),
                    toml::Value::from(4),
                )])),
            );
        });
        assert_eq!(
            refused(&text),
            "[players] strategies allots 4 seats, more than the 3 players"
        );
        let text = edited(|table| {
            at(table, "players").insert("strategy".to_string(), toml::Value::from("loud"));
        });
        assert_eq!(
            refused(&text),
            "[players] strategy names the strategy loud, which is not defined"
        );
        let text = edited(|table| {
            at(table, "players").remove("strategy");
        });
        assert_eq!(
            refused(&text),
            "[players] strategy names no strategy, which it may leave out only when the file \
             defines exactly one; it defines 2"
        );
        // Every seat allotted, none is left to fill, and none allotted 0.
        let full = settled(&edited(|table| {
            let players = at(table, "players");
            players.remove("strategy");
            players.insert(
                "strategies".to_string(),
                toml::Value::from(toml::Table::from_iter([
                    ("chatty".to_string(), toml::Value::from(3)),
                    ("plain".to_string(), toml::Value::from(0)),
                ])),
            );
        }));
        assert_eq!(full.allotments, BTreeMap::from([("chatty".to_string(), 3)]));
    }

    #[test]
    fn a_template_that_does_not_render_is_refused_naming_it() {
        let text = edited(|table| {
            at(at(table, "strategies"), "chatty")
                .insert("system".to_string(), toml::Value::from("{{ object }}"));
        });
        let error = format!("{:#}", settled(&text).prompts().unwrap_err());
        assert!(
            error.starts_with("rendering the prompt of strategy chatty for Alice"),
            "{error}"
        );
        let text = edited(|table| {
            at(table, "host").insert("system".to_string(), toml::Value::from("{{ name }}"));
        });
        let error = format!("{:#}", settled(&text).prompts().unwrap_err());
        assert!(error.starts_with("rendering the host's prompt"), "{error}");
        let text = edited(|table| {
            let host = at(table, "host");
            host.insert("pick".to_string(), toml::Value::from("{{ category }}"));
        });
        let error = format!("{:#}", settled(&text).prompts().unwrap_err());
        assert!(error.starts_with("rendering the host's pick"), "{error}");
    }

    #[test]
    fn an_error_about_one_prompt_or_part_says_which() {
        let about = |error: anyhow::Error| error.downcast_ref::<Placed>().map(|p| p.about.clone());
        let text = edited(|table| {
            at(at(table, "strategies"), "chatty")
                .insert("system".to_string(), toml::Value::from("{{ object }}"));
        });
        let error = settled(&text).prompts().unwrap_err();
        assert_eq!(about(error), Some(About::Strategy("chatty".to_string())));
        let text = edited(|table| {
            at(table, "host").insert("system".to_string(), toml::Value::from("{{ name }}"));
        });
        assert_eq!(
            about(settled(&text).prompts().unwrap_err()),
            Some(About::Host)
        );
        let text = edited(|table| {
            at(table, "host").insert("pick".to_string(), toml::Value::from("{{ object }}"));
        });
        assert_eq!(
            about(settled(&text).prompts().unwrap_err()),
            Some(About::Pick)
        );
        for part in ["2nd", "object"] {
            let text = edited(|table| {
                at(table, "prompt_parts").insert(part.to_string(), toml::Value::from("text"));
            });
            let error = parse(&text).unwrap_err();
            assert_eq!(about(error), Some(About::Part(part.to_string())));
        }
        let error = parse(&edited(|table| {
            let allotted = toml::Value::from(toml::Table::from_iter([(
                "chatty".to_string(),
                toml::Value::from(4),
            )]));
            at(table, "players").insert("strategies".to_string(), allotted);
        }))
        .unwrap_err();
        assert_eq!(about(error), Some(About::Allotments));
        let error = parse(&edited(|table| {
            table.insert("guesses".to_string(), toml::Value::from(0));
        }))
        .unwrap_err();
        assert_eq!(about(error), None);
    }

    #[test]
    fn an_unknown_key_or_a_newer_format_is_refused_where_it_is() {
        let error = refused(&format!("{SMALL}\n[host.extra]\n"));
        assert!(error.contains("unknown field `extra`"), "{error}");
        let error = refused(&format!("format = 2\n{SMALL}"));
        assert!(
            error.contains("This version of twenty-questions reads formats up to 1."),
            "{error}"
        );
        let error = refused("[players]\ncount = 3\n");
        assert!(error.contains("missing field `host`"), "{error}");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_invalid_configuration_file_naming_it() {
        let error = load(Path::new("no-such-file.toml")).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("invalid configuration file no-such-file.toml"),
            "{error}"
        );
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/configs/prompted.toml"
        ));
        assert_eq!(load(path).unwrap().seats.len(), 3);
        assert_eq!(named(12), "Leo");
    }
}
