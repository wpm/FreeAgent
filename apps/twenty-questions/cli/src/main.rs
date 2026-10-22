//! Play Twenty Questions: name a configuration file, play a game, log it
//! to standard error as JSON Lines, and tell it on standard output as it
//! happens.

use clap::Parser;
use free_agent::{Behavior, Episode, Event, Logger};
use free_agent_game::log::{Ended, Secrets};
use free_agent_game::report::Line;
use free_agent_model::spec::Environment;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use twenty_questions::config;
use twenty_questions::log::{Entry, Side, Writer};
use twenty_questions::narrator::{Narrator, shown};
use twenty_questions::run::Ready;

/// The command line.
#[derive(Parser, Debug, PartialEq)]
#[command(version, about)]
enum CommandLine {
    /// Play a game
    Play {
        /// The TOML configuration file
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let CommandLine::Play { config } = CommandLine::parse();
    let ready = Ready::new(config::load(&config)?, &Environment).await?;
    let mut narrator = Narrator::default();
    play(
        io::stderr(),
        ready.secrets(),
        ready.patience(),
        move |event| narrator.narrate(event),
        |logger| ready.episode(logger),
    )
    .await
}

/// Play one game, the episode `game` builds once given the logger every
/// actor holds, with where its winning side comes, waiting at most
/// `patience` for it. The narration `narrate` makes of each event goes to
/// standard output as the game happens, and the episode is written to `log`
/// as [`Writer`] writes it, without `secrets`, ending with how its run came
/// back.
async fn play<A, W>(
    log: W,
    secrets: &Secrets,
    patience: Duration,
    mut narrate: impl FnMut(&Event<Entry>) -> Vec<Line>,
    game: impl FnOnce(Logger<Entry>) -> (Episode<A>, oneshot::Receiver<Side>),
) -> anyhow::Result<()>
where
    A: Behavior<Log = Entry> + 'static,
    W: Write,
{
    let (logger, mut events) = unbounded_channel();
    let (episode, mut won) = game(logger);
    let mut writer = Writer::start(log, episode.id(), secrets.clone())?;
    let telling = async {
        let mut stdout = io::stdout();
        while let Some(event) = events.recv().await {
            writer.write(&event)?;
            for line in narrate(&event) {
                writeln!(stdout, "{}", shown(&line))?;
            }
        }
        stdout.flush()
    };
    // Everything is told once the last actor has let go of its logger.
    let (ran, told) = tokio::join!(episode.run(patience), telling);
    told?;
    writer.end(Ended::of(&ran, won.try_recv().is_ok()))?;
    ran
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use clap::CommandFactory;
    use clap::error::ErrorKind;
    use free_agent::{ActorInit, Context};
    use std::collections::{HashMap, HashSet};
    use twenty_questions::log::Header;
    use twenty_questions::message::Message;

    #[test]
    fn the_command_line_plays_a_configuration_file() {
        CommandLine::command().debug_assert();
        assert_eq!(
            CommandLine::parse_from(["twenty-questions", "play", "game.toml"]),
            CommandLine::Play {
                config: PathBuf::from("game.toml")
            }
        );
        let refused = |args: &[&str]| {
            CommandLine::try_parse_from([&["twenty-questions"], args].concat())
                .unwrap_err()
                .kind()
        };
        assert_eq!(refused(&["play"]), ErrorKind::MissingRequiredArgument);
        assert_eq!(
            refused(&["play", "a.toml", "b.toml"]),
            ErrorKind::UnknownArgument
        );
    }

    /// A player who fails as it starts.
    struct Failing(Context<Message, Entry>);

    #[async_trait]
    impl Behavior for Failing {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.0
        }

        async fn start(&mut self) -> anyhow::Result<()> {
            anyhow::bail!("no model")
        }
    }

    /// An episode of one player who fails as it starts, stopped before it
    /// runs when `stopped`, and which nobody wins.
    fn failing(
        stopped: bool,
    ) -> impl FnOnce(Logger<Entry>) -> (Episode<Failing>, oneshot::Receiver<Side>) {
        move |logger| {
            let init = ActorInit {
                behavior: Box::new(Failing),
                think: None,
                can_send_to: HashSet::new(),
                can_shut_down: HashSet::new(),
                has_logger: true,
            };
            let episode = Episode::new(HashMap::from([("ann".to_string(), init)]), logger);
            if stopped {
                episode.stopper().stop();
            }
            (episode, oneshot::channel().1)
        }
    }

    /// Every entry `written` holds, in order.
    fn entries(written: &[u8]) -> Vec<Entry> {
        std::str::from_utf8(written)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Event<Entry>>(line).unwrap().payload)
            .collect()
    }

    #[tokio::test]
    async fn an_episode_that_fails_is_logged_as_failed_with_its_error() {
        let mut written = Vec::new();
        let none = Secrets::default();
        let patience = Duration::from_secs(60);
        let error = play(&mut written, &none, patience, |_| vec![], failing(false))
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert_eq!(shown, "actor ann failed: no model");
        assert_eq!(
            entries(&written),
            [
                Entry::Header(Header::new()),
                Entry::Ended(Ended::Failed(shown))
            ]
        );
    }

    #[tokio::test]
    async fn an_episode_that_is_stopped_is_logged_as_stopped() {
        let mut written = Vec::new();
        let none = Secrets::default();
        let patience = Duration::from_secs(60);
        play(&mut written, &none, patience, |_| vec![], failing(true))
            .await
            .unwrap();
        assert_eq!(
            entries(&written).last(),
            Some(&Entry::Ended(Ended::Stopped))
        );
    }
}
