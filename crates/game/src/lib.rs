//! What every Free Agent game shares, whatever it plays: its seats and
//! whom its players send to, the strategies its players are dealt, its
//! example configurations and their format, what its log never carries,
//! and the lines its story is told in. Every game is played as episodes of
//! [free agents](free_agent).
//!
//! Helpers for an app's tests, such as a player named by a test, come with
//! the `test-support` feature.

#![warn(missing_docs)]

pub mod config;
pub mod examples;
pub mod log;
pub mod report;
pub mod seat;
pub mod strategy;
