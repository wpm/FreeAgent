//! The viewer for social deduction games: one game in God mode, as a group
//! chat beside a live board, drawn from the game's cues.
//!
//! [`screen`] is what the screen shows, a pure function of the cues so
//! far; [`view`] draws it in the browser.

#![warn(missing_docs)]

pub mod screen;
pub mod view;
