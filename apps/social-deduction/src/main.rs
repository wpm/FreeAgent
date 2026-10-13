//! Play a social deduction game: name its configuration file, which names
//! the game, deal the roles and strategies, run one game, log it to standard error as
//! JSON lines, and tell it on standard output as it happens. Or list the
//! models a provider serves, which belongs to no game.

use clap::{Args, Parser};
use free_agent::{Behavior, Episode, Event, Logger};
use serde::Serialize;
use social_deduction::avalon;
use social_deduction::config::{self, Game};
use social_deduction::model::spec::Probed;
use social_deduction::model::{
    Model, Provider, REQUEST_TIMEOUT, ToolChoice, api_key, key_variable, known_providers, served,
};
use social_deduction::werewolf::prompted;
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::{self, Team};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// The command line: a game to play, or a command that belongs to no game.
#[derive(Parser, Debug, PartialEq)]
#[command(version, about)]
enum CommandLine {
    /// Play a game: the configuration file names which
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
        /// Ask every model listed, or only the IDs given, to play as a
        /// player does by night and by day, and mark those that do. End the
        /// IDs with -- when the base URL follows them
        #[arg(long, num_args = 0.., value_name = "ID", value_terminator = "--")]
        check: Option<Vec<String>>,
    },
}

/// A game to play: its configuration file, and whether it is logged.
#[derive(Args, Debug, PartialEq)]
struct Play {
    /// The TOML configuration file, whose `game` key names the game
    config: PathBuf,
    #[command(flatten)]
    logging: Logging,
}

/// Whether the game is logged, for a game to flatten into its command
/// line arguments. It is logged unless `--no-log` says otherwise, and when
/// both flags are given the last one wins, as `clap` has paired flags
/// behave.
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

/// How long a game may take before it is abandoned.
const PATIENCE: Duration = Duration::from_secs(60 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match CommandLine::parse() {
        CommandLine::Play(Play { config, logging }) => match config::load(&config)? {
            Game::Werewolf(settings) => {
                // Every prompt is rendered, and every model in play set
                // up, before any actor is built, so that a template that is
                // broken or a model that does not play fails before
                // anything else.
                let prompts = settings.prompts()?;
                let (models, handles) = settings.set_up(&prompts).await?;
                let deal = settings.lineup.deal();
                let configuration = settings.configuration(&deal, &prompts, models)?;
                let players = werewolf::config::players(&configuration, &deal, &handles);
                let rules = settings.rules();
                let mut narrator = Narrator::default();
                play(
                    logging.enabled(),
                    move |event| narrator.narrate(event),
                    |logger| {
                        let (winner, _) = oneshot::channel::<Team>();
                        werewolf::game(deal, rules, configuration, winner, players, logger)
                    },
                )
                .await
            }
            Game::Avalon(settings) => {
                // As in Werewolf, every prompt is rendered and every model
                // in play set up before any actor is built.
                let prompts = settings.prompts()?;
                let (models, handles) = settings.set_up(&prompts).await?;
                let deal = settings.lineup.deal();
                let configuration = settings.configuration(&deal, &prompts, models)?;
                let players = avalon::config::players(&configuration, &deal, &handles);
                let rules = settings.rules;
                let mut narrator = avalon::report::Narrator::default();
                play(
                    logging.enabled(),
                    move |event| narrator.narrate(event),
                    |logger| {
                        let (winner, _) = oneshot::channel();
                        avalon::game(deal, rules, configuration, winner, players, logger)
                    },
                )
                .await
            }
        },
        CommandLine::Models {
            base_url,
            api_key_env,
            check,
        } => {
            let variable = key_variable(&base_url, api_key_env.as_deref());
            let api_key = variable.map(api_key).transpose()?;
            let provider = Provider::new(&base_url, api_key, REQUEST_TIMEOUT)?;
            let listed = provider.listing().await?;
            let mut ids: Vec<String> = listed.iter().map(|model| model.id.clone()).collect();
            ids.sort();
            let models = match check {
                None => ids.into_iter().map(|id| (id, None)).collect(),
                Some(check) => {
                    for id in &check {
                        served(&base_url, &listed, id)?;
                    }
                    checked(&provider, ids, &check, &mut io::stderr()).await?
                }
            };
            print!("{}", listing(&models));
            Ok(())
        }
    }
}

/// What `models --help` ends with: a table of the providers known, the
/// root of each one's API and the environment variable its key is read
/// from unless --api-key-env names another.
fn known_providers_help() -> String {
    let providers = known_providers();
    let width = providers
        .iter()
        .map(|(root, _)| root.len())
        .max()
        .unwrap_or_default();
    let mut help = String::from(
        "Providers known, with the environment variable their API key is read from:\n",
    );
    for (root, variable) in providers {
        writeln!(help, "  {root:width$}  {}", variable.unwrap_or("-")).unwrap();
    }
    help
}

/// How a model is marked in the `models --check` listing: by whether it
/// played when asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    /// Asked as by night and as by day, it answered both requests well,
    /// under the tool choice the probe settled.
    Played(ToolChoice),
    /// Asked as by night and as by day, it did not answer one of them well.
    DidNotPlay,
}

/// Each of `ids`, in order, with its mark: those in `check`, or every one
/// of them when `check` is empty, are asked by `model` to play, one model
/// at a time, since a local server loads each model it is asked for, and
/// the rest are unmarked. Why a model that did not play failed goes to
/// `reasons`, as [`reason`] writes it. A model that could not be asked
/// stops the listing with the error, since nothing it would say about
/// the rest would be true.
async fn checked(
    model: &dyn Model,
    ids: Vec<String>,
    check: &[String],
    reasons: &mut impl Write,
) -> anyhow::Result<Vec<(String, Option<Mark>)>> {
    let mut marked = Vec::new();
    for id in ids {
        let mark = if check.is_empty() || check.contains(&id) {
            match prompted::check_plays(model, &id).await? {
                Probed::Played(tool_choice) => Some(Mark::Played(tool_choice)),
                Probed::DidNotPlay(error) => {
                    reason(reasons, &id, &error)?;
                    Some(Mark::DidNotPlay)
                }
            }
        } else {
            None
        };
        marked.push((id, mark));
    }
    Ok(marked)
}

/// Write why the model `id` did not play: a line `! id` naming it as
/// the listing marks it, then `error`'s causes, each on its own indented
/// line, so a provider's words may run to several lines and still be read
/// as one model's.
fn reason(out: &mut impl Write, id: &str, error: &anyhow::Error) -> io::Result<()> {
    writeln!(out, "! {id}")?;
    for cause in error.chain() {
        for line in cause.to_string().lines() {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

/// The listing `models` prints: one id per line, in order. When any model
/// is marked, each id comes after its mark, `* `, `~ ` or `! `, or two spaces
/// when it has none, so the ids line up, and the listing ends, set off by
/// an empty line, with a line saying what the marks mean.
fn listing(models: &[(String, Option<Mark>)]) -> String {
    let any_marked = models.iter().any(|(_, mark)| mark.is_some());
    let mut listing = String::new();
    for (id, mark) in models {
        let shown = match mark {
            Some(Mark::Played(ToolChoice::Required)) => "* ",
            Some(Mark::Played(ToolChoice::Auto)) => "~ ",
            Some(Mark::DidNotPlay) => "! ",
            None if any_marked => "  ",
            None => "",
        };
        writeln!(listing, "{shown}{id}").unwrap();
    }
    if any_marked {
        writeln!(
            listing,
            "\n* marks a model that played when asked, ~ one that played when asked to call a \
             tool but not made to, and ! one that did not."
        )
        .unwrap();
    }
    listing
}

/// Play one game, the episode `game` builds once given the logger the
/// environment holds. The narration `narrate` makes of each event goes to
/// standard output as the game happens, and so does the log to standard
/// error as JSON lines when `logging` is on.
async fn play<A, L>(
    logging: bool,
    mut narrate: impl FnMut(&Event<L>) -> Vec<String> + Send + 'static,
    game: impl FnOnce(Logger<L>) -> Episode<A>,
) -> anyhow::Result<()>
where
    A: Behavior<Log = L> + 'static,
    L: Serialize + Send + 'static,
{
    let (logger, mut log) = unbounded_channel();
    let episode = game(logger);
    let telling = tokio::spawn(async move {
        let mut stdout = io::stdout();
        while let Some(event) = log.recv().await {
            if logging {
                event.write(io::stderr())?;
            }
            for line in narrate(&event) {
                writeln!(stdout, "{line}")?;
            }
        }
        stdout.flush()
    });
    episode.run(PATIENCE).await?;
    // Everything is told once the last actor has let go of its logger.
    telling.await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;
    use social_deduction::model::canned::{self, refusal};
    use social_deduction::model::fake::{FakeModel, choosing_first};
    use std::collections::HashSet;

    /// The command line `args` parse to, after the binary's name.
    fn parsed(args: &[&str]) -> CommandLine {
        CommandLine::parse_from([&["social-deduction"], args].concat())
    }

    /// The command line `args` parse to, after the binary's name and
    /// `play`.
    fn play(args: &[&str]) -> CommandLine {
        parsed(&[&["play"], args].concat())
    }

    /// The command line of a game played from `game.toml` with `logging`.
    fn logged(logging: Logging) -> CommandLine {
        CommandLine::Play(Play {
            config: PathBuf::from("game.toml"),
            logging,
        })
    }

    /// The kind of error `clap` gives the command line `args`, after the
    /// binary's name, or `None` when it parses.
    fn refused(args: &[&str]) -> Option<ErrorKind> {
        CommandLine::try_parse_from([&["social-deduction"], args].concat())
            .err()
            .map(|error| error.kind())
    }

    #[test]
    fn the_command_line_plays_a_configuration_file() {
        CommandLine::command().debug_assert();
        assert_eq!(play(&["game.toml"]), logged(Logging::default()));
    }

    #[test]
    fn the_games_are_not_subcommands() {
        for game in ["werewolf", "avalon"] {
            assert_eq!(
                refused(&[game, "game.toml"]),
                Some(ErrorKind::InvalidSubcommand),
                "{game}"
            );
        }
    }

    #[test]
    fn play_without_a_configuration_file_is_an_error_naming_it() {
        let error = CommandLine::try_parse_from(["social-deduction", "play"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        assert!(error.to_string().contains("<CONFIG>"), "{error}");
    }

    #[test]
    fn play_takes_one_configuration_file_at_most() {
        assert_eq!(
            refused(&["play", "game.toml", "other.toml"]),
            Some(ErrorKind::UnknownArgument)
        );
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
        assert_eq!(play(&["game.toml", "--no-log"]), logged(off()));
        assert_eq!(play(&["game.toml", "--no-log", "--log"]), logged(on()));
        assert_eq!(play(&["game.toml", "--log", "--no-log"]), logged(off()));
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
    fn the_settings_the_file_holds_are_not_flags() {
        let flags = [
            "--werewolves",
            "--villagers",
            "--doctors",
            "--seers",
            "--night-limit",
            "--day-limit",
            "--ratify-limit",
            "--merlin",
            "--percival",
            "--servants",
            "--assassin",
            "--morgana",
            "--mordred",
            "--oberon",
            "--minions",
            "--propose-limit",
            "--vote-limit",
            "--quest-limit",
            "--assassinate-limit",
            "--game",
            "--model-base-url",
            "--model-id",
            "--context-window",
        ];
        for flag in flags {
            assert_eq!(
                refused(&["play", "game.toml", flag, "1"]),
                Some(ErrorKind::UnknownArgument),
                "{flag}"
            );
        }
    }

    #[test]
    fn the_commands_are_play_and_models_and_play_lists_the_file_and_the_log_flags_alone() {
        let mut command = CommandLine::command();
        let commands: Vec<&str> = command
            .get_subcommands()
            .map(|subcommand| subcommand.get_name())
            .filter(|name| *name != "help")
            .collect();
        assert_eq!(commands, ["play", "models"]);
        let play = command.find_subcommand_mut("play").unwrap();
        assert_eq!(
            play.get_about().unwrap().to_string(),
            "Play a game: the configuration file names which"
        );
        let arguments: Vec<String> = play
            .get_arguments()
            .map(|argument| argument.get_id().to_string())
            .collect();
        assert_eq!(arguments, ["config", "log", "no_log"]);
        let help = play.render_help().to_string();
        assert!(help.contains("<CONFIG>"), "{help}");
        assert!(
            help.contains("The TOML configuration file, whose `game` key names the game"),
            "{help}"
        );
    }

    #[test]
    fn the_models_command_takes_the_base_url_and_a_key_variable_and_belongs_to_no_game() {
        assert_eq!(
            parsed(&["models", "http://localhost:1234/v1"]),
            CommandLine::Models {
                base_url: "http://localhost:1234/v1".to_string(),
                api_key_env: None,
                check: None,
            }
        );
        let args = [
            "models",
            "https://api.openai.com/v1",
            "--api-key-env",
            "OPENAI_API_KEY",
        ];
        assert_eq!(
            parsed(&args),
            CommandLine::Models {
                base_url: "https://api.openai.com/v1".to_string(),
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                check: None,
            }
        );
        let args = [
            "social-deduction",
            "play",
            "models",
            "http://localhost:1234/v1",
        ];
        assert!(CommandLine::try_parse_from(args).is_err());
    }

    #[test]
    fn the_models_command_takes_check_alone_or_with_the_ids_to_check() {
        let models = |check| CommandLine::Models {
            base_url: "http://localhost:1234/v1".to_string(),
            api_key_env: None,
            check,
        };
        assert_eq!(
            parsed(&["models", "http://localhost:1234/v1", "--check"]),
            models(Some(vec![]))
        );
        assert_eq!(
            parsed(&["models", "http://localhost:1234/v1", "--check", "a", "b"]),
            models(Some(vec!["a".to_string(), "b".to_string()]))
        );
        // The ids end with -- when the base URL follows them; without it,
        // the URL would be read as an id.
        assert_eq!(
            parsed(&["models", "--check", "a", "--", "http://localhost:1234/v1"]),
            models(Some(vec!["a".to_string()]))
        );
        assert_eq!(
            parsed(&["models", "--check", "--", "http://localhost:1234/v1"]),
            models(Some(vec![]))
        );
        assert!(
            CommandLine::try_parse_from([
                "social-deduction",
                "models",
                "--check",
                "http://localhost:1234/v1"
            ])
            .is_err()
        );
    }

    #[test]
    fn the_models_help_ends_with_the_providers_known_in_a_table() {
        let help = CommandLine::command()
            .find_subcommand_mut("models")
            .unwrap()
            .render_help()
            .to_string();
        assert!(help.ends_with(&known_providers_help()), "{help}");
        let table = known_providers_help();
        let rows: Vec<&str> = table.lines().skip(1).collect();
        assert!(
            rows.contains(&"  https://api.openai.com/v1     OPENAI_API_KEY"),
            "{table}"
        );
        assert!(
            rows.contains(&"  https://api.anthropic.com/v1  ANTHROPIC_API_KEY"),
            "{table}"
        );
        // The variables line up, whatever the roots' lengths.
        let columns: HashSet<Option<usize>> = rows.iter().map(|row| row.rfind("  ")).collect();
        assert_eq!(columns.len(), 1, "{table}");
    }

    #[test]
    fn the_listing_is_one_id_per_line_and_when_any_is_marked_lines_them_up_and_says_what_the_marks_mean()
     {
        let ids = ["gpt-0", "qwen2.5-7b-instruct"].map(String::from);
        let plain: Vec<(String, Option<Mark>)> = ids.iter().map(|id| (id.clone(), None)).collect();
        assert_eq!(listing(&plain), "gpt-0\nqwen2.5-7b-instruct\n");
        assert_eq!(listing(&[]), "");
        let models = [
            ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
            ("llama".to_string(), None),
            (
                "qwen2.5-7b-instruct".to_string(),
                Some(Mark::Played(ToolChoice::Required)),
            ),
            ("sonnet".to_string(), Some(Mark::Played(ToolChoice::Auto))),
        ];
        assert_eq!(
            listing(&models),
            "! gpt-0\n  llama\n* qwen2.5-7b-instruct\n~ sonnet\n\n\
             * marks a model that played when asked, ~ one that played when asked to call a tool \
             but not made to, and ! one that did not.\n"
        );
    }

    /// A model that plays as `id` and refuses as anything else, over two
    /// lines.
    fn playing_only_as(id: &'static str) -> FakeModel {
        FakeModel::answering(move |request| {
            if request.model == id {
                choosing_first(request)
            } else {
                Err(anyhow::anyhow!("{}\nin two lines", refusal(&request.model)))
            }
        })
    }

    #[tokio::test]
    async fn every_model_listed_is_checked_unless_ids_are_given_and_then_only_those() {
        let ids = ["gpt-0", "llama", "qwen2.5-7b-instruct"].map(String::from);
        let model = playing_only_as("qwen2.5-7b-instruct");
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &[], &mut reasons)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
                ("llama".to_string(), Some(Mark::DidNotPlay)),
                (
                    "qwen2.5-7b-instruct".to_string(),
                    Some(Mark::Played(ToolChoice::Required))
                ),
            ]
        );
        // A model that plays by night is asked again as by day.
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(
            asked,
            [
                "gpt-0",
                "llama",
                "qwen2.5-7b-instruct",
                "qwen2.5-7b-instruct"
            ]
        );
        // Each reason under a line naming its model as the listing marks
        // it, the error's causes one per indented line, a cause of several
        // lines indented on each.
        assert_eq!(
            String::from_utf8(reasons).unwrap(),
            "! gpt-0\n  gpt-0 did not answer a night's request\n  gpt-0 does not support tools\n  in two lines\n\
             ! llama\n  llama did not answer a night's request\n  llama does not support tools\n  in two lines\n"
        );
        // Only the ids given, in the listing's order.
        let model = playing_only_as("qwen2.5-7b-instruct");
        let check = ["qwen2.5-7b-instruct", "gpt-0"].map(String::from);
        let mut reasons = Vec::new();
        let marked = checked(&model, ids.to_vec(), &check, &mut reasons)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Some(Mark::DidNotPlay)),
                ("llama".to_string(), None),
                (
                    "qwen2.5-7b-instruct".to_string(),
                    Some(Mark::Played(ToolChoice::Required))
                ),
            ]
        );
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(
            asked,
            ["gpt-0", "qwen2.5-7b-instruct", "qwen2.5-7b-instruct"]
        );
        assert!(String::from_utf8(reasons).unwrap().starts_with("! gpt-0\n"));
    }

    #[tokio::test]
    async fn a_model_that_plays_only_when_not_made_to_call_a_tool_is_marked_so() {
        let base_url = canned::refusing_required(&["sonnet"]);
        let provider = Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap();
        let mut reasons = Vec::new();
        let marked = checked(&provider, vec!["sonnet".to_string()], &[], &mut reasons)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [("sonnet".to_string(), Some(Mark::Played(ToolChoice::Auto)))]
        );
        assert!(reasons.is_empty());
    }

    #[tokio::test]
    async fn a_model_that_could_not_be_asked_stops_the_listing_with_the_error() {
        // A provider answering anything but the model's answer says nothing
        // about this model or the rest.
        let (base_url, _served) = canned::answering("429 Too Many Requests", "{}");
        let provider = Provider::new(base_url.clone(), None, REQUEST_TIMEOUT).unwrap();
        let ids = ["gpt-0", "llama"].map(String::from);
        let mut reasons = Vec::new();
        let error = checked(&provider, ids.to_vec(), &[], &mut reasons)
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.starts_with("gpt-0 could not be asked to play"),
            "{shown}"
        );
        assert!(shown.contains("429 Too Many Requests"), "{shown}");
        assert!(reasons.is_empty());
    }
}
