//! Getting a game ready to play, from a configuration's text and the
//! [`Keys`] its models' API keys come from. [`prepare`] does everything
//! that can fail before the game starts: it settles the configuration,
//! renders every prompt, sets up and probes every model in play, and deals
//! the seats. What it gives back builds each episode, dealt afresh and
//! logging to a logger its caller owns, and knows the keys it loaded, which
//! no log carries.

use crate::avalon::Avalon;
use crate::config::{self, Configuration, Name, Settings, Settled, SystemPrompt};
use crate::game::{self, Actor, Entry, Game};
use crate::werewolf::Werewolf;
use free_agent::{Episode, Logger};
use free_agent_game::log::Secrets;
use free_agent_game::seat::{Dealt, PlayerId};
use free_agent_model::spec::{Handles, Keys, Models};
use secrecy::SecretString;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// A game ready to start, of either kind, boxed, since the two differ in
/// size.
pub enum Prepared {
    /// A game of Werewolf.
    Werewolf(Box<Ready<Werewolf>>),
    /// A game of Avalon.
    Avalon(Box<Ready<Avalon>>),
}

impl Prepared {
    /// The game `settled` describes, made ready as [`Ready::new`] makes it,
    /// with its models' keys looked up in `keys`.
    pub async fn new(settled: Settled, keys: &dyn Keys) -> anyhow::Result<Self> {
        Ok(match settled {
            Settled::Werewolf(settings) => {
                Prepared::Werewolf(Box::new(Ready::new(*settings, keys).await?))
            }
            Settled::Avalon(settings) => {
                Prepared::Avalon(Box::new(Ready::new(*settings, keys).await?))
            }
        })
    }

    /// Which game this is.
    pub fn game(&self) -> Name {
        match self {
            Prepared::Werewolf(_) => Name::Werewolf,
            Prepared::Avalon(_) => Name::Avalon,
        }
    }
}

/// The game the configuration `text` describes, parsed as
/// [`config::parse`] parses it and made ready as [`Prepared::new`] makes
/// it, with its models' keys looked up in `keys`.
pub async fn prepare(text: &str, keys: &dyn Keys) -> anyhow::Result<Prepared> {
    Prepared::new(config::parse(text)?, keys).await
}

/// A game of `G` ready to start: the settings it is played under, every
/// system prompt they render, the models in play as they were set up, the
/// handle each is asked through, and the keys they were set up with.
pub struct Ready<G: Game> {
    settings: Settings<G>,
    prompts: Vec<SystemPrompt<G::Role>>,
    models: Models,
    handles: Handles,
    secrets: Secrets,
}

impl<G: Game> Ready<G> {
    /// A game under `settings`, made ready. Every prompt is rendered, and
    /// every model in play set up with its key looked up in `keys`, so that
    /// a template that is broken or a model that does not play fails before
    /// any actor is built.
    pub async fn new(settings: Settings<G>, keys: &dyn Keys) -> anyhow::Result<Self> {
        let prompts = settings.prompts()?;
        let loading = Loading {
            keys,
            loaded: Mutex::default(),
        };
        let (models, handles) = settings.set_up(&prompts, &loading).await?;
        Ok(Ready {
            settings,
            prompts,
            models,
            handles,
            secrets: loading.loaded.into_inner().unwrap().into_iter().collect(),
        })
    }

    /// The keys the models in play were set up with, for the log's
    /// [`Writer`](crate::log::Writer) to leave out.
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// A fresh deal of the (role, strategy) pairs to the seats, in a new
    /// random order, with the configuration its log records. Every seat's
    /// prompt is rendered for every pair, so any deal has its prompts.
    fn dealt(&self) -> (HashMap<PlayerId, Dealt<G::Role>>, Configuration<G>) {
        let deal = self.settings.lineup.deal();
        let configuration = self
            .settings
            .configuration(&deal, &self.prompts, self.models.clone())
            .expect("every seat's prompt is rendered for every pair");
        (deal, configuration)
    }

    /// An episode of this game, dealt afresh, logging to `logger`, with
    /// where the winning team comes once a side wins. Every episode has
    /// the same seats and the same (role, strategy) pairs, and asks the
    /// models as they were set up.
    pub fn episode(
        &self,
        logger: Logger<Entry<G>>,
    ) -> (Episode<Actor<G>>, oneshot::Receiver<G::Team>) {
        let (deal, configuration) = self.dealt();
        let players = config::players(&configuration, &deal, &self.handles);
        let (winner, won) = oneshot::channel();
        let episode = game::game(
            deal,
            self.settings.rules(),
            configuration,
            winner,
            players,
            logger,
        );
        (episode, won)
    }
}

/// Keys that keep a copy of every key they give out.
struct Loading<'a> {
    keys: &'a dyn Keys,
    loaded: Mutex<Vec<SecretString>>,
}

impl Keys for Loading<'_> {
    fn key(&self, variable: &str) -> anyhow::Result<Option<SecretString>> {
        let key = self.keys.key(variable)?;
        self.loaded.lock().unwrap().extend(key.clone());
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Writer;
    use crate::model::canned::{
        CANARY, CANARY_VARIABLE, assert_redacted, leaking, leaking_werewolf, playing,
    };
    use crate::{avalon, werewolf};
    use free_agent::Event;
    use free_agent_game::log::Ended;
    use free_agent_model::secret::Keychain;
    use free_agent_model::secret::testing::Fake;
    use free_agent_model::spec::Environment;
    use secrecy::SecretString;
    use std::collections::HashSet;
    use std::path::Path;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

    /// The text of the example file `name` in `configs/`.
    fn example(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("configs")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    /// Every entry `ready`'s episode logs as it plays to the end, which a
    /// side wins.
    async fn played<G: Game>(ready: Ready<G>) -> Vec<Entry<G>> {
        let (logger, mut log) = unbounded_channel();
        let (episode, won) = ready.episode(logger);
        episode.run(Duration::from_secs(60 * 60)).await.unwrap();
        won.await.unwrap();
        let mut entries = Vec::new();
        while let Some(Event { payload, .. }) = log.recv().await {
            entries.push(payload);
        }
        entries
    }

    #[tokio::test(start_paused = true)]
    async fn a_random_game_is_prepared_from_its_text_as_the_game_it_names_and_plays_to_the_end() {
        for (name, game) in [
            ("werewolf/random.toml", Name::Werewolf),
            ("avalon/random.toml", Name::Avalon),
        ] {
            let prepared = prepare(&example(name), &Environment).await.unwrap();
            assert_eq!(prepared.game(), game, "{name}");
            let ended = match prepared {
                Prepared::Werewolf(ready) => matches!(
                    played(*ready).await.last(),
                    Some(Entry::Event(werewolf::Outcome::End { .. }))
                ),
                Prepared::Avalon(ready) => matches!(
                    played(*ready).await.last(),
                    Some(Entry::Event(avalon::Outcome::End { .. }))
                ),
            };
            assert!(ended, "{name}");
        }
    }

    #[tokio::test]
    async fn text_that_does_not_parse_is_not_prepared() {
        let error = prepare("game = \"chess\"\n", &Environment)
            .await
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("unknown variant `chess`"));
    }

    /// Every seat in `deal`, with the role and the strategy dealt it, in
    /// seat order.
    fn described<R: std::fmt::Display>(deal: &HashMap<PlayerId, Dealt<R>>) -> Vec<String> {
        let mut described: Vec<String> = deal
            .iter()
            .map(|(seat, dealt)| format!("{seat}: {} {}", dealt.role, dealt.strategy))
            .collect();
        described.sort();
        described
    }

    /// The (role, strategy) pairs `deal` deals, in order.
    fn pairs<R: Copy + Ord>(deal: &HashMap<PlayerId, Dealt<R>>) -> Vec<(R, &str)> {
        let mut pairs: Vec<(R, &str)> = deal
            .values()
            .map(|dealt| (dealt.role, dealt.strategy.as_str()))
            .collect();
        pairs.sort();
        pairs
    }

    /// The game of Werewolf the file `text` describes, made ready.
    async fn werewolf(text: &str) -> Ready<Werewolf> {
        let settings = werewolf::config::parse(text).unwrap().settle().unwrap();
        Ready::new(settings, &Environment).await.unwrap()
    }

    #[tokio::test]
    async fn each_deal_gives_the_same_pairs_to_the_same_seats_in_a_new_order() {
        let text = "[strategies.dice]\npolicy = \"random\"\n\
                    [strategies.coin]\npolicy = \"random\"\n\
                    [roles.werewolf.strategies]\ndice = 1\ncoin = 1\n\
                    [roles.villager.strategies]\ndice = 2\ncoin = 1\n";
        let ready = werewolf(text).await;
        let (first, _) = ready.dealt();
        let mut deals = HashSet::from([described(&first)]);
        for _ in 0..50 {
            let (deal, _) = ready.dealt();
            assert_eq!(pairs(&deal), pairs(&first));
            deals.insert(described(&deal));
        }
        // Seven seats dealt fifty times over are not dealt alike every time.
        assert!(deals.len() > 1, "{deals:?}");
    }

    #[tokio::test]
    async fn each_deal_tells_each_prompted_player_the_prompt_of_its_role() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let text = format!(
            "[models.qwen]\nbase_url = \"{base_url}\"\nid = \"qwen2.5-7b-instruct\"\n\
             [strategies.plain]\npolicy = \"prompted\"\nsystem = \"{{{{ name }}}} the {{{{ role }}}}\"\n\
             [roles.werewolf]\nstrategy = \"plain\"\n\
             [roles.villager]\nstrategy = \"plain\"\n\
             [roles.doctor]\nstrategy = \"plain\"\n\
             [roles.seer]\nstrategy = \"plain\"\n"
        );
        let ready = werewolf(&text).await;
        for _ in 0..3 {
            let (deal, configuration) = ready.dealt();
            assert_eq!(configuration.prompts.len(), deal.len());
            for (seat, dealt) in &deal {
                let told = format!("{seat} the {}", dealt.role);
                assert_eq!(configuration.prompts[seat], told);
            }
        }
    }

    /// Keys that hold none.
    struct Empty;

    impl Keys for Empty {
        fn key(&self, _: &str) -> anyhow::Result<Option<SecretString>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn a_model_whose_key_the_keys_do_not_hold_is_not_prepared() {
        let text = format!(
            "{}\n[models.gpt]\nbase_url = \"https://api.openai.com/v1\"\nid = \"gpt-0\"\n\
             [strategies.smart]\npolicy = \"prompted\"\nsystem = \"Play.\"\n",
            example("avalon/random.toml")
        );
        let text = text.replacen("strategy = \"random\"", "strategy = \"smart\"", 1);
        let error = prepare(&text, &Empty).await.err().unwrap();
        let shown = format!("{error:#}");
        assert_eq!(
            shown, "setting up the model gpt: OPENAI_API_KEY is not set",
            "{shown}"
        );
    }

    #[tokio::test]
    async fn a_game_whose_keys_come_from_the_keychain_logs_none_of_them() {
        let base_url = leaking(&["qwen2.5-7b-instruct"], CANARY);
        let keychain = Keychain(Fake::default());
        keychain
            .store(CANARY_VARIABLE, &SecretString::from(CANARY))
            .unwrap();
        let text = leaking_werewolf(&base_url);
        let settings = werewolf::config::parse(&text).unwrap().settle().unwrap();
        let ready = Ready::new(settings, &keychain).await.unwrap();
        let (logger, mut events) = unbounded_channel();
        let (episode, mut won) = ready.episode(logger);
        let secrets = ready.secrets().clone();
        let mut writer = Writer::<Werewolf, _>::start(Vec::new(), episode.id(), secrets).unwrap();
        let ran = episode.run(Duration::from_secs(60 * 60)).await;
        while let Some(event) = events.recv().await {
            writer.write(&event).unwrap();
        }
        let written = writer.end(Ended::of(&ran, won.try_recv().is_ok())).unwrap();
        let log = String::from_utf8(written).unwrap();
        assert_redacted(&log, CANARY);
        for line in log.lines() {
            serde_json::from_str::<Event<Entry<Werewolf>>>(line).unwrap();
        }
        assert!(log.ends_with("{\"Ended\":\"won\"}}\n"), "{log}");
    }
}
