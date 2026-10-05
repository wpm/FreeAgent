#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub mod config;
pub mod environment;
mod variant;

pub type PlayerId = String;

#[derive(Debug, Serialize)]
pub enum Team {
    Werewolves,
    Villagers,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
pub enum Role {
    Werewolf,
    Villager,
    Doctor,
    Seer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
enum Phase {
    Night,
    Day,
}
