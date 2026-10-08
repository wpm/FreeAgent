//! Episodes in which actors talk to each other by broadcast, request, and
//! reply.
//!
//! The crate-level documentation is the project README, included below so
//! the two stay one document.

#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod actor;
mod episode;
mod log;
mod message;

pub use actor::{ActorId, ActorInit, Builder, Context, Lifecycle, Strategy};
pub use episode::Episode;
pub use log::{Event, Logger, console_log, drain};
pub use message::Message;
