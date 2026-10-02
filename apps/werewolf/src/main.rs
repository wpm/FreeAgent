//! Runs a single episode of Werewolf and prints its log.

use anyhow::Result;
use clap::Parser;
use free_agent::actor::{Actor, ActorId};
use free_agent::episode::{self, Entry, Sink};
use rand::prelude::*;
use rand::rngs::StdRng;
use std::io::Write;
use std::time::{Duration, SystemTime};
use werewolf::environment::{Environment, standard_roles};
use werewolf::game::Village;
use werewolf::player::random;
use werewolf::protocol::Message;

/// One episode of Werewolf, played by random agents, logged to stdout.
#[derive(Debug, Parser)]
#[command(name = "werewolf", version, about)]
struct Options {
    /// How many players sit down.
    #[arg(short, long, default_value_t = 8)]
    players: usize,

    /// Seed for the roles and every player's choices, so a game can be
    /// replayed exactly.
    #[arg(short, long)]
    seed: Option<u64>,

    /// How long a round of asking may take, in milliseconds.
    #[arg(short, long, default_value_t = 2000)]
    timeout: u64,

    /// Stop the episode after this long, in seconds, however it is going.
    /// Nothing in the rules ends a game in which nobody dies, so this is
    /// what does.
    #[arg(long, default_value_t = 60)]
    limit: u64,
}

/// Writes one JSON object per line to standard output: the training log.
///
/// Each line is rendered whole before it reaches the stream, so concurrent
/// actors never split one another's output.
#[derive(Debug)]
struct Stdout;

impl Sink for Stdout {
    fn write(&self, entry: Entry) -> Result<()> {
        let at = entry
            .at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let mut line = serde_json::json!({
            "at": at,
            "actor": entry.actor,
            "payload": entry.payload,
        })
        .to_string();
        line.push('\n');
        std::io::stdout().lock().write_all(line.as_bytes())?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        std::io::stdout().lock().flush()?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse();
    let seed = options.seed.unwrap_or_else(rand::random);

    // One generator decides the roles; each player gets its own, derived
    // from the same seed, so the whole episode replays from one number.
    let mut rng = StdRng::seed_from_u64(seed);
    let mut roles = standard_roles(options.players);
    roles.shuffle(&mut rng);

    let village = Village::numbered(roles);
    let names: Vec<ActorId> = village.everyone().iter().map(|p| p.id.clone()).collect();

    let mut roster: Vec<(ActorId, Box<dyn Actor<Message>>)> = vec![(
        "Environment".to_string(),
        Box::new(Environment::seeded(
            village,
            Duration::from_millis(options.timeout),
            seed,
        )),
    )];
    for (n, name) in names.iter().enumerate() {
        roster.push((name.clone(), Box::new(random(seed ^ n as u64))));
    }

    episode::episode(roster, Stdout, Some(Duration::from_secs(options.limit))).await
}
