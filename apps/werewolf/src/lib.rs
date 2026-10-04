use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZero;

pub mod config;
pub mod game;
pub mod moderator;
mod variation;
mod rounds;

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

/// The two sides. A game ends when one of them has won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// Kill by night and win on reaching parity.
    Werewolves,
    /// Vote by day and win when the last werewolf is gone.
    Villagers,
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

/// How a game ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Who won.
    pub winner: Team,
    /// The round the game ended in.
    pub rounds: NonZero<u8>,
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let living = state.living();
        assert!(!living.contains(&"bob".to_string()));
        assert_eq!(living, vec!["ann", "cat", "wolf"]);
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
}
