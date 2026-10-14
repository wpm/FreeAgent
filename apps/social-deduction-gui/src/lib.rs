//! The viewer for social deduction games: one game in God mode, as a group
//! chat beside a live board, drawn from the game's cues.
//!
//! [`screen`] is what the screen shows, a pure function of the cues so
//! far; [`view`] draws it. In the desktop app, [`tauri`] reaches the
//! backend that plays a game live, and [`settings`] picks the game.

#![warn(missing_docs)]

mod desktop;
pub mod screen;
pub mod settings;
pub mod tauri;
pub mod view;
