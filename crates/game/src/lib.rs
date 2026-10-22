//! What every Free Agent game shares, whatever it plays: its seats and
//! their order, the prompted strategy, its example configurations and
//! their format, what its log never carries, and the lines its story is
//! told in. Every game is played as episodes of [free agents](free_agent).
//!
//! An app's build script embeds its example configurations with [`embed`],
//! which builds without the default feature `games`, and so without the
//! rest of the crate or its dependencies.
//!
//! Helpers for an app's tests, such as the models a story's test has in
//! play, come with the `test-support` feature.

#![warn(missing_docs)]

#[cfg(feature = "games")]
pub mod config;
pub mod embed;
#[cfg(feature = "games")]
pub mod examples;
#[cfg(feature = "games")]
pub mod log;
#[cfg(feature = "games")]
pub mod report;
#[cfg(feature = "games")]
pub mod seat;
#[cfg(feature = "games")]
pub mod strategy;
