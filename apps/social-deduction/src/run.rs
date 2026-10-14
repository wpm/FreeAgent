//! Getting a game ready to play, from a configuration's text and the
//! [`Keys`] its models' API keys come from. [`prepare`] does everything
//! that can fail before the game starts: it settles the configuration,
//! renders every prompt, sets up and probes every model in play, and deals
//! the seats. What it gives back builds the episode, logging to a logger
//! its caller owns.

use crate::avalon::Avalon;
use crate::config::{self, Configuration, Name, Settings, Settled};
use crate::game::{self, Actor, Entry, Game};
use crate::model::spec::{Handles, Keys};
use crate::seat::{Dealt, PlayerId};
use crate::werewolf::Werewolf;
use free_agent::{Episode, Logger};
use std::collections::HashMap;
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

/// A game of `G` ready to start: its deal, its rules, the configuration
/// the log records, and the handle each model in play is asked through.
pub struct Ready<G: Game> {
    deal: HashMap<PlayerId, Dealt<G::Role>>,
    rules: G::Rules,
    configuration: Configuration<G>,
    handles: Handles,
}

impl<G: Game> Ready<G> {
    /// A game under `settings`, made ready. Every prompt is rendered, and
    /// every model in play set up with its key looked up in `keys`, before
    /// the seats are dealt, so that a template that is broken or a model
    /// that does not play fails before any actor is built.
    pub async fn new(settings: Settings<G>, keys: &dyn Keys) -> anyhow::Result<Self> {
        let prompts = settings.prompts()?;
        let (models, handles) = settings.set_up(&prompts, keys).await?;
        let deal = settings.lineup.deal();
        let configuration = settings.configuration(&deal, &prompts, models)?;
        Ok(Ready {
            deal,
            rules: settings.rules(),
            configuration,
            handles,
        })
    }

    /// The episode that plays this game, logging to `logger`.
    pub fn episode(self, logger: Logger<Entry<G>>) -> Episode<Actor<G>> {
        let players = config::players(&self.configuration, &self.deal, &self.handles);
        let (winner, _) = oneshot::channel();
        game::game(
            self.deal,
            self.rules,
            self.configuration,
            winner,
            players,
            logger,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::spec::Environment;
    use crate::{avalon, werewolf};
    use free_agent::Event;
    use secrecy::SecretString;
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

    /// Every entry `ready`'s episode logs as it plays to the end.
    async fn played<G: Game>(ready: Ready<G>) -> Vec<Entry<G>> {
        let (logger, mut log) = unbounded_channel();
        ready
            .episode(logger)
            .run(Duration::from_secs(60 * 60))
            .await
            .unwrap();
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
}
