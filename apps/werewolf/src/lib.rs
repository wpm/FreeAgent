#![allow(dead_code)]

pub mod config;
mod variant;

use crate::sealed::Sealed;
use crate::variant::Rules;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

#[derive(Debug, Serialize, Deserialize)]
struct State<P: Phase> {
    phase: P,
    round: NonZero<u8>,
    werewolves: HashMap<PlayerId, bool>,
    villagers: HashMap<PlayerId, bool>,
    doctor: (PlayerId, bool),
    seer: (PlayerId, bool),
    known_roles: HashMap<PlayerId, Role>,
}

struct Observation {
    phase: ObservedPhase,
    round: NonZero<u8>,
    alive: HashSet<PlayerId, bool>,
    known_roles: HashMap<PlayerId, Role>,
}

#[derive(Debug, Serialize, Deserialize)]
enum Role {
    Werewolf,
    Villager,
    Doctor,
    Seer,
}

#[derive(Debug, Serialize, Deserialize)]
enum ObservedPhase {
    Night,
    Day,
}

impl<P: Phase> State<P> {
    fn observation(&self, player_id: PlayerId) -> Observation {
        Observation {
            phase: self.phase.phase(),
            round: self.round,
            alive: alive,
            known_roles: self.known_roles,
        }
    }
    pub fn winner(&self) -> Option<Team> {
        fn alive(players: &HashMap<PlayerId, bool>) -> usize {
            players.values().filter(|alive| **alive).count()
        }
        let werewolves = alive(&self.werewolves);
        let mut villagers = alive(&self.villagers);
        if self.seer.1 {
            villagers += 1
        }
        if self.doctor.1 {
            villagers += 1
        }
        if werewolves == 0 {
            Some(Team::Villagers)
        } else if werewolves >= villagers {
            Some(Team::Werewolves)
        } else {
            None
        }
    }
}

impl State<Night> {
    fn new(
        werewolves: HashSet<PlayerId>,
        villagers: HashSet<PlayerId>,
        doctor: PlayerId,
        seer: PlayerId,
    ) -> Self {
        fn all_alive(player_ids: HashSet<PlayerId>) -> HashMap<PlayerId, bool> {
            player_ids
                .into_iter()
                .map(|player_id| (player_id, true))
                .collect()
        }
        let mut known_roles: HashMap<PlayerId, Role> = werewolves
            .iter()
            .map(|player_id| (player_id.clone(), Role::Werewolf))
            .collect();
        known_roles.extend(
            villagers
                .iter()
                .map(|player_id| (player_id.clone(), Role::Villager)),
        );
        known_roles.insert(doctor.clone(), Role::Doctor);
        known_roles.insert(seer.clone(), Role::Seer);
        Self {
            phase: Night,
            round: NonZero::new(1).unwrap(),
            werewolves: all_alive(werewolves),
            villagers: all_alive(villagers),
            doctor: (doctor, true),
            seer: (seer, true),
            known_roles,
        }
    }

    pub fn run<R: Rules>(mut self, rules: &mut R) -> <Night as Phase>::Next {
        let wolves = self.surviving_player_roles(&[Role::Werewolf]);
        rules.werewolves_at_night(&mut self, &wolves);
        if let Some(seer) = self.player_with(Role::Seer) {
            rules.seer_at_night(&mut self, &seer);
        }
        self.dawn_or_end() // the win check stays in your crate
    }
}
type PlayerId = String;

#[derive(Debug, Serialize)]
enum Team {
    Werewolves,
    Villagers,
}

struct Night;

struct Day;

trait Phase: Sealed {
    type Next;

    fn phase(&self) -> ObservedPhase {
        ObservedPhase::Day
    }
}

impl Sealed for Night {}

impl Sealed for Day {}
impl Phase for Night {
    type Next = Option<State<Day>>;

    fn phase(&self) -> ObservedPhase {
        ObservedPhase::Night
    }
}

impl Phase for Day {
    type Next = Option<State<Night>>;

    fn phase(&self) -> ObservedPhase {
        ObservedPhase::Day
    }
}

mod sealed {
    pub trait Sealed {}
}
