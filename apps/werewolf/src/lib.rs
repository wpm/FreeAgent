#![allow(dead_code)]

pub mod config;

use crate::sealed::Sealed;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

pub type PlayerId = String;

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

#[derive(Debug, Serialize)]
pub enum Team {
    Werewooves,
    Villagers,
}

#[derive(Debug)]
struct State<P: Phase> {
    round: NonZero<u8>,
    phase: P,
    role: HashMap<PlayerId, Role>,
    alive: HashSet<PlayerId>,
}

mod sealed {
    pub trait Sealed {}
}

trait Phase: Sealed {
    type Next;
}
struct Night;
impl State<Night> {
    fn new(role: HashMap<PlayerId, Role>) -> Self {
        let alive = role.keys().cloned().collect();
        Self {
            round: NonZero::new(1).unwrap(),
            phase: Night,
            role,
            alive,
        }
    }
}

struct Day;

impl Sealed for Night {}

impl Phase for Night {
    type Next = Option<State<Day>>;
}

impl Sealed for Day {}

impl Phase for Day {
    type Next = Option<State<Night>>;
}

trait Run<P: Phase> {
    fn run(&mut self, state: State<P>) -> P::Next;
}
