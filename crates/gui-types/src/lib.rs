//! The types every Free Agent app's native code shares with its WASM
//! frontend.
//!
//! A frontend builds for `wasm32-unknown-unknown`, so it cannot depend on
//! the native crates, which need Tokio, reqwest and the keychain. A type or
//! helper belongs here when any app's frontend reads, produces or checks it
//! and the native side has to agree with it, whatever the game: the
//! desktop app's [`settings`] that every app has, and the [`go`] menu's
//! moves.

#![warn(missing_docs)]

pub mod go;
pub mod settings;
