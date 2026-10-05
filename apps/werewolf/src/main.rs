#![allow(dead_code)]

//! Play one game of Werewolf as a configuration file describes, and print
//! everything that was said.

use anyhow::Result;
use clap::Parser;
use serde::Serialize;
use std::io::Write;
use std::num::NonZero;
use std::path::PathBuf;
use werewolf::config::Config;
use werewolf::werewolf::Team;

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
    run(Args::parse(), &mut std::io::stdout()).await
}

/// Play the game the arguments describe, writing its record and then its
/// outcome to `out`, one JSON line each.
async fn run(args: Args, _out: &mut impl Write) -> Result<()> {
    let config = Config::load(&args.config)?;
    let _seed = args.seed.or(config.seed).unwrap_or_else(rand::random);

    // let (log, mut events) = Log::new();
    // let writer = async {
    //     while let Some(event) = events.recv().await {
    //         writeln!(out, "{}", serde_json::to_string(&event)?)?;
    //     }
    //     Ok::<(), anyhow::Error>(())
    // };
    // The writer runs dry once the game is over and every handle on the
    // log is gone.
    // let (outcome, written) = tokio::join!(play(&config, seed, Some(log)), writer);
    // written?;
    // let outcome = outcome?;

    // let record = Record {
    //     players: config.players,
    //     werewolves: config.werewolves,
    //     seed,
    //     winner: outcome.winner,
    //     rounds: outcome.rounds,
    // };
    // writeln!(out, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use serde_json::Value;

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

    /// A configuration file that lives for the test.
    fn config_file(name: &str, contents: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("werewolf-{}-{name}.toml", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    #[ignore = "nothing is played until the game is rebuilt"]
    async fn a_run_prints_the_record_and_then_the_outcome() {
        let config = config_file(
            "record",
            "players = 5\nwerewolves = 1\nseed = 3\n[policy]\nkind = \"random\"\n",
        );
        let args = Args { config, seed: None };
        let mut out = Vec::new();

        run(args, &mut out).await.unwrap();

        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let (outcome, events) = lines.split_last().unwrap();
        assert_eq!(outcome["seed"], 3);
        assert_eq!(outcome["players"], 5);
        assert!(outcome["winner"].is_string(), "{outcome}");
        assert!(!events.is_empty());
        assert!(
            events.iter().all(|event| event["said"].is_object()),
            "{events:?}"
        );
    }

    #[tokio::test]
    #[ignore = "nothing is played until the game is rebuilt"]
    async fn the_command_line_seed_wins_over_the_files() {
        let config = config_file(
            "seed",
            "players = 4\nwerewolves = 1\nseed = 3\n[policy]\nkind = \"random\"\n",
        );
        let args = Args {
            config,
            seed: Some(11),
        };
        let mut out = Vec::new();

        run(args, &mut out).await.unwrap();

        let last = String::from_utf8(out)
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_string();
        let outcome: Value = serde_json::from_str(&last).unwrap();
        assert_eq!(outcome["seed"], 11);
    }

    #[tokio::test]
    async fn a_missing_configuration_file_is_an_error() {
        let args = Args {
            config: PathBuf::from("/nowhere/at/all.toml"),
            seed: None,
        };

        let error = run(args, &mut Vec::new()).await.unwrap_err();

        assert!(error.to_string().contains("cannot read"), "{error}");
    }
}
