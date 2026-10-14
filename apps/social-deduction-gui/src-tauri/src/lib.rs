//! The desktop shell around the viewer: a window that plays a game live.
//! The frontend picks a preset and calls [`play`](game::play); the game
//! runs here as `social-deduction play` runs it, its log saved to the app's
//! data directory and folded into cues sent to the frontend. Closing the
//! window stops the game. The opener plugin hands links to the system
//! browser and reveals a game's log.

#![warn(missing_docs)]

#[cfg(all(target_os = "macos", debug_assertions))]
mod dev_bundle;
pub mod game;
pub mod secret;

use tauri::{Manager, WindowEvent};

/// Open the window and run the app until it closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    #[cfg(all(target_os = "macos", debug_assertions))]
    dev_bundle::relaunch_in_bundle(&dev_bundle::App {
        name: &context.package_info().name,
        version: &context.package_info().version.to_string(),
        identifier: &context.config().identifier,
    });
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(game::Game::default())
        .invoke_handler(tauri::generate_handler![
            game::presets,
            game::play,
            game::stop,
            game::show_log
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                window.state::<game::Game>().stop();
            }
        })
        .run(context)
        .expect("the Tauri application runs");
}
