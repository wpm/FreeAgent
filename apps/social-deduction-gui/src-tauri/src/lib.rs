//! The desktop shell around the viewer: a main window that plays a game
//! live, and a Settings window. The app owns its settings, in [`settings`];
//! the Models tab's providers and listings, and the checks run on their
//! models in the background from launch, are in [`catalog`]; API keys are
//! stored through [`secret`]; and [`play`](game::play) runs the
//! chosen game as `social-deduction play` runs it, its log folded into
//! cues sent to the main window. The [`menu`] opens Settings,
//! starts a new game, imports and exports configuration files, and on
//! macOS puts the command line on the PATH.
//! Closing the main window stops the game, whether it is being prepared or
//! playing, and closes Settings. The opener plugin hands links to the
//! system browser, and the dialog plugin picks files and asks before an
//! import.

#![warn(missing_docs)]

pub mod catalog;
#[cfg(target_os = "macos")]
pub mod command_line;
#[cfg(all(target_os = "macos", debug_assertions))]
mod dev_bundle;
pub mod game;
pub mod menu;
pub mod secret;
pub mod settings;

use social_deduction::live::Session;
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
        .plugin(tauri_plugin_dialog::init())
        .manage(Session::default())
        .setup(|app| {
            let handle = app.handle();
            app.manage(settings::Store::open(handle)?);
            app.manage(catalog::Catalog::open(handle)?);
            catalog::check_in_background(handle);
            app.set_menu(menu::build(handle)?)?;
            Ok(())
        })
        .on_menu_event(menu::chosen)
        .invoke_handler(tauri::generate_handler![
            game::play,
            game::stop,
            settings::settings_get,
            settings::settings_set,
            settings::settings_open,
            settings::game_defaults,
            catalog::providers,
            catalog::listing,
            catalog::checking,
            secret::key_store,
            secret::key_remove,
            secret::key_status
        ])
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let WindowEvent::CloseRequested { .. } = event
            {
                window.state::<Session>().stop();
                if let Some(settings) = window.app_handle().get_webview_window(settings::WINDOW) {
                    let _ = settings.close();
                }
            }
        })
        .run(context)
        .expect("the Tauri application runs");
}
