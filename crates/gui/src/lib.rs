//! The look every Free Agent app's web frontend shares, the Free Agent
//! site's: black ink on white paper, Georgia for what is read, Helvetica
//! Neue for controls, and marigold for a problem.
//!
//! [`style`] puts the shared stylesheet on the page, and [`theme`] holds
//! it light or dark. The main window is built from the [`chrome`], a
//! [`splitter`] beside a pane, a [`clock`] counting down, and a
//! [`review`] header that steps through a game log. In the desktop app,
//! [`tauri`] reaches the backend, and [`tip`] shows each element's help.

#![warn(missing_docs)]

pub mod chrome;
pub mod clock;
pub mod review;
pub mod splitter;
pub mod style;
pub mod tauri;
pub mod theme;
pub mod tip;
