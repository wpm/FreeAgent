#![allow(dead_code)]

pub mod config;
mod variant;

use crate::sealed::Sealed;
use serde::{Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
enum Role {
    Werewolf,
    Villager,
    Doctor,
    Seer,
}

#[derive(Debug, Serialize, Deserialize)]
struct State<P: Phase> {
    phase: P,
    round: NonZero<u8>,
    roles: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

impl<P: Phase> State<P> {
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

    // pub fn run<R: Rules>(mut self, rules: &mut R) -> <Night as Phase>::Next {
    //     let wolves = self.surviving_player_roles(&[Role::Werewolf]);
    //     rules.werewolves_at_night(&mut self, &wolves);
    //     if let Some(seer) = self.player_with(Role::Seer) {
    //         rules.seer_at_night(&mut self, &seer);
    //     }
    //     self.dawn_or_end() // the win check stays in your crate
    // }
}
type PlayerId = String;

#[derive(Debug, Serialize)]
enum Team {
    Werewolves,
    Villagers,
}

#[derive(Debug, Serialize, Deserialize)]
enum PhaseName {
    Night,
    Day,
}

struct Night;

struct Day;

trait Phase: Sealed {
    type Next;
    const NAME: PhaseName;
}

impl Sealed for Night {}

impl Sealed for Day {}
impl Phase for Night {
    type Next = Option<State<Day>>;
    const NAME: PhaseName = PhaseName::Night;
}

impl Phase for Day {
    type Next = Option<State<Night>>;
    const NAME: PhaseName = PhaseName::Day;
}

mod sealed {
    pub trait Sealed {}
}
