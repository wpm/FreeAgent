//! The desktop shell every Free Agent app shares, around its own main
//! window and its Settings window. API keys are stored in the OS keychain
//! through [`secret`]; the Models tab's providers and listings, and the
//! checks run on their models in the background from launch, are in
//! [`catalog`]; [`settings`] keeps the app's settings in a file and opens
//! the Settings window; [`dialog`] tells of failures and saves files;
//! [`review`] opens a game log for the main window to review; the
//! [`menu`] holds File, Settings, Edit, Go and Window, and on macOS puts
//! the command line on the PATH, as `command_line` does; on launch,
//! [`update`] checks for an update and, when the user agrees, installs it
//! and restarts; and on macOS a debug build runs inside an app bundle, as
//! `dev_bundle` makes it. On Windows, each app's installer puts its
//! command line on the user's Path with this crate's `user-path.ps1`.
//!
//! What names the app, such as its keychain service, its probe, its
//! settings, its command line and what its File menu does, the app passes
//! in. Its Tauri commands are its own, each a thin wrapper around a
//! function here, so that `generate_handler!` lists them where they are
//! registered.

#![warn(missing_docs)]

pub mod catalog;
#[cfg(target_os = "macos")]
pub(crate) mod command_line;
#[cfg(all(target_os = "macos", debug_assertions))]
pub mod dev_bundle;
pub mod dialog;
pub mod menu;
pub mod review;
pub mod secret;
pub mod settings;
pub mod update;
