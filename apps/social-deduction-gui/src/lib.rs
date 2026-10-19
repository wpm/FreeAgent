//! The viewer for social deduction games: one game in God mode, as a group
//! chat beside a live board, drawn from the game's cues.
//!
//! [`screen`] is what the screen shows, a pure function of the cues so
//! far; [`view`] draws it, for the game the URL's [`address`] names, and
//! [`review`] steps through a game log bubble by bubble. In
//! the desktop app, [`tauri`] reaches the backend that plays a game live,
//! and [`settings`] edits its configuration, with a [`tip`] for help.

#![warn(missing_docs)]

pub mod address;
mod desktop;
pub mod review;
pub mod screen;
pub mod settings;
pub mod splitter;
pub mod tauri;
pub mod tip;
pub mod view;
