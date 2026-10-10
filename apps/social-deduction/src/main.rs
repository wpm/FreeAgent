//! Play a social deduction game: name the game and its variant, deal the
//! roles, run one game, log it to standard error as JSON lines, and tell it
//! on standard output as it happens. Or list the models a provider serves,
//! which belongs to no game.

use clap::{Parser, Subcommand};
use free_agent::{Behavior, Episode, Logger};
use social_deduction::model::{
    Provider, REQUEST_TIMEOUT, api_key, check_makes_tool_calls, key_variable, known_providers,
    makes_tool_calls,
};
use social_deduction::werewolf::llm::{self, Config};
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::{Entry, PhaseLimits, RoleCounts, Rules, Table, Team};
use social_deduction::werewolf::{scripted, uniform_random};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
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
    /// List the models a provider serves, marking those known to make tool
    /// calls.
    #[command(after_help = known_providers_help())]
    Models {
        /// The root of the provider's OpenAI-compatible API, ending in /v1
        base_url: String,
        /// The environment variable holding the API key, over the one known
        /// for the provider
        #[arg(long)]
        api_key_env: Option<String>,
    },
}

/// The variants of Werewolf, each with its own arguments.
#[derive(Subcommand, Debug, PartialEq)]
enum Werewolf {
    /// Every player chooses uniformly at random.
    UniformRandom {
        #[command(flatten)]
        roles: RoleCounts,
    },
    /// Every player is a script that answers each announcement at once with
    /// a choice made at random: the model-played game, without a model.
    Scripted {
        #[command(flatten)]
        roles: RoleCounts,
        #[command(flatten)]
        limits: PhaseLimits,
    },
    /// Every player is a language model, set up by a configuration file.
    Llm {
        /// The TOML configuration file
        #[arg(long)]
        config: PathBuf,
        #[command(flatten)]
        overrides: llm::Overrides,
    },
}

/// How long a game may take before it is abandoned.
const PATIENCE: Duration = Duration::from_secs(60 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match CommandLine::parse() {
        CommandLine::Werewolf(Werewolf::UniformRandom { roles }) => {
            let roles = roles.or(Table::default()).deal();
            play(|winner, logger| {
                uniform_random::game(roles, Rules::default(), winner, logger, |_| {
                    uniform_random::Actor::player()
                })
            })
            .await
        }
        CommandLine::Werewolf(Werewolf::Scripted { roles, limits }) => {
            let roles = roles.or(Table::default()).deal();
            let rules = limits.or(scripted::rules());
            play(|winner, logger| scripted::game(roles, rules, winner, logger)).await
        }
        CommandLine::Werewolf(Werewolf::Llm { config, overrides }) => {
            let settings = Config::load(&config)?.settle(overrides);
            // The key is read, every prompt rendered, and the model checked
            // now, so that a variable that is not set, a template that is
            // broken, or a model that cannot play fails before anything
            // else. The game that would use them comes later.
            let model = &settings.config.model;
            let provider = model.provider()?;
            let prompts = settings.prompts()?;
            check_makes_tool_calls(&model.id)?;
            provider.check_serves(&model.id).await?;
            print!("{settings}");
            for prompt in &prompts {
                print!("{prompt}");
            }
            println!("The model-played game is not playable yet.");
            Ok(())
        }
        CommandLine::Models {
            base_url,
            api_key_env,
        } => {
            let variable = key_variable(&base_url, api_key_env.as_deref());
            let api_key = variable.map(api_key).transpose()?;
            let provider = Provider::new(base_url, api_key, REQUEST_TIMEOUT)?;
            print!("{}", marked(provider.models().await?));
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

/// The listing the `models` command prints: one id per line in sorted
/// order, those known to make tool calls marked `*` and the rest indented
/// to line up, then, set off by an empty line, a line saying what the
/// mark means.
fn marked(mut ids: Vec<String>) -> String {
    ids.sort();
    let mut listing = String::new();
    for id in &ids {
        let mark = if makes_tool_calls(id) { "*" } else { " " };
        writeln!(listing, "{mark} {id}").unwrap();
    }
    if !ids.is_empty() {
        listing.push('\n');
    }
    writeln!(listing, "* marks a model known to make tool calls.").unwrap();
    listing
}

/// Play one game, the episode `game` builds once given where to send the
/// winner and the logger the environment holds. The log goes to standard
/// error as JSON lines and the narration to standard output as the game
/// happens.
async fn play<A: Behavior<Log = Entry> + 'static>(
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
            event.write(io::stderr())?;
            for line in narrator.narrate(&event.payload) {
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
                }
            })
        );
    }

    #[test]
    fn the_llm_variant_takes_the_file_the_role_counts_and_the_limits() {
        let args = [
            "--config",
            "game.toml",
            "--seers",
            "2",
            "--night-limit",
            "30s",
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
                },
            })
        );
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
            }
        );
        assert!(CommandLine::try_parse_from(["social-deduction", "werewolf", "models"]).is_err());
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
            rows.contains(&"  https://api.openai.com     OPENAI_API_KEY"),
            "{table}"
        );
        assert!(
            rows.contains(&"  https://api.anthropic.com  ANTHROPIC_API_KEY"),
            "{table}"
        );
        // The variables line up, whatever the roots' lengths.
        let columns: HashSet<Option<usize>> = rows.iter().map(|row| row.rfind("  ")).collect();
        assert_eq!(columns.len(), 1, "{table}");
    }

    #[test]
    fn the_listing_is_sorted_marks_the_models_known_to_make_tool_calls_and_says_so() {
        let ids = ["qwen2.5-7b-instruct", "gpt-0"].map(String::from).to_vec();
        assert_eq!(
            marked(ids),
            "  gpt-0\n* qwen2.5-7b-instruct\n\n* marks a model known to make tool calls.\n"
        );
        assert_eq!(
            marked(vec![]),
            "* marks a model known to make tool calls.\n"
        );
    }
}
