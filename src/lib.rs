//! Small agents that perceive, act, and talk to each other.
//!
//! The crate-level documentation is the project README, included below so the
//! two cannot drift apart and so its example is compiled with the doctests.

#![warn(missing_docs)]
// The README's example is compiled and run with the other doctests, so the
// first thing a reader sees cannot quietly rot.
#![doc = include_str!("../README.md")]

pub mod actor;
pub mod episode;
