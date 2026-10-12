//! Play a social deduction game: name the game and its configuration file,
//! deal the roles and strategies, run one game, log it to standard error as
//! JSON lines, and tell it on standard output as it happens. Or list the
//! models a provider serves, which belongs to no game.

use clap::{Args, Parser};
use free_agent::{Behavior, Episode, Event, Logger};
use serde::Serialize;
use social_deduction::avalon;
use social_deduction::model::spec::Probed;
use social_deduction::model::{
    Model, Provider, REQUEST_TIMEOUT, ToolChoice, api_key, key_variable, known_providers, served,
};
use social_deduction::werewolf::config::{self, Config, Overrides};
use social_deduction::werewolf::prompted;
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::{self, Team};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// The command line: a game, or a command that belongs to no game.
#[derive(Parser, Debug, PartialEq)]
#[command(version, about)]
enum CommandLine {
    /// Werewolf: werewolves against villagers, by night and by day.
    Werewolf(Werewolf),
    /// Avalon: good against evil, over five quests.
    Avalon(Avalon),
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

/// A game of Werewolf: its configuration file, if any, and what the
/// command line says over it.
#[derive(Args, Debug, PartialEq)]
struct Werewolf {
    /// The TOML configuration file; without one, every player is random
    config: Option<PathBuf>,
    #[command(flatten)]
    overrides: Overrides,
    #[command(flatten)]
    logging: Logging,
}

/// A game of Avalon: what the command line says of the table and the
/// limits, every player random.
#[derive(Args, Debug, PartialEq)]
struct Avalon {
    #[command(flatten)]
    roles: avalon::RoleCounts,
    #[command(flatten)]
    limits: avalon::PhaseLimits,
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
        CommandLine::Werewolf(Werewolf {
            config,
            overrides,
            logging,
        }) => {
            let config = match config {
                Some(path) => Config::load(&path)?,
                None => Config::default(),
            };
            let settings = config.settle(overrides)?;
            // Every prompt is rendered, and every model in play set up,
            // before any actor is built, so that a template that is broken
            // or a model that does not play fails before anything else.
            let prompts = settings.prompts()?;
            let (models, handles) = settings.set_up(&prompts).await?;
            let deal = settings.deal();
            let configuration = settings.configuration(&deal, &prompts, models)?;
            let players = config::players(&configuration, &deal, &handles);
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
        CommandLine::Avalon(Avalon {
            roles,
            limits,
            logging,
        }) => {
            let deal = roles.table()?.deal();
            let rules = limits.rules();
            let mut narrator = avalon::report::Narrator::default();
            play(
                logging.enabled(),
                move |event| narrator.narrate(event),
                |logger| {
                    let (winner, _) = oneshot::channel();
                    avalon::game(
                        deal,
                        rules,
                        winner,
                        |_| avalon::random::Player::init(),
                        logger,
                    )
                },
            )
            .await
        }
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
    use social_deduction::model::canned::{self, refusal};
    use social_deduction::model::fake::{FakeModel, choosing_first};
    use social_deduction::werewolf::{PhaseLimits, RoleCounts, Rules, Table};
    use std::collections::HashSet;

    /// The command line `args` parse to, after the binary's name.
    fn parsed(args: &[&str]) -> CommandLine {
        CommandLine::parse_from([&["social-deduction"], args].concat())
    }

    /// The command line `args` parse to, after the binary's and the
    /// game's names.
    fn werewolf(args: &[&str]) -> CommandLine {
        parsed(&[&["werewolf"], args].concat())
    }

    /// The command line of a game of Werewolf with `logging` and nothing
    /// else.
    fn logged(logging: Logging) -> CommandLine {
        CommandLine::Werewolf(Werewolf {
            config: None,
            overrides: Overrides::default(),
            logging,
        })
    }

    #[test]
    fn the_command_line_is_a_game_that_tells_unset_from_given() {
        CommandLine::command().debug_assert();
        assert_eq!(
            parsed(&["werewolf", "--werewolves", "1"]),
            CommandLine::Werewolf(Werewolf {
                config: None,
                overrides: Overrides {
                    roles: RoleCounts {
                        werewolves: Some(1),
                        ..RoleCounts::default()
                    },
                    ..Overrides::default()
                },
                logging: Logging::default(),
            })
        );
    }

    #[test]
    fn werewolf_takes_the_log_flags_and_the_last_of_the_two_wins() {
        let off = || Logging {
            log: false,
            no_log: true,
        };
        let on = || Logging {
            log: true,
            no_log: false,
        };
        assert_eq!(werewolf(&["--no-log"]), logged(off()));
        assert_eq!(werewolf(&["--no-log", "--log"]), logged(on()));
        assert_eq!(
            werewolf(&["game.toml", "--log", "--no-log"]),
            CommandLine::Werewolf(Werewolf {
                config: Some(PathBuf::from("game.toml")),
                overrides: Overrides::default(),
                logging: off(),
            })
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
    fn werewolf_takes_a_file_the_role_counts_and_the_limits() {
        let args = [
            "game.toml",
            "--seers",
            "2",
            "--night-limit",
            "30s",
            "--day-limit",
            "1m 30s",
            "--ratify-limit",
            "15s",
        ];
        assert_eq!(
            werewolf(&args),
            CommandLine::Werewolf(Werewolf {
                config: Some(PathBuf::from("game.toml")),
                overrides: Overrides {
                    roles: RoleCounts {
                        seers: Some(2),
                        ..RoleCounts::default()
                    },
                    limits: PhaseLimits {
                        night_limit: Some(Duration::from_secs(30)),
                        day_limit: Some(Duration::from_secs(90)),
                    },
                    ratify_limit: Some(Duration::from_secs(15)),
                },
                logging: Logging::default(),
            })
        );
    }

    #[test]
    fn werewolf_takes_durations_in_humantime_form_and_one_file_at_most() {
        let refused = |args: &[&str]| {
            CommandLine::try_parse_from([&["social-deduction", "werewolf"], args].concat()).is_err()
        };
        assert!(refused(&["--night-limit", "soon"]));
        assert!(refused(&["--day-limit", "30"]));
        assert!(refused(&["--ratify-limit", "soon"]));
        assert!(refused(&["llm", "game.toml"]));
    }

    #[test]
    fn werewolf_takes_no_model_options() {
        for option in ["--model-base-url", "--model-id", "--context-window"] {
            assert!(
                CommandLine::try_parse_from(["social-deduction", "werewolf", option, "x"]).is_err(),
                "{option}"
            );
        }
    }

    #[test]
    fn the_help_names_the_defaults_the_code_fills_in() {
        let table = Table::default();
        let counts = [
            ("--werewolves", table.werewolves.to_string()),
            ("--villagers", table.villagers.to_string()),
            ("--doctors", table.doctors.to_string()),
            ("--seers", table.seers.to_string()),
        ];
        let rules = Rules::default();
        let limits = [
            ("--night-limit", rules.night_limit),
            ("--day-limit", rules.day_limit),
        ]
        .map(|(option, limit)| (option, humantime::format_duration(limit).to_string()));
        let help = CommandLine::command()
            .find_subcommand_mut("werewolf")
            .unwrap()
            .render_help()
            .to_string();
        for (option, default) in counts.into_iter().chain(limits) {
            let line = help.lines().find(|line| line.contains(option)).unwrap();
            assert!(line.ends_with(&format!("[default: {default}]")), "{line}");
        }
        // The ratify limit's default, zero, is told in words.
        let line = help
            .lines()
            .find(|line| line.contains("--ratify-limit"))
            .unwrap();
        assert!(
            line.ends_with("zero, the default, means a day is not ratified"),
            "{line}"
        );
        assert!(help.contains("[CONFIG]"), "{help}");
    }

    #[test]
    fn avalon_takes_the_counts_the_limits_and_the_log_flags_and_no_file() {
        assert_eq!(
            parsed(&["avalon"]),
            CommandLine::Avalon(Avalon {
                roles: avalon::RoleCounts::default(),
                limits: avalon::PhaseLimits::default(),
                logging: Logging::default(),
            })
        );
        let args = [
            "avalon",
            "--merlin",
            "1",
            "--percival",
            "1",
            "--servants",
            "3",
            "--assassin",
            "1",
            "--morgana",
            "1",
            "--mordred",
            "1",
            "--oberon",
            "1",
            "--minions",
            "2",
            "--propose-limit",
            "30s",
            "--vote-limit",
            "1m 30s",
            "--quest-limit",
            "2m",
            "--assassinate-limit",
            "5s",
            "--no-log",
        ];
        assert_eq!(
            parsed(&args),
            CommandLine::Avalon(Avalon {
                roles: avalon::RoleCounts {
                    merlin: Some(1),
                    percival: Some(1),
                    servants: Some(3),
                    assassin: Some(1),
                    morgana: Some(1),
                    mordred: Some(1),
                    oberon: Some(1),
                    minions: Some(2),
                },
                limits: avalon::PhaseLimits {
                    propose_limit: Some(Duration::from_secs(30)),
                    vote_limit: Some(Duration::from_secs(90)),
                    quest_limit: Some(Duration::from_secs(120)),
                    assassinate_limit: Some(Duration::from_secs(5)),
                },
                logging: Logging {
                    log: false,
                    no_log: true
                },
            })
        );
        let refused = |args: &[&str]| {
            CommandLine::try_parse_from([&["social-deduction", "avalon"], args].concat()).is_err()
        };
        assert!(refused(&["game.toml"]));
        assert!(refused(&["--vote-limit", "30"]));
        assert!(refused(&["--werewolves", "1"]));
    }

    #[test]
    fn the_avalon_help_names_the_defaults_the_code_fills_in() {
        let table = avalon::Table::default();
        let counts = table
            .counts()
            .map(|(role, count)| (avalon::RoleCounts::flag(role), count.to_string()));
        let rules = avalon::Rules::default();
        let limits = [
            ("--propose-limit", rules.propose_limit),
            ("--vote-limit", rules.vote_limit),
            ("--quest-limit", rules.quest_limit),
            ("--assassinate-limit", rules.assassinate_limit),
        ]
        .map(|(option, limit)| {
            (
                option.to_string(),
                humantime::format_duration(limit).to_string(),
            )
        });
        let mut command = CommandLine::command();
        let avalon = command.find_subcommand_mut("avalon").unwrap();
        assert_eq!(
            avalon.get_about().unwrap().to_string(),
            "Avalon: good against evil, over five quests"
        );
        let help = avalon.render_help().to_string();
        for (option, default) in counts.into_iter().chain(limits) {
            let line = help.lines().find(|line| line.contains(&option)).unwrap();
            assert!(line.ends_with(&format!("[default: {default}]")), "{line}");
        }
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
            "werewolf",
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
