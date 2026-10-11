//! Play a social deduction game: name the game and its variant, deal the
//! roles, run one game, log it to standard error as JSON lines, and tell it
//! on standard output as it happens. Or list the models a provider serves,
//! which belongs to no game.

use anyhow::bail;
use clap::{Args, Parser, Subcommand};
use free_agent::{Behavior, Episode, Logger};
use social_deduction::model::{
    Model, Provider, REQUEST_TIMEOUT, api_key, key_variable, known_providers,
};
use social_deduction::werewolf::llm::{self, Config};
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::{Entry, PhaseLimits, RoleCounts, Rules, Table, Team};
use social_deduction::werewolf::{scripted, uniform_random};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// The command line: a game, which is a subcommand that names one of its
/// variants, or a command that belongs to no game.
#[derive(Parser, Debug, PartialEq)]
#[command(version, about)]
enum CommandLine {
    /// Werewolf: werewolves against villagers, by night and by day.
    #[command(subcommand)]
    Werewolf(Werewolf),
    /// List the models a provider serves, and which of them select when
    /// asked.
    #[command(after_help = known_providers_help())]
    Models {
        /// The root of the provider's OpenAI-compatible API, ending in /v1
        base_url: String,
        /// The environment variable holding the API key, over the one known
        /// for the provider
        #[arg(long)]
        api_key_env: Option<String>,
        /// Ask every model listed, or only the IDs given, for a selection
        /// as a player would, and mark those that select
        #[arg(long, num_args = 0.., value_name = "ID")]
        check: Option<Vec<String>>,
    },
}

/// The variants of Werewolf, each with its own arguments.
#[derive(Subcommand, Debug, PartialEq)]
enum Werewolf {
    /// Every player chooses uniformly at random.
    UniformRandom {
        #[command(flatten)]
        roles: RoleCounts,
        #[command(flatten)]
        logging: Logging,
    },
    /// Every player is a script that answers each announcement at once with
    /// a choice made at random: the model-played game, without a model.
    Scripted {
        #[command(flatten)]
        roles: RoleCounts,
        #[command(flatten)]
        limits: PhaseLimits,
        #[command(flatten)]
        logging: Logging,
    },
    /// Every player is a language model, set up by a configuration file.
    Llm {
        /// The TOML configuration file
        config: PathBuf,
        #[command(flatten)]
        overrides: llm::Overrides,
        #[command(flatten)]
        logging: Logging,
    },
}

/// Whether the game is logged, for every variant to flatten into its
/// command line arguments. It is logged unless `--no-log` says otherwise,
/// and when both flags are given the last one wins, as `clap` has
/// paired flags behave.
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
        CommandLine::Werewolf(Werewolf::UniformRandom { roles, logging }) => {
            let roles = roles.or(Table::default()).deal();
            play(logging.enabled(), |winner, logger| {
                uniform_random::game(roles, Rules::default(), winner, logger, |_| {
                    uniform_random::Actor::player()
                })
            })
            .await
        }
        CommandLine::Werewolf(Werewolf::Scripted {
            roles,
            limits,
            logging,
        }) => {
            let roles = roles.or(Table::default()).deal();
            let rules = limits.or(scripted::rules());
            play(logging.enabled(), |winner, logger| {
                scripted::game(roles, rules, winner, logger)
            })
            .await
        }
        CommandLine::Werewolf(Werewolf::Llm {
            config,
            overrides,
            logging,
        }) => {
            let settings = Config::load(&config)?.settle(overrides);
            // The key is read, every prompt rendered, and the model asked
            // for a selection before any actor is built, so that a
            // variable that is not set, a template that is broken, or a
            // model that does not select fails before anything else.
            let model = &settings.config.model;
            let provider = model.provider()?;
            let prompts = settings.prompts()?;
            provider.check_serves(&model.id).await?;
            llm::check_selects(&provider, &model.id).await?;
            let roles = settings.table.deal();
            let configuration = settings.configuration(&roles, &prompts)?;
            let rules = settings.rules();
            play(logging.enabled(), |winner, logger| {
                llm::game(
                    roles,
                    rules,
                    configuration,
                    Arc::new(provider),
                    winner,
                    logger,
                )
            })
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
            let mut ids = provider.models().await?;
            ids.sort();
            let listing = match check {
                None => plain(&ids),
                Some(check) => marked(&checked(&provider, &base_url, ids, &check).await?),
            };
            print!("{listing}");
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
/// selected when asked, or not at all when it was not asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    /// Asked for a selection, it selected a candidate.
    Selected,
    /// Asked for a selection, it did not.
    DidNotSelect,
    /// Not asked.
    Unchecked,
}

impl Mark {
    /// The mark as the listing shows it before the id: `* `, `! `, or two
    /// spaces, so the ids line up.
    fn shown(self) -> &'static str {
        match self {
            Mark::Selected => "* ",
            Mark::DidNotSelect => "! ",
            Mark::Unchecked => "  ",
        }
    }
}

/// Each of `ids`, in order, with its mark: those in `check`, or every one
/// of them when `check` is empty, are asked by `model` for a selection one
/// at a time, since a local server loads each model it is asked for. The
/// reason a model that did not select failed goes to standard error, one
/// line per model. An id in `check` that is not in `ids` is an error
/// naming it and `base_url`, before anything is asked.
async fn checked(
    model: &dyn Model,
    base_url: &str,
    ids: Vec<String>,
    check: &[String],
) -> anyhow::Result<Vec<(String, Mark)>> {
    if let Some(id) = check.iter().find(|id| !ids.contains(id)) {
        bail!("{base_url} does not serve a model with the id {id}");
    }
    let mut marked = Vec::new();
    for id in ids {
        let mark = if check.is_empty() || check.contains(&id) {
            match llm::check_selects(model, &id).await {
                Ok(()) => Mark::Selected,
                Err(error) => {
                    eprintln!("{error:#}");
                    Mark::DidNotSelect
                }
            }
        } else {
            Mark::Unchecked
        };
        marked.push((id, mark));
    }
    Ok(marked)
}

/// The listing `models` prints: one id per line, in order.
fn plain(ids: &[String]) -> String {
    ids.iter().map(|id| format!("{id}\n")).collect()
}

/// The listing `models --check` prints: one id per line, in order, each
/// after its mark, then, set off by an empty line, a line saying what the
/// marks mean.
fn marked(models: &[(String, Mark)]) -> String {
    let mut listing = String::new();
    for (id, mark) in models {
        writeln!(listing, "{}{id}", mark.shown()).unwrap();
    }
    if !models.is_empty() {
        listing.push('\n');
    }
    writeln!(
        listing,
        "* marks a model that selected when asked, and ! one that did not."
    )
    .unwrap();
    listing
}

/// Play one game, the episode `game` builds once given where to send the
/// winner and the logger the environment holds. The narration goes to
/// standard output as the game happens, and so does the log to standard
/// error as JSON lines when `logging` is on.
async fn play<A: Behavior<Log = Entry> + 'static>(
    logging: bool,
    game: impl FnOnce(oneshot::Sender<Team>, Logger<Entry>) -> Episode<A>,
) -> anyhow::Result<()> {
    // The winner reaches the log too, which is where it is read from here.
    let (winner, _won) = oneshot::channel();
    let (logger, mut log) = unbounded_channel();
    let episode = game(winner, logger);
    let telling = tokio::spawn(async move {
        let mut narrator = Narrator::default();
        let mut stdout = io::stdout();
        while let Some(event) = log.recv().await {
            if logging {
                event.write(io::stderr())?;
            }
            for line in narrator.narrate(&event) {
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
    use social_deduction::model::fake::{FakeModel, choosing_first};
    use std::collections::HashSet;

    /// The command line `args` parse to, after the binary's name.
    fn parsed(args: &[&str]) -> CommandLine {
        CommandLine::parse_from([&["social-deduction"], args].concat())
    }

    #[test]
    fn the_command_line_is_a_game_then_a_variant_that_tells_unset_from_given() {
        CommandLine::command().debug_assert();
        assert_eq!(
            parsed(&["werewolf", "uniform-random", "--werewolves", "1"]),
            CommandLine::Werewolf(Werewolf::UniformRandom {
                roles: RoleCounts {
                    werewolves: Some(1),
                    ..RoleCounts::default()
                },
                logging: Logging::default(),
            })
        );
    }

    #[test]
    fn every_variant_takes_the_log_flags_and_the_last_of_the_two_wins() {
        let off = || Logging {
            log: false,
            no_log: true,
        };
        let on = || Logging {
            log: true,
            no_log: false,
        };
        assert_eq!(
            parsed(&["werewolf", "uniform-random", "--no-log"]),
            CommandLine::Werewolf(Werewolf::UniformRandom {
                roles: RoleCounts::default(),
                logging: off(),
            })
        );
        assert_eq!(
            parsed(&["werewolf", "scripted", "--no-log", "--log"]),
            CommandLine::Werewolf(Werewolf::Scripted {
                roles: RoleCounts::default(),
                limits: PhaseLimits::default(),
                logging: on(),
            })
        );
        assert_eq!(
            parsed(&["werewolf", "llm", "game.toml", "--log", "--no-log"]),
            CommandLine::Werewolf(Werewolf::Llm {
                config: PathBuf::from("game.toml"),
                overrides: llm::Overrides::default(),
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
    fn the_llm_variant_takes_the_file_the_role_counts_the_limits_and_the_model() {
        let args = [
            "game.toml",
            "--seers",
            "2",
            "--night-limit",
            "30s",
            "--model-id",
            "gpt-5.5",
        ];
        assert_eq!(
            parsed(&[&["werewolf", "llm"], &args[..]].concat()),
            CommandLine::Werewolf(Werewolf::Llm {
                config: PathBuf::from("game.toml"),
                overrides: llm::Overrides {
                    roles: RoleCounts {
                        seers: Some(2),
                        ..RoleCounts::default()
                    },
                    limits: PhaseLimits {
                        night_limit: Some(Duration::from_secs(30)),
                        day_limit: None,
                    },
                    model: llm::ModelOverrides {
                        base_url: None,
                        id: Some("gpt-5.5".to_string()),
                    },
                },
                logging: Logging::default(),
            })
        );
    }

    #[test]
    fn the_llm_help_lists_the_model_options_under_a_heading_of_their_own() {
        let help = CommandLine::command()
            .find_subcommand_mut("werewolf")
            .unwrap()
            .find_subcommand_mut("llm")
            .unwrap()
            .render_help()
            .to_string();
        let at = |text: &str| {
            help.find(text)
                .unwrap_or_else(|| panic!("{text} in {help}"))
        };
        assert!(at("Options:") < at("--werewolves"), "{help}");
        assert!(at("--werewolves") < at("\nModel:\n"), "{help}");
        assert!(at("\nModel:\n") < at("--model-base-url"), "{help}");
        assert!(at("--model-base-url") < at("--model-id"), "{help}");
    }

    #[test]
    fn the_scripted_variant_takes_the_role_counts_and_the_limits() {
        let args = [
            "werewolf",
            "scripted",
            "--villagers",
            "4",
            "--day-limit",
            "2m",
        ];
        assert_eq!(
            parsed(&args),
            CommandLine::Werewolf(Werewolf::Scripted {
                roles: RoleCounts {
                    villagers: Some(4),
                    ..RoleCounts::default()
                },
                limits: PhaseLimits {
                    night_limit: None,
                    day_limit: Some(Duration::from_secs(120)),
                },
                logging: Logging::default(),
            })
        );
    }

    #[test]
    fn the_help_names_the_defaults_the_code_fills_in() {
        let table = Table::default();
        let counts = vec![
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
        let limited = [counts.clone(), limits.to_vec()].concat();
        let variants = [
            ("uniform-random", counts),
            ("scripted", limited.clone()),
            ("llm", limited),
        ];
        for (variant, defaults) in variants {
            let help = CommandLine::command()
                .find_subcommand_mut("werewolf")
                .unwrap()
                .find_subcommand_mut(variant)
                .unwrap()
                .render_help()
                .to_string();
            for (option, default) in defaults {
                let line = help.lines().find(|line| line.contains(option)).unwrap();
                assert!(line.ends_with(&format!("[default: {default}]")), "{line}");
            }
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
        assert!(CommandLine::try_parse_from(["social-deduction", "werewolf", "models"]).is_err());
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
    fn the_plain_listing_is_one_id_per_line_with_no_legend() {
        let ids = ["gpt-0", "qwen2.5-7b-instruct"].map(String::from);
        assert_eq!(plain(&ids), "gpt-0\nqwen2.5-7b-instruct\n");
        assert_eq!(plain(&[]), "");
    }

    /// The legend the marked listing ends with.
    const LEGEND: &str = "* marks a model that selected when asked, and ! one that did not.\n";

    #[test]
    fn the_marked_listing_marks_each_id_lines_them_up_and_says_what_the_marks_mean() {
        let models = [
            ("gpt-0".to_string(), Mark::DidNotSelect),
            ("llama".to_string(), Mark::Unchecked),
            ("qwen2.5-7b-instruct".to_string(), Mark::Selected),
        ];
        assert_eq!(
            marked(&models),
            format!("! gpt-0\n  llama\n* qwen2.5-7b-instruct\n\n{LEGEND}")
        );
        assert_eq!(marked(&[]), LEGEND);
    }

    /// A model that selects as `id` and refuses as anything else.
    fn selecting_only_as(id: &'static str) -> FakeModel {
        FakeModel::answering(move |request| {
            if request.model == id {
                choosing_first(request)
            } else {
                Err(anyhow::anyhow!("{} does not support tools", request.model))
            }
        })
    }

    #[tokio::test]
    async fn every_model_listed_is_checked_unless_ids_are_given_and_then_only_those() {
        let ids = ["gpt-0", "llama", "qwen2.5-7b-instruct"].map(String::from);
        let base_url = "http://localhost:1234/v1";
        let model = selecting_only_as("qwen2.5-7b-instruct");
        let marked = checked(&model, base_url, ids.to_vec(), &[]).await.unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Mark::DidNotSelect),
                ("llama".to_string(), Mark::DidNotSelect),
                ("qwen2.5-7b-instruct".to_string(), Mark::Selected),
            ]
        );
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(asked, ids);
        // Only the ids given, in the listing's order.
        let model = selecting_only_as("qwen2.5-7b-instruct");
        let check = ["qwen2.5-7b-instruct", "gpt-0"].map(String::from);
        let marked = checked(&model, base_url, ids.to_vec(), &check)
            .await
            .unwrap();
        assert_eq!(
            marked,
            [
                ("gpt-0".to_string(), Mark::DidNotSelect),
                ("llama".to_string(), Mark::Unchecked),
                ("qwen2.5-7b-instruct".to_string(), Mark::Selected),
            ]
        );
        let asked: Vec<String> = model.requests().into_iter().map(|r| r.model).collect();
        assert_eq!(asked, ["gpt-0", "qwen2.5-7b-instruct"]);
    }

    #[tokio::test]
    async fn an_id_to_check_that_is_not_listed_is_an_error_naming_it_before_anything_is_asked() {
        let ids = ["qwen2.5-7b-instruct"].map(String::from);
        let model = selecting_only_as("qwen2.5-7b-instruct");
        let check = ["qwen2.5-7b-instruct", "gpt-0"].map(String::from);
        let error = checked(&model, "http://localhost:1234/v1", ids.to_vec(), &check)
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("gpt-0"), "{shown}");
        assert!(shown.contains("http://localhost:1234/v1"), "{shown}");
        assert!(model.requests().is_empty());
    }
}
