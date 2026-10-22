//! Twenty Questions, played as episodes of [free agents](free_agent): a
//! host holds a secret object, and model players talk among themselves and
//! put guesses to it until one names the object, the guesses run out, or
//! the time does.

#![warn(missing_docs)]

pub mod config;
pub mod examples;
pub mod game;
pub mod host;
pub mod log;
pub mod message;
pub mod model;
pub mod narrator;
pub mod object;
pub mod player;
pub mod run;
pub mod story;
