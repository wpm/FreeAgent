//! Play Werewolf: deal the roles, run one game, log it to standard error
//! as JSON lines, and tell it on standard output as it happens.

use clap::Parser;
use rand::seq::SliceRandom;
use social_deduction::report::Narrator;
use social_deduction::{Actor, PlayerId, Role, Rules, game};
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// How many of each role sit at the table.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Table {
    /// How many werewolves.
    #[arg(long, default_value_t = 2)]
    werewolves: usize,
    /// How many plain villagers.
    #[arg(long, default_value_t = 3)]
    villagers: usize,
    /// How many doctors.
    #[arg(long, default_value_t = 1)]
    doctors: usize,
    /// How many seers.
    #[arg(long, default_value_t = 1)]
    seers: usize,
}

impl Table {
    /// Deal the roles at random to players named `player1` onward. The
    /// names say nothing about the roles, since every player sees every
    /// name.
    fn deal(&self) -> HashMap<PlayerId, Role> {
        let mut roles = Vec::new();
        for (role, count) in [
            (Role::Werewolf, self.werewolves),
            (Role::Villager, self.villagers),
            (Role::Doctor, self.doctors),
            (Role::Seer, self.seers),
        ] {
            roles.extend(std::iter::repeat_n(role, count));
        }
        roles.shuffle(&mut rand::rng());
        roles
            .into_iter()
            .enumerate()
            .map(|(seat, role)| (format!("player{}", seat + 1), role))
            .collect()
    }
}

/// How long a game may take before it is abandoned.
const PATIENCE: Duration = Duration::from_secs(60 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let table = Table::parse();
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
    use std::collections::HashSet;

    #[test]
    fn the_deal_seats_every_role_as_many_times_as_asked() {
        let table = Table {
            werewolves: 2,
            villagers: 3,
            doctors: 1,
            seers: 1,
        };
        let roles = table.deal();
        let count = |role| roles.values().filter(|r| **r == role).count();
        assert_eq!(roles.len(), 7);
        assert_eq!(count(Role::Werewolf), 2);
        assert_eq!(count(Role::Villager), 3);
        assert_eq!(count(Role::Doctor), 1);
        assert_eq!(count(Role::Seer), 1);
    }

    #[test]
    fn the_deal_names_players_without_giving_away_their_roles() {
        let table = Table {
            werewolves: 1,
            villagers: 2,
            doctors: 0,
            seers: 0,
        };
        let mut names: Vec<_> = table.deal().into_keys().collect();
        names.sort();
        assert_eq!(names, ["player1", "player2", "player3"]);
    }

    #[test]
    fn the_deal_is_shuffled() {
        let table = Table {
            werewolves: 1,
            villagers: 1,
            doctors: 0,
            seers: 0,
        };
        let wolves: HashSet<PlayerId> = (0..50)
            .flat_map(|_| {
                table
                    .deal()
                    .into_iter()
                    .filter(|(_, role)| *role == Role::Werewolf)
                    .map(|(name, _)| name)
            })
            .collect();
        assert_eq!(wolves.len(), 2, "{wolves:?}");
    }
}
