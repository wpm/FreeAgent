//! The look every Free Agent app's web frontend shares, the Free Agent
//! site's: black ink on white paper, Georgia for what is read, Helvetica
//! Neue for controls, and marigold for a problem.
//!
//! [`style`] puts the shared stylesheet on the page, and [`theme`] holds
//! it light or dark. The main window is built from the [`chrome`], a
//! [`splitter`] beside a pane, a [`clock`] counting down, and a
//! [`review`] header that steps through a game log. In the desktop app,
//! the main window's [`mode`] says what it is doing, [`tauri`] reaches the
//! backend, [`tip`] shows each element's help, and the [`settings`] window
//! edits the app's settings and its models.

#![warn(missing_docs)]

pub mod chrome;
pub mod clock;
pub mod mode;
pub mod review;
pub mod settings;
pub mod splitter;
pub mod style;
pub mod tauri;
pub mod theme;
pub mod tip;
