//! Play one game of Werewolf as a configuration file describes, and print
//! everything that was said.

use anyhow::Result;
use clap::Parser;
use free_agent::Log;
use serde::Serialize;
use std::num::NonZero;
use std::path::PathBuf;
use werewolf::game::{Config, Team, play};

/// Play one game of Werewolf as a configuration file describes.
///
/// The file says how many players there are, how many are werewolves,
/// and what decides for them: uniform chance, after Braverman, Etesami,
/// and Mossel, or a language model speaking the OpenAI chat protocol.
/// See the configs directory for examples.
///
/// Prints the record of the game as JSON, one line per event: every
/// request and every reply between the moderator and the players, in the
/// order they happened. The last line says how the game ended.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// The configuration file to play.
    config: PathBuf,

    /// The seed every random choice follows from, overriding the file's.
    /// The same seed plays the same game, as far as the players are
    /// deterministic. Drawn at random when neither gives one, and always
    /// printed.
    #[arg(long)]
    seed: Option<u64>,
}

/// The last line printed: how the game ended.
#[derive(Debug, Serialize)]
struct Record {
    players: u8,
    werewolves: u8,
    seed: u64,
    winner: Team,
    rounds: NonZero<u8>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    let seed = args.seed.or(config.seed).unwrap_or_else(rand::random);

    let (log, mut events) = Log::new();
    let writer = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            println!("{}", serde_json::to_string(&event)?);
        }
        Ok::<(), anyhow::Error>(())
    });

    let outcome = play(&config, seed, Some(log)).await?;
    // The game is over, so every handle on the log is gone and the writer
    // runs dry.
    writer.await??;

    let record = Record {
        players: config.players,
        werewolves: config.werewolves,
        seed,
        winner: outcome.winner,
        rounds: outcome.rounds,
    };
    println!("{}", serde_json::to_string(&record)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        Args::command().debug_assert();
    }

    #[test]
    fn a_configuration_file_is_required() {
        assert!(Args::try_parse_from(["werewolf"]).is_err());

        let args = Args::try_parse_from(["werewolf", "game.toml"]).unwrap();

        assert_eq!(args.config, PathBuf::from("game.toml"));
        assert_eq!(args.seed, None);
    }

    #[test]
    fn the_seed_can_be_overridden() {
        let args = Args::try_parse_from(["werewolf", "game.toml", "--seed", "7"]).unwrap();

        assert_eq!(args.seed, Some(7));
    }
}
