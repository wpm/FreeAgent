//! Play a social deduction game: name the game and its variant, deal the
//! roles, run one game, log it to standard error as JSON lines, and tell it
//! on standard output as it happens.

use clap::{Parser, Subcommand};
use social_deduction::werewolf::llm::{self, Config};
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::uniform_random::{Actor, game};
use social_deduction::werewolf::{RoleCounts, Rules, Table};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// The games. Each is a subcommand that names one of its variants.
#[derive(Parser, Debug)]
#[command(version, about)]
enum Game {
    /// Werewolf: werewolves against villagers, by night and by day.
    #[command(subcommand)]
    Werewolf(Werewolf),
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
    match Game::parse() {
        Game::Werewolf(Werewolf::UniformRandom { roles }) => {
            uniform_random(roles.or(Table::default())).await
        }
        Game::Werewolf(Werewolf::Llm { config, overrides }) => {
            let settings = Config::load(&config)?.settle(overrides);
            // The key is read and every prompt rendered now, so that a
            // variable that is not set or a template that is broken fails
            // before anything else. The game that would use them comes later.
            let _api_key = settings.config.model.api_key()?;
            let prompts = settings.prompts()?;
            print!("{settings}");
            for prompt in &prompts {
                print!("{prompt}");
            }
            println!("The model-played game is not playable yet.");
            Ok(())
        }
    }
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
        Game::command().debug_assert();
        let Game::Werewolf(Werewolf::UniformRandom { roles }) = Game::parse_from([
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
        let Game::Werewolf(Werewolf::Llm { config, overrides }) = Game::parse_from([
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
            let help = Game::command()
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
}
