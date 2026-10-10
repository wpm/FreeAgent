//! Play a social deduction game: name the game and its variant, deal the
//! roles, run one game, log it to standard error as JSON lines, and tell it
//! on standard output as it happens.

use clap::{Args, Parser, Subcommand};
use rand::seq::SliceRandom;
use social_deduction::werewolf::report::Narrator;
use social_deduction::werewolf::uniform_random::{Actor, game};
use social_deduction::werewolf::{PlayerId, Role, Rules};
use std::collections::HashMap;
use std::io::{self, Write};
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
}

/// How many of each role sit at the table, for a variant to flatten into its
/// arguments. A count left unset has no `clap` default, so that a variant
/// can tell it apart from a count that was given, and fill it in from
/// [`Table::default`] or from somewhere else.
#[derive(Args, Debug, Default)]
struct RoleCounts {
    /// How many werewolves [default: 2]
    #[arg(long)]
    werewolves: Option<usize>,
    /// How many plain villagers [default: 3]
    #[arg(long)]
    villagers: Option<usize>,
    /// How many doctors [default: 1]
    #[arg(long)]
    doctors: Option<usize>,
    /// How many seers [default: 1]
    #[arg(long)]
    seers: Option<usize>,
}

impl RoleCounts {
    /// The table these counts seat, with any count left unset taken from
    /// `default`.
    fn or(&self, default: Table) -> Table {
        Table {
            werewolves: self.werewolves.unwrap_or(default.werewolves),
            villagers: self.villagers.unwrap_or(default.villagers),
            doctors: self.doctors.unwrap_or(default.doctors),
            seers: self.seers.unwrap_or(default.seers),
        }
    }
}

/// How many of each role sit at the table.
#[derive(Debug, PartialEq, Eq)]
struct Table {
    werewolves: usize,
    villagers: usize,
    doctors: usize,
    seers: usize,
}

impl Default for Table {
    /// Two werewolves, three villagers, a doctor and a seer: seven players.
    fn default() -> Self {
        Table {
            werewolves: 2,
            villagers: 3,
            doctors: 1,
            seers: 1,
        }
    }
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
    let Game::Werewolf(Werewolf::UniformRandom { roles }) = Game::parse();
    let table = roles.or(Table::default());
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
    use std::collections::HashSet;

    #[test]
    fn the_command_line_is_a_game_then_a_variant_that_tells_unset_from_given() {
        Game::command().debug_assert();
        let Game::Werewolf(Werewolf::UniformRandom { roles }) = Game::parse_from([
            "social-deduction",
            "werewolf",
            "uniform-random",
            "--werewolves",
            "1",
        ]);
        assert_eq!(roles.werewolves, Some(1));
        assert_eq!(roles.villagers, None);
    }

    #[test]
    fn the_help_names_the_defaults_the_code_fills_in() {
        let help = Game::command()
            .find_subcommand_mut("werewolf")
            .unwrap()
            .find_subcommand_mut("uniform-random")
            .unwrap()
            .render_help()
            .to_string();
        let table = Table::default();
        for (count, default) in [
            ("--werewolves", table.werewolves),
            ("--villagers", table.villagers),
            ("--doctors", table.doctors),
            ("--seers", table.seers),
        ] {
            let line = help.lines().find(|line| line.contains(count)).unwrap();
            assert!(line.ends_with(&format!("[default: {default}]")), "{line}");
        }
    }

    #[test]
    fn unset_counts_are_filled_from_the_defaults() {
        let roles = RoleCounts {
            werewolves: Some(1),
            ..RoleCounts::default()
        };
        assert_eq!(
            roles.or(Table::default()),
            Table {
                werewolves: 1,
                ..Table::default()
            }
        );
        assert_eq!(RoleCounts::default().or(Table::default()), Table::default());
    }

    #[test]
    fn the_deal_seats_every_role_as_many_times_as_asked() {
        let roles = Table::default().deal();
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
