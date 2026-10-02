//! What the environment and the players say to each other.
//!
//! Every exchange is the same shape. The environment hands a player the
//! [`Observation`] it is entitled to and the [`ActionSpace`] it may draw
//! from; the player answers with an [`Action`], naming one living player.
//! That is the whole action space for now — speech will join it later,
//! which is why these are types rather than bare ids.

use crate::game::{Outcome, Phase};
use crate::role::{Role, Team};
use free_agent::actor::ActorId;
use serde::{Deserialize, Serialize};

/// Everything said in a game.
///
/// One message type per episode is what free-agent asks for, so this is the
/// union of what the environment sends and what the players answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// The environment tells a player who they are, at the start.
    Assigned(Assignment),
    /// The environment asks a player to act: what it can see, and what it
    /// may do about it. An [`Action`] is expected back.
    Decide(Observation, ActionSpace),
    /// A player's chosen action.
    Act(Action),
    /// The environment tells a player what came of the night.
    Learned(Knowledge),
    /// The environment tells everyone the game is over.
    Ended(Outcome),
}

/// What a player is told at the start: their own role, and anything the role
/// entitles them to know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    /// What this player is.
    pub role: Role,
    /// Everyone in the game, in seating order.
    pub players: Vec<ActorId>,
    /// The other werewolves, for a werewolf. Empty for anyone else: the
    /// environment does not tell a villager who it may trust.
    pub allies: Vec<ActorId>,
}

/// What a player can see of the world when it is asked to act.
///
/// The observation in the reinforcement-learning sense: everything the
/// environment is willing to tell this player about the state, and nothing
/// it is not. What the player additionally remembers from earlier turns is
/// its own [`State`](crate::player::State).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Which round this is, counting from one.
    pub round: u32,
    /// Night or day.
    pub phase: Phase,
    /// Who is still alive, including this player.
    pub living: Vec<ActorId>,
}

/// Everything a player may legally do this turn.
///
/// Every action in this game is naming one living player, so an action
/// space is the set of players that may be named. A policy must return an
/// action drawn from here; the environment refuses anything else.
///
/// Speech will join the action space later, which is why this is a type
/// rather than a bare `Vec<ActorId>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpace {
    /// Who this player may name. Always a subset of the living.
    pub targets: Vec<ActorId>,
}

impl ActionSpace {
    /// The space of naming any one of `targets`.
    pub fn naming(targets: impl IntoIterator<Item = ActorId>) -> Self {
        Self {
            targets: targets.into_iter().collect(),
        }
    }

    /// Whether `action` is one this space allows.
    pub fn allows(&self, action: &Action) -> bool {
        self.targets.contains(&action.target)
    }

    /// Whether there is nothing to choose from.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// How many actions this space holds.
    pub fn len(&self) -> usize {
        self.targets.len()
    }
}

/// What a player does: name one player.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Who the player names.
    pub target: ActorId,
}

impl Action {
    /// Names `target`.
    pub fn on(target: impl Into<ActorId>) -> Self {
        Self {
            target: target.into(),
        }
    }
}

/// Something a player learns privately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Knowledge {
    /// What the seer read.
    Reading {
        /// Who was read.
        player: ActorId,
        /// Which side they are on.
        team: Team,
    },
    /// Who died in the night, or that nobody did.
    Died(Option<ActorId>),
    /// Who the village lynched, or that it could not agree.
    Lynched(Option<ActorId>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_survives_a_round_trip() {
        let message = Message::Decide(
            Observation {
                round: 1,
                phase: Phase::Night,
                living: vec!["Wolf".to_string(), "Doc".to_string()],
            },
            ActionSpace::naming(["Doc".to_string()]),
        );
        let json = serde_json::to_string(&message).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn an_action_names_one_player() {
        assert_eq!(Action::on("Doc").target, "Doc");
    }

    #[test]
    fn an_action_space_knows_what_it_allows() {
        let space = ActionSpace::naming(["Doc".to_string(), "Seer".to_string()]);
        assert!(space.allows(&Action::on("Doc")));
        assert!(!space.allows(&Action::on("Wolf")));
        assert_eq!(space.len(), 2);
        assert!(!space.is_empty());
        assert!(ActionSpace::naming([]).is_empty());
    }

    /// The log is for training, so a reading must say both who and what.
    #[test]
    fn a_reading_carries_the_player_and_the_team() {
        let knowledge = Knowledge::Reading {
            player: "Wolf".to_string(),
            team: Team::Werewolves,
        };
        let json = serde_json::to_value(&knowledge).unwrap();
        assert_eq!(json["Reading"]["player"], "Wolf");
        assert_eq!(json["Reading"]["team"], "Werewolves");
    }
}
