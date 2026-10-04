//! Werewolf played at random, after "Mafia: A Theoretical Study of Players
//! and Coalitions in a Partial Information Environment" by Braverman,
//! Etesami, and Mossel.
//!
//! The paper strips the game to its arithmetic. Each round has a day, when
//! every living player votes and one of them is eliminated, and a night,
//! when the werewolves kill a villager. Nobody has any information to act
//! on, so everyone chooses uniformly at random, and the only question is
//! how many werewolves it takes to make the game fair. The paper's answer
//! is about the square root of the number of players.
//!
//! Here the game is an episode in the [`free_agent`] framework. A
//! [`Moderator`] actor runs it: it deals the roles, asks the living
//! players for their votes by day and the werewolves for their victim by
//! night, and tells everyone when it is over. Each player is a
//! [`UniformRandom`] actor that answers every question with a uniformly
//! random choice. With everyone choosing uniformly, a plurality vote with
//! ties broken at random eliminates a uniformly random candidate, which is
//! exactly the paper's rule.

pub use crate::config::{Config, PolicyConfig};
pub use crate::variation::llm::Llm;
pub use crate::moderator::{Moderator, Settings};
pub use crate::variation::random::UniformRandom;

use crate::moderator;
use anyhow::{Result, ensure};
use free_agent::{ActorId, Ending, Log, Policy, episode};
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZero;
use std::time::Duration;
use tokio::sync::oneshot;

/// The two sides. A game ends when one of them has won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// Kill by night and win on reaching parity.
    Werewolves,
    /// Vote by day and win when the last werewolf is gone.
    Villagers,
}

/// What a player is. The random game deals only werewolves and villagers;
/// the other roles are the paper's extensions, kept for when they are
/// played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Kills by night, and knows the other werewolves.
    Werewolf,
    /// Votes by day and nothing more.
    Villager,
    /// Saves one player from the night's kill.
    Doctor,
    /// Learns one player's team each night.
    Seer,
}

impl Role {
    /// Which side this role wins with.
    pub fn team(self) -> Team {
        match self {
            Role::Werewolf => Team::Werewolves,
            Role::Villager | Role::Doctor | Role::Seer => Team::Villagers,
        }
    }
}

/// Everything the moderator knows: whose turn it is, who is what, and who
/// is still alive.
#[derive(Debug, Clone)]
pub struct State {
    /// The turn being played.
    pub turn: Turn,
    /// Every player's role, dead or alive.
    pub role: HashMap<PlayerId, Role>,
    /// Whether each player is still in the game.
    pub alive: HashMap<PlayerId, bool>,
}

impl State {
    /// A game about to begin, with everyone alive.
    pub fn new(role: HashMap<PlayerId, Role>) -> Self {
        let alive = role.keys().map(|id| (id.clone(), true)).collect();
        State {
            turn: Turn::FIRST,
            role,
            alive,
        }
    }

    /// Everyone still in the game, in a fixed order.
    pub fn living(&self) -> Vec<PlayerId> {
        let mut living: Vec<_> = self
            .alive
            .iter()
            .filter(|(_, alive)| **alive)
            .map(|(id, _)| id.clone())
            .collect();
        living.sort();
        living
    }

    /// Everyone still in the game on one side, in a fixed order.
    pub fn living_on(&self, team: Team) -> Vec<PlayerId> {
        self.living()
            .into_iter()
            .filter(|id| self.role[id].team() == team)
            .collect()
    }

    /// Whether a player is still in the game.
    pub fn is_alive(&self, id: &PlayerId) -> bool {
        self.alive.get(id).copied().unwrap_or(false)
    }

    /// Take a player out of the game.
    pub fn kill(&mut self, id: &PlayerId) {
        if let Some(alive) = self.alive.get_mut(id) {
            *alive = false;
        }
    }

    /// Who has won, if anyone. The werewolves win on reaching parity,
    /// since from there no vote can go against them; the villagers win
    /// when the last werewolf is gone.
    pub fn winner(&self) -> Option<Team> {
        let werewolves = self.living_on(Team::Werewolves).len();
        let villagers = self.living_on(Team::Villagers).len();
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }
}

/// Where the game is: which round, and which half of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    /// Rounds are counted from one.
    pub round: NonZero<u8>,
    /// Which half of the round is being played.
    pub phase: Phase,
}

impl Turn {
    /// The turn a game begins on. The paper's rounds begin with the day.
    pub const FIRST: Turn = Turn {
        round: NonZero::<u8>::MIN,
        phase: Phase::Day,
    };

    /// The turn after this one: night follows day, and the next day
    /// begins the next round.
    pub fn next(self) -> Turn {
        match self.phase {
            Phase::Day => Turn {
                round: self.round,
                phase: Phase::Night,
            },
            Phase::Night => Turn {
                round: self
                    .round
                    .checked_add(1)
                    .expect("a game loses a player a day and cannot last this long"),
                phase: Phase::Day,
            },
        }
    }
}

/// The two halves of a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    /// The werewolves choose a villager to kill.
    Night,
    /// Everyone votes on whom to eliminate.
    Day,
}

/// How a player is known in the game. The same string names the player's
/// actor.
pub type PlayerId = String;

/// Everything said between the moderator and the players.
///
/// The moderator asks; the players answer. A question carries the choices
/// it allows, so a player never has to be told anything else about the
/// state of the game. What the moderator tells players without asking
/// anything, they simply acknowledge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// Dealt to each player at the start. The werewolves are named only
    /// to the werewolves.
    YouAre {
        /// The player's own role.
        role: Role,
        /// Everyone in the game.
        players: Vec<PlayerId>,
        /// The werewolves, for a werewolf; empty for anyone else.
        werewolves: Vec<PlayerId>,
    },
    /// Asked of one living player by day: their turn to speak and, if they
    /// like, to nominate someone to eliminate.
    Turn {
        /// Everyone still in the game.
        candidates: Vec<PlayerId>,
    },
    /// A player's answer to their turn: what they say to the village, and
    /// whom they nominate, either of which they may leave out.
    Statement {
        /// What they say, for everyone to hear.
        said: Option<String>,
        /// Whom they nominate to eliminate, replacing any earlier
        /// nomination of theirs.
        nominated: Option<PlayerId>,
    },
    /// Told to the rest of the village when a player has taken a turn.
    Heard {
        /// Who spoke.
        from: PlayerId,
        /// What they said, if anything.
        said: Option<String>,
        /// Whom they nominated, if anyone.
        nominated: Option<PlayerId>,
    },
    /// Asked of every living werewolf by night.
    Kill {
        /// Every villager still in the game.
        candidates: Vec<PlayerId>,
    },
    /// A werewolf's answer to a kill.
    Choice(PlayerId),
    /// Told to everyone alive when a player dies, the player included.
    Died {
        /// Who died.
        player: PlayerId,
        /// By day, eliminated by the village; by night, killed by the
        /// werewolves.
        phase: Phase,
    },
    /// Told to everyone at the end.
    GameOver {
        /// Who won.
        winner: Team,
    },
}

/// Everyone tied for the most votes, in a fixed order. No votes means
/// nobody.
pub fn leaders(votes: &[PlayerId]) -> Vec<PlayerId> {
    let mut tally: HashMap<&PlayerId, usize> = HashMap::new();
    for vote in votes {
        *tally.entry(vote).or_default() += 1;
    }
    let Some(most) = tally.values().max().copied() else {
        return vec![];
    };
    let mut leaders: Vec<_> = tally
        .into_iter()
        .filter(|(_, count)| *count == most)
        .map(|(id, _)| id.clone())
        .collect();
    leaders.sort();
    leaders
}

/// Whoever got the most votes, with a tie broken uniformly at random
/// among those tied. No votes choose nobody.
pub fn plurality(votes: &[PlayerId], rng: &mut impl Rng) -> Option<PlayerId> {
    leaders(votes).choose(rng).cloned()
}

/// How a game ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Who won.
    pub winner: Team,
    /// The round the game ended in.
    pub rounds: NonZero<u8>,
}

/// How long a game may take altogether. Random players answer at once, so
/// this only guards against a game that has gone wrong; a game of models
/// is bounded by its own timing instead.
const TIME_LIMIT: Duration = Duration::from_secs(60 * 60);

/// Play one game as configured. The same seed plays the same game, as
/// far as the players' choices are deterministic. With a log, everything
/// said in the game is recorded on it.
///
/// # Errors
///
/// Fails if the configuration cannot be played, if a model's key is
/// missing, if any actor fails, or if the game does not end within its
/// time limit.
pub async fn play(config: &Config, seed: u64, log: Option<Log<Message>>) -> Result<Outcome> {
    config.check()?;
    let names = config.player_names();
    let (report, outcome) = oneshot::channel();
    let settings = Settings {
        day: config.timing.day(),
        patience: config.timing.patience(),
    };
    let moderator = Moderator::new(names.clone(), config.werewolves, seed, settings, report);

    // The key is read once, up front, so a missing one fails before any
    // actor runs rather than inside one.
    let model = match &config.policy {
        PolicyConfig::Random => None,
        PolicyConfig::Llm(llm) => Some((llm.clone(), llm.api_key()?, reqwest::Client::new())),
    };
    let mut actors: Vec<(ActorId, Box<dyn Policy<Message = Message> + Send>)> =
        vec![(moderator::NAME.to_string(), Box::new(moderator))];
    for (n, name) in names.into_iter().enumerate() {
        let player: Box<dyn Policy<Message = Message> + Send> = match &model {
            None => Box::new(UniformRandom::new(seed.wrapping_add(n as u64 + 1))),
            Some((llm, api_key, client)) => Box::new(Llm::new(
                llm.clone(),
                api_key.clone(),
                config.prompts.clone(),
                client.clone(),
            )),
        };
        actors.push((name, player));
    }

    let ending = episode(actors, None, TIME_LIMIT, log).await?;
    ensure!(ending == Ending::Finished, "the game ran out of time");
    Ok(outcome.await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn village() -> State {
        State::new(HashMap::from([
            ("wolf".to_string(), Role::Werewolf),
            ("ann".to_string(), Role::Villager),
            ("bob".to_string(), Role::Villager),
            ("cat".to_string(), Role::Villager),
        ]))
    }

    #[test]
    fn a_new_state_is_on_the_first_turn_with_everyone_alive() {
        let state = village();

        assert_eq!(state.turn, Turn::FIRST);
        assert_eq!(state.living(), vec!["ann", "bob", "cat", "wolf"]);
    }

    #[test]
    fn the_living_can_be_narrowed_to_a_team() {
        let state = village();

        assert_eq!(state.living_on(Team::Werewolves), vec!["wolf"]);
        assert_eq!(state.living_on(Team::Villagers), vec!["ann", "bob", "cat"]);
    }

    #[test]
    fn killing_a_player_removes_them_from_the_living() {
        let mut state = village();

        state.kill(&"bob".to_string());

        assert!(!state.is_alive(&"bob".to_string()));
        assert_eq!(state.living(), vec!["ann", "cat", "wolf"]);
    }

    #[test]
    fn nobody_has_won_at_the_start() {
        assert_eq!(village().winner(), None);
    }

    #[test]
    fn the_villagers_win_when_the_last_werewolf_dies() {
        let mut state = village();

        state.kill(&"wolf".to_string());

        assert_eq!(state.winner(), Some(Team::Villagers));
    }

    #[test]
    fn the_werewolves_win_on_reaching_parity() {
        let mut state = village();
        state.kill(&"ann".to_string());
        assert_eq!(state.winner(), None);

        state.kill(&"bob".to_string());

        assert_eq!(state.winner(), Some(Team::Werewolves));
    }

    #[test]
    fn a_round_is_a_day_then_a_night() {
        let day = Turn::FIRST;
        assert_eq!(day.phase, Phase::Day);

        let night = day.next();
        assert_eq!(
            night,
            Turn {
                round: day.round,
                phase: Phase::Night
            }
        );

        let next_day = night.next();
        assert_eq!(next_day.round.get(), day.round.get() + 1);
        assert_eq!(next_day.phase, Phase::Day);
    }

    #[test]
    fn werewolves_hunt_with_the_werewolves_and_everyone_else_is_a_villager() {
        assert_eq!(Role::Werewolf.team(), Team::Werewolves);
        assert_eq!(Role::Villager.team(), Team::Villagers);
        assert_eq!(Role::Doctor.team(), Team::Villagers);
        assert_eq!(Role::Seer.team(), Team::Villagers);
    }

    fn votes(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_plurality_is_whoever_got_the_most_votes() {
        let mut rng = StdRng::seed_from_u64(0);

        let chosen = plurality(&votes(&["ann", "bob", "ann"]), &mut rng);

        assert_eq!(chosen, Some("ann".to_string()));
    }

    #[test]
    fn a_tie_is_broken_among_those_tied() {
        let tied = votes(&["ann", "bob", "cat", "bob", "ann"]);

        for seed in 0..20 {
            let chosen = plurality(&tied, &mut StdRng::seed_from_u64(seed)).unwrap();
            assert!(chosen == "ann" || chosen == "bob", "{chosen}");
        }
    }

    #[test]
    fn leaders_are_everyone_tied_for_the_most_votes() {
        assert_eq!(
            leaders(&votes(&["ann", "bob", "cat", "bob", "ann"])),
            vec!["ann", "bob"]
        );
        assert_eq!(leaders(&votes(&["ann"])), vec!["ann"]);
        assert!(leaders(&[]).is_empty());
    }

    #[test]
    fn no_votes_choose_nobody() {
        assert_eq!(plurality(&[], &mut StdRng::seed_from_u64(0)), None);
    }

    #[tokio::test]
    async fn a_game_with_no_werewolves_is_won_before_it_starts() {
        let outcome = play(&Config::random(5, 0), 1, None).await.unwrap();

        assert_eq!(outcome.winner, Team::Villagers);
        assert_eq!(outcome.rounds.get(), 1);
    }

    #[tokio::test]
    async fn a_game_that_starts_at_parity_is_lost_before_it_starts() {
        let outcome = play(&Config::random(4, 2), 1, None).await.unwrap();

        assert_eq!(outcome.winner, Team::Werewolves);
        assert_eq!(outcome.rounds.get(), 1);
    }

    #[tokio::test]
    async fn every_game_ends_within_as_many_rounds_as_there_are_players() {
        for seed in 0..10 {
            let outcome = play(&Config::random(7, 2), seed, None).await.unwrap();
            assert!(outcome.rounds.get() <= 7, "seed {seed}: {outcome:?}");
        }
    }

    #[tokio::test]
    async fn the_same_seed_plays_the_same_game() {
        let first = play(&Config::random(9, 3), 42, None).await.unwrap();
        let again = play(&Config::random(9, 3), 42, None).await.unwrap();

        assert_eq!(first, again);
    }

    #[tokio::test]
    async fn there_cannot_be_more_werewolves_than_players() {
        assert!(play(&Config::random(3, 4), 0, None).await.is_err());
    }
}
