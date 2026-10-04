//! Episodes in which actors talk to each other by request and reply.
//!
//! The crate-level documentation is the project README, included below so the
//! two cannot drift apart and so its example is compiled with the doctests.

#![warn(missing_docs)]
// The README's example is compiled and run with the other doctests, so the
// first thing a reader sees cannot quietly rot.
#![doc = include_str!("../README.md")]

mod framework;

pub use framework::*;
