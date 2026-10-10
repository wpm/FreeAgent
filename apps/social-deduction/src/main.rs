//! Play a social deduction game: name the game and its variant, deal the
//! roles, run one game, log it to standard error as JSON lines, and tell it
//! on standard output as it happens. Or list the models a provider serves,
//! which belongs to no game.

use clap::{Parser, Subcommand};
use social_deduction::model::{check_makes_tool_calls, makes_tool_calls};
use social_deduction::werewolf::llm::{self, Config};
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::uniform_random::{Actor, game};
use social_deduction::werewolf::{RoleCounts, Rules, Table};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// The command line: a game, which is a subcommand that names one of its
/// variants, or a command that belongs to no game.
#[derive(Parser, Debug)]
#[command(version, about)]
enum CommandLine {
    /// Werewolf: werewolves against villagers, by night and by day.
    #[command(subcommand)]
    Werewolf(Werewolf),
    /// List the models a provider serves, marking those known to make tool
    /// calls.
    Models {
        /// The TOML configuration file whose [model] table names the provider
        #[arg(long)]
        config: PathBuf,
    },
}

/// The variants of Werewolf, each with its own arguments.
#[derive(Subcommand, Debug)]
enum Werewolf {
    /// Every player chooses uniformly at random.
    UniformRandom {
        #[command(flatten)]
        roles: RoleCounts,
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
            uniform_random(roles.or(Table::default())).await
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
        CommandLine::Models { config } => {
            let mut ids = Config::load(&config)?.model.provider()?.models().await?;
            ids.sort();
            print!("{}", marked(&ids));
            Ok(())
        }
    }
}

/// The listing the `models` command prints: one id per line, those known to
/// make tool calls marked `*` and the rest indented to line up, then a line
/// saying what the mark means.
fn marked(ids: &[String]) -> String {
    let mut listing = String::new();
    for id in ids {
        let mark = if makes_tool_calls(id) { "*" } else { " " };
        writeln!(listing, "{mark} {id}").unwrap();
    }
    writeln!(listing, "* marks a model known to make tool calls.").unwrap();
    listing
}

/// Play one uniform-random game at `table`.
async fn uniform_random(table: Table) -> anyhow::Result<()> {
    // The winner reaches the log too, which is where it is read from here.
    let (winner, _won) = oneshot::channel();
    let (logger, mut log) = unbounded_channel();
    let episode = game(table.deal(), Rules::default(), winner, logger, |_| {
        Actor::player()
    });
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

    #[test]
    fn the_command_line_is_a_game_then_a_variant_that_tells_unset_from_given() {
        CommandLine::command().debug_assert();
        let CommandLine::Werewolf(Werewolf::UniformRandom { roles }) = CommandLine::parse_from([
            "social-deduction",
            "werewolf",
            "uniform-random",
            "--werewolves",
            "1",
        ]) else {
            panic!()
        };
        assert_eq!(roles.werewolves, Some(1));
        assert_eq!(roles.villagers, None);
    }

    #[test]
    fn the_llm_variant_takes_the_file_the_role_counts_and_the_limits() {
        let CommandLine::Werewolf(Werewolf::Llm { config, overrides }) = CommandLine::parse_from([
            "social-deduction",
            "werewolf",
            "llm",
            "--config",
            "game.toml",
            "--seers",
            "2",
            "--night-limit",
            "30s",
        ]) else {
            panic!()
        };
        assert_eq!(config, PathBuf::from("game.toml"));
        assert_eq!(overrides.roles.seers, Some(2));
        assert_eq!(overrides.night_limit, Some(Duration::from_secs(30)));
        assert_eq!(overrides.day_limit, None);
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
        let llm = [counts.clone(), limits.to_vec()].concat();
        for (variant, defaults) in [("uniform-random", counts), ("llm", llm)] {
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
    fn the_models_command_takes_the_file_and_belongs_to_no_game() {
        let CommandLine::Models { config } =
            CommandLine::parse_from(["social-deduction", "models", "--config", "game.toml"])
        else {
            panic!()
        };
        assert_eq!(config, PathBuf::from("game.toml"));
        assert!(CommandLine::try_parse_from(["social-deduction", "werewolf", "models"]).is_err());
    }

    #[test]
    fn the_listing_marks_the_models_known_to_make_tool_calls_and_says_so() {
        let ids = ["gpt-0", "qwen2.5-7b-instruct"].map(String::from);
        assert_eq!(
            marked(&ids),
            "  gpt-0\n* qwen2.5-7b-instruct\n* marks a model known to make tool calls.\n"
        );
        assert_eq!(marked(&[]), "* marks a model known to make tool calls.\n");
    }
}
