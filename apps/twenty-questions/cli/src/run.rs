//! Getting a game ready to play, from a configuration's text and the
//! [`Keys`] its models' API keys come from. [`prepare`] does everything
//! that can fail before the game starts: it settles the configuration,
//! renders every prompt, and sets up and probes every model in play. What
//! it gives back builds each episode, dealt afresh and logging to a logger
//! its caller owns, and knows the keys it loaded, which no log carries.

use crate::config::{self, Prompts, Settings};
use crate::game::{self, Actor};
use crate::host::{Brief, Host};
use crate::log::{Entry, Side};
use crate::model::Asker;
use crate::prompted::Player;
use free_agent::{Episode, Logger};
use free_agent_game::log::Secrets;
use free_agent_model::spec::{Handles, Keys, Models};
use secrecy::SecretString;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long past its time limit an episode is waited for before it is
/// given up on: long enough for the slowest model call in flight at the
/// limit to come back.
pub(crate) const GRACE: Duration = Duration::from_secs(60 * 60);

/// The game the configuration `text` describes, parsed as
/// [`config::parse`] parses it and made ready as [`Ready::new`] makes it,
/// with its models' keys looked up in `keys`.
pub async fn prepare(text: &str, keys: &dyn Keys) -> anyhow::Result<Ready> {
    Ready::new(config::parse(text)?, keys).await
}

/// A game ready to start: the settings it is played under, every prompt
/// they render, the models in play as they were set up, the handle each is
/// asked through, and the keys they were set up with.
pub struct Ready {
    settings: Settings,
    prompts: Prompts,
    models: Models,
    handles: Handles,
    secrets: Secrets,
}

impl Ready {
    /// A game under `settings`, made ready. Every prompt is rendered, and
    /// every model in play set up with its key looked up in `keys`, so that
    /// a template that is broken or a model that does not play fails before
    /// any actor is built.
    pub async fn new(settings: Settings, keys: &dyn Keys) -> anyhow::Result<Self> {
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

    /// How long an episode is waited for: its time limit and an hour's
    /// grace.
    pub fn patience(&self) -> Duration {
        self.settings.time_limit + GRACE
    }

    /// How the model `name` is asked.
    fn asker(&self, name: &str) -> Asker {
        let model = &self.models[name];
        Asker {
            model_id: model.id.clone(),
            tool_choice: model.tool_choice,
            context_window: model.context_window.tokens,
            model: Arc::clone(&self.handles[name]),
        }
    }

    /// An episode of this game, the strategies dealt afresh, logging to
    /// `logger`, with where the winning side comes once the game is won.
    pub fn episode(&self, logger: Logger<Entry>) -> (Episode<Actor>, oneshot::Receiver<Side>) {
        let settings = &self.settings;
        let deal = settings.deal();
        let configuration = settings.configuration(&deal, &self.prompts, self.models.clone());
        let host = settings.host.model.as_deref().unwrap_or_default();
        let brief = Brief {
            players: settings.seats.clone(),
            strategies: deal.clone(),
            guesses: settings.guesses,
            time_limit: settings.time_limit,
            object: settings.host.object.clone(),
            picks: self.prompts.picks.clone(),
            temperature: settings.host.temperature,
            prompt: self.prompts.host.clone(),
            asker: self.asker(host),
            configuration: configuration.clone(),
        };
        let (winner, won) = oneshot::channel();
        let host = Host::init(brief, winner);
        let player = |seat: &String| {
            let strategy = &deal[seat];
            let prompt = configuration.prompts[seat].clone();
            Player::init(
                seat,
                prompt,
                self.asker(&settings.strategies[strategy].model),
            )
        };
        (game::game(&settings.seats, host, player, logger), won)
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
    use crate::host::CHOOSE;
    use crate::log::{Victory, Writer};
    use crate::model::canned::{
        CANARY, CANARY_VARIABLE, assert_redacted, leaking, playing, refusing, refusing_offers_of,
    };
    use crate::prompted::GUESS;
    use free_agent::Event;
    use free_agent_game::log::Ended;
    use free_agent_model::ToolChoice;
    use free_agent_model::secret::Keychain;
    use free_agent_model::secret::testing::Fake;
    use free_agent_model::spec::Environment;
    use tokio::sync::mpsc::unbounded_channel;

    /// A quick game of two players at the provider at `base_url`, with
    /// `host` as the host's table after its system prompt and `model` as
    /// the model's table after its id.
    fn quick(base_url: &str, host: &str, model: &str) -> String {
        format!(
            "time_limit = \"30s\"\n\
             [players]\ncount = 2\n\
             [host]\nsystem = \"You are the host. The object is {{{{ object }}}}.\"\n{host}\n\
             [models.qwen]\nbase_url = \"{base_url}\"\nid = \"qwen2.5-7b-instruct\"\n{model}\n\
             [strategies.plain]\nsystem = \"You are {{{{ name }}}}.\"\n"
        )
    }

    /// Every event `ready`'s episode logs as it plays, written as the log's
    /// writer writes it, and the side that won.
    async fn played(ready: &Ready) -> (String, Option<crate::log::Side>) {
        let (logger, mut events) = unbounded_channel();
        let (episode, mut won) = ready.episode(logger);
        let mut writer = Writer::start(Vec::new(), episode.id(), ready.secrets().clone()).unwrap();
        let ran = episode.run(ready.patience()).await;
        while let Some(event) = events.recv().await {
            writer.write(&event).unwrap();
        }
        let won = won.try_recv().ok();
        let written = writer.end(Ended::of(&ran, won.is_some())).unwrap();
        (String::from_utf8(written).unwrap(), won)
    }

    /// Every entry of `log`, in order.
    fn entries(log: &str) -> Vec<Entry> {
        log.lines()
            .map(|line| serde_json::from_str::<Event<Entry>>(line).unwrap().payload)
            .collect()
    }

    #[tokio::test]
    async fn a_game_is_prepared_from_its_text_and_plays_to_the_end() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let text = quick(&base_url, "object = \"a teapot\"", "");
        let ready = prepare(&text, &Environment).await.unwrap();
        assert_eq!(ready.patience(), Duration::from_secs(30) + GRACE);
        let (log, won) = played(&ready).await;
        assert_eq!(won, Some(Side::Players));
        let entries = entries(&log);
        let configuration = entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Configuration(configuration) => Some(configuration),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            configuration.models["qwen"].tool_choice,
            ToolChoice::Required
        );
        assert_eq!(
            configuration.prompts.values().collect::<Vec<_>>(),
            ["You are Alice.", "You are Bob."]
        );
        assert!(
            entries.iter().any(|entry| matches!(
                entry,
                Entry::End {
                    by: Victory::Named,
                    ..
                }
            )),
            "{log}"
        );
        assert!(log.ends_with("{\"Ended\":\"won\"}}\n"), "{log}");
    }

    #[tokio::test]
    async fn the_host_picks_an_object_when_the_file_names_none() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let text = quick(&base_url, "pick = \"Pick something.\"", "");
        let ready = prepare(&text, &Environment).await.unwrap();
        let (log, _) = played(&ready).await;
        assert!(
            entries(&log).contains(&Entry::Chosen {
                object: "a teapot".to_string(),
                category: None
            }),
            "{log}"
        );
    }

    #[tokio::test]
    async fn a_model_that_does_not_play_is_refused_before_any_game_in_the_provider_s_words() {
        let base_url = refusing(&["qwen2.5-7b-instruct"]);
        let text = quick(&base_url, "object = \"a teapot\"", "");
        let error = prepare(&text, &Environment).await.err().unwrap();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with(
                "setting up the model qwen: qwen2.5-7b-instruct did not play Twenty Questions: \
                 did not answer as a player:"
            ),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn a_model_is_asked_to_play_only_the_parts_it_plays() {
        // The host's model never guesses, and with the object named it
        // never picks.
        let players = playing(&["qwen2.5-7b-instruct"]);
        let host = refusing_offers_of(GUESS, &["judge"]);
        let text = format!(
            "{}model = \"qwen\"\n[models.judge]\nbase_url = \"{host}\"\nid = \"judge\"\n",
            quick(&players, "object = \"a teapot\"\nmodel = \"judge\"", "")
        );
        prepare(&text, &Environment).await.unwrap();
        let picker = refusing_offers_of(CHOOSE, &["qwen2.5-7b-instruct"]);
        let text = quick(&picker, "object = \"a teapot\"", "");
        prepare(&text, &Environment).await.unwrap();
        let text = quick(&picker, "pick = \"Pick something.\"", "");
        let error = prepare(&text, &Environment).await.err().unwrap();
        assert!(
            format!("{error:#}").contains("did not answer as the host picking an object"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn a_game_whose_keys_come_from_the_keychain_logs_none_of_them() {
        let base_url = leaking(&["qwen2.5-7b-instruct"], CANARY);
        let keychain = Keychain(Fake::default());
        keychain
            .store(CANARY_VARIABLE, &SecretString::from(CANARY))
            .unwrap();
        let text = quick(
            &base_url,
            "object = \"a teapot\"",
            &format!("api_key_env = \"{CANARY_VARIABLE}\""),
        );
        let ready = prepare(&text, &keychain).await.unwrap();
        let (log, won) = played(&ready).await;
        assert_redacted(&log, CANARY);
        assert_eq!(won, Some(Side::Players));
    }
}
