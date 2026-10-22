//! Play Twenty Questions: name a configuration file, play one game or as
//! many episodes as asked, log them to standard error as JSON Lines, and
//! tell them on standard output as they happen. Or list the models a
//! provider serves, or list or write out the example configurations.

use clap::{Args, Parser};
use free_agent::{Behavior, Episode, Event, Logger};
use free_agent_game::examples;
use free_agent_game::log::{Ended, Secrets};
use free_agent_game::report::Line;
use free_agent_model::check::models;
use free_agent_model::known_providers_help;
use free_agent_model::spec::Environment;
use std::io::{self, ErrorKind::NotFound, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use twenty_questions::config;
use twenty_questions::examples::EXAMPLES;
use twenty_questions::log::{Entry, Side, Writer};
use twenty_questions::model::plays;
use twenty_questions::narrator::{Narrator, shown};
use twenty_questions::run::Ready;

/// The command line: a game to play, the models a provider serves, or the
/// example configurations.
#[derive(Parser, Debug, PartialEq)]
#[command(version, about)]
enum CommandLine {
    /// Play a game
    Play(Play),
    /// List the models a provider serves, and which of them play when
    /// asked.
    #[command(after_help = known_providers_help())]
    Models {
        /// The root of the provider's OpenAI-compatible API, ending in /v1
        base_url: String,
        /// The environment variable holding the API key, over the one known
        /// for the provider
        #[arg(long)]
        api_key_env: Option<String>,
        /// Ask every model listed, or only the IDs given, to play Twenty
        /// Questions as a player and as the host, and mark those that do.
        /// End the IDs with -- when the base URL follows them
        #[arg(long, num_args = 0.., value_name = "ID", value_terminator = "--")]
        check: Option<Vec<String>>,
    },
    /// List the example configurations, or write them out to play
    Configs {
        /// Write the examples into this directory
        #[arg(long, value_name = "DIR")]
        write: Option<PathBuf>,
        /// Overwrite example files already in the directory
        #[arg(long, requires = "write")]
        force: bool,
    },
}

/// A game to play: its configuration file, how many episodes of it, and
/// whether it is logged.
#[derive(Args, Debug, PartialEq)]
struct Play {
    /// The TOML configuration file
    config: PathBuf,
    /// Play this many episodes, one after another, each dealing the
    /// strategies to the seats afresh and, when the host picks the object,
    /// picking a new one
    #[arg(long, value_name = "N", default_value_t = NonZeroUsize::MIN)]
    episodes: NonZeroUsize,
    #[command(flatten)]
    logging: Logging,
}

/// Whether the game is logged. It is logged unless `--no-log` says
/// otherwise, and when both flags are given the last one wins, as `clap`
/// has paired flags behave.
#[derive(Args, Debug, Default, PartialEq, Eq)]
struct Logging {
    /// Log the game to standard error as JSON Lines [default]
    #[arg(long, overrides_with = "no_log")]
    log: bool,
    /// Do not log the game
    #[arg(long)]
    no_log: bool,
}

impl Logging {
    /// Whether the game is logged.
    fn enabled(&self) -> bool {
        !self.no_log
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match CommandLine::parse() {
        CommandLine::Play(Play {
            config,
            episodes,
            logging,
        }) => {
            let ready = Ready::new(loaded(&config)?, &Environment).await?;
            run(&ready, episodes, logging.enabled()).await
        }
        CommandLine::Models {
            base_url,
            api_key_env,
            check,
        } => {
            let listing = models(
                &base_url,
                api_key_env.as_deref(),
                check.as_deref(),
                &Environment,
                &mut io::stderr(),
                plays,
            )
            .await?;
            print!("{listing}");
            Ok(())
        }
        CommandLine::Configs { write: None, .. } => {
            print!("{}", examples::listing(EXAMPLES));
            Ok(())
        }
        CommandLine::Configs {
            write: Some(dir),
            force,
        } => examples::write(EXAMPLES, &dir, force),
    }
}

/// The configuration file at `path`, loaded. When there is no file there,
/// the error says how to write out the examples.
fn loaded(path: &Path) -> anyhow::Result<config::Settings> {
    config::load(path).map_err(|error| {
        let missing = error
            .chain()
            .any(|cause| cause.downcast_ref::<io::Error>().map(io::Error::kind) == Some(NotFound));
        if missing {
            error.context(format!(
                "no configuration file {}; `twenty-questions configs --write .` writes the \
                 example configurations into the current directory",
                path.display()
            ))
        } else {
            error
        }
    })
}

/// Play `episodes` episodes of the game that is `ready`, one after
/// another, each dealt afresh, logged to standard error when `logging` is
/// on. Each episode's account is set off from the one before it by a blank
/// line.
async fn run(ready: &Ready, episodes: NonZeroUsize, logging: bool) -> anyhow::Result<()> {
    for episode in 0..episodes.get() {
        if episode > 0 {
            writeln!(io::stdout())?;
        }
        let mut narrator = Narrator::default();
        play(
            logging.then(io::stderr),
            ready.secrets(),
            ready.patience(),
            move |event| narrator.narrate(event),
            |logger| ready.episode(logger),
        )
        .await?;
    }
    Ok(())
}

/// Play one game, the episode `game` builds once given the logger every
/// actor holds, with where its winning side comes, waiting at most
/// `patience` for it. The narration `narrate` makes of each event goes to
/// standard output as the game happens, and when there is a `log`, the
/// episode is written to it as [`Writer`] writes it, without `secrets`,
/// ending with how its run came back.
async fn play<A, W>(
    log: Option<W>,
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
    let mut writer = log
        .map(|sink| Writer::start(sink, episode.id(), secrets.clone()))
        .transpose()?;
    let telling = async {
        let mut stdout = io::stdout();
        while let Some(event) = events.recv().await {
            if let Some(writer) = &mut writer {
                writer.write(&event)?;
            }
            for line in narrate(&event) {
                writeln!(stdout, "{}", shown(&line))?;
            }
        }
        stdout.flush()
    };
    // Everything is told once the last actor has let go of its logger.
    let (ran, told) = tokio::join!(episode.run(patience), telling);
    told?;
    if let Some(writer) = writer {
        writer.end(Ended::of(&ran, won.try_recv().is_ok()))?;
    }
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

    /// The command line `args` parse to, after the binary's name.
    fn parsed(args: &[&str]) -> CommandLine {
        CommandLine::parse_from([&["twenty-questions"], args].concat())
    }

    /// The command line `args` parse to, after the binary's name and
    /// `play`.
    fn play_args(args: &[&str]) -> CommandLine {
        parsed(&[&["play"], args].concat())
    }

    /// The command line of `episodes` games played from `game.toml` with
    /// `logging`.
    fn playing(episodes: usize, logging: Logging) -> CommandLine {
        CommandLine::Play(Play {
            config: PathBuf::from("game.toml"),
            episodes: NonZeroUsize::new(episodes).unwrap(),
            logging,
        })
    }

    /// The kind of error `clap` gives the command line `args`, after the
    /// binary's name, or `None` when it parses.
    fn refused(args: &[&str]) -> Option<ErrorKind> {
        CommandLine::try_parse_from([&["twenty-questions"], args].concat())
            .err()
            .map(|error| error.kind())
    }

    #[test]
    fn the_command_line_plays_a_configuration_file() {
        CommandLine::command().debug_assert();
        assert_eq!(play_args(&["game.toml"]), playing(1, Logging::default()));
        assert_eq!(refused(&["play"]), Some(ErrorKind::MissingRequiredArgument));
        assert_eq!(
            refused(&["play", "a.toml", "b.toml"]),
            Some(ErrorKind::UnknownArgument)
        );
    }

    #[test]
    fn the_commands_are_play_models_and_configs() {
        let commands: Vec<String> = CommandLine::command()
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .filter(|name| name != "help")
            .collect();
        assert_eq!(commands, ["play", "models", "configs"]);
    }

    #[test]
    fn play_takes_the_log_flags_and_the_last_of_the_two_wins() {
        let off = || Logging {
            log: false,
            no_log: true,
        };
        let on = || Logging {
            log: true,
            no_log: false,
        };
        assert_eq!(play_args(&["game.toml", "--no-log"]), playing(1, off()));
        assert_eq!(
            play_args(&["game.toml", "--no-log", "--log"]),
            playing(1, on())
        );
        assert_eq!(
            play_args(&["game.toml", "--log", "--no-log"]),
            playing(1, off())
        );
    }

    #[test]
    fn play_plays_one_episode_unless_episodes_says_how_many() {
        assert_eq!(
            play_args(&["game.toml", "--episodes", "3"]),
            playing(3, Logging::default())
        );
        assert_eq!(
            refused(&["play", "game.toml", "--episodes", "0"]),
            Some(ErrorKind::ValueValidation)
        );
    }

    #[test]
    fn the_game_is_logged_unless_no_log_says_otherwise() {
        assert!(Logging::default().enabled());
        assert!(
            Logging {
                log: true,
                no_log: false
            }
            .enabled()
        );
        assert!(
            !Logging {
                log: false,
                no_log: true
            }
            .enabled()
        );
    }

    #[test]
    fn the_models_command_takes_the_base_url_a_key_variable_and_the_ids_to_check() {
        let models = |api_key_env: Option<&str>, check: Option<&[&str]>| CommandLine::Models {
            base_url: "http://localhost:1234/v1".to_string(),
            api_key_env: api_key_env.map(String::from),
            check: check.map(|ids| ids.iter().map(|id| id.to_string()).collect()),
        };
        let url = "http://localhost:1234/v1";
        assert_eq!(parsed(&["models", url]), models(None, None));
        assert_eq!(
            parsed(&["models", url, "--api-key-env", "OPENAI_API_KEY"]),
            models(Some("OPENAI_API_KEY"), None)
        );
        assert_eq!(parsed(&["models", url, "--check"]), models(None, Some(&[])));
        assert_eq!(
            parsed(&["models", url, "--check", "a", "b"]),
            models(None, Some(&["a", "b"]))
        );
        // The ids end with -- when the base URL follows them; without it,
        // the URL would be read as an id.
        assert_eq!(
            parsed(&["models", "--check", "a", "--", url]),
            models(None, Some(&["a"]))
        );
        assert_eq!(
            refused(&["models", "--check", url]),
            Some(ErrorKind::MissingRequiredArgument)
        );
        assert_eq!(
            refused(&["models"]),
            Some(ErrorKind::MissingRequiredArgument)
        );
    }

    #[test]
    fn the_models_help_ends_with_the_providers_known() {
        let help = CommandLine::command()
            .find_subcommand_mut("models")
            .unwrap()
            .render_help()
            .to_string();
        assert!(help.ends_with(&known_providers_help()), "{help}");
    }

    #[test]
    fn the_configs_command_lists_unless_told_where_to_write_and_forces_only_a_write() {
        assert_eq!(
            parsed(&["configs"]),
            CommandLine::Configs {
                write: None,
                force: false,
            }
        );
        assert_eq!(
            parsed(&["configs", "--write", ".", "--force"]),
            CommandLine::Configs {
                write: Some(PathBuf::from(".")),
                force: true,
            }
        );
        assert_eq!(
            refused(&["configs", "--force"]),
            Some(ErrorKind::MissingRequiredArgument)
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
        let log = Some(&mut written);
        let error = play(log, &none, patience, |_| vec![], failing(false))
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
        let log = Some(&mut written);
        play(log, &none, patience, |_| vec![], failing(true))
            .await
            .unwrap();
        assert_eq!(
            entries(&written).last(),
            Some(&Entry::Ended(Ended::Stopped))
        );
    }

    #[tokio::test]
    async fn an_episode_with_no_log_is_written_nowhere() {
        let none = Secrets::default();
        let patience = Duration::from_secs(60);
        let error = play::<_, Vec<u8>>(None, &none, patience, |_| vec![], failing(false))
            .await
            .unwrap_err();
        assert_eq!(format!("{error:#}"), "actor ann failed: no model");
    }
}
