//! Twenty Questions' page: one game in God mode, as a group chat under
//! the secret object and the guesses used, drawn from the game's cues.
//!
//! [`screen`] is what the screen shows, a pure function of the cues so
//! far; [`view`] draws it. In the desktop app, [`tauri`] reaches the
//! backend that plays a game live, and [`settings`] edits its
//! configuration. The look and the chrome are Free Agent's, from
//! [`free_agent_gui`].

#![warn(missing_docs)]

mod desktop;
mod mode;
pub mod screen;
pub mod settings;
pub mod tauri;
pub mod view;
