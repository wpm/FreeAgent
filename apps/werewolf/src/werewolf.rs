use sealed::Sealed;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

#[derive(Debug, Serialize, Deserialize)]
struct State<P: PhaseType> {
    phase: P,
    round: NonZero<u8>,
    roles: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

impl<P: PhaseType> State<P> {
    pub fn winner(&self) -> Option<Team> {
        let werewolves = self.surviving(Team::Werewolves);
        let villagers = self.surviving(Team::Villagers);
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }

    fn surviving(&self, team: Team) -> usize {
        self.alive
            .iter()
            .filter(|player_id| {
                let role = self.roles[*player_id];
                match team {
                    Team::Werewolves => role == Role::Werewolf,
                    Team::Villagers => role != Role::Werewolf,
                }
            })
            .count()
    }
}

impl State<Night> {
    fn new(roles: HashMap<PlayerId, Role>) -> Self {
        let alive = roles.keys().cloned().collect();
        Self {
            phase: Night,
            round: NonZero::<u8>::MIN,
            roles,
            alive,
        }
    }
}

pub type PlayerId = String;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
pub enum Role {
    Werewolf,
    Villager,
    Doctor,
    Seer,
}

#[derive(Debug, Serialize)]
pub enum Team {
    Werewolves,
    Villagers,
}

#[derive(Debug, Serialize, Deserialize)]
enum Phase {
    Night,
    Day,
}

pub struct Night;

pub struct Day;

trait PhaseType: Sealed {
    type Next;
    const PHASE: Phase;
}

impl Sealed for Night {}

impl Sealed for Day {}

impl PhaseType for Night {
    type Next = Option<State<Day>>;
    const PHASE: Phase = Phase::Night;
}

impl PhaseType for Day {
    type Next = Option<State<Night>>;
    const PHASE: Phase = Phase::Day;
}

pub mod sealed {
    pub trait Sealed {}
}
