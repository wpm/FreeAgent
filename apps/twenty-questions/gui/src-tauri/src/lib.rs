//! The desktop shell around Twenty Questions' page: a main window that
//! plays a game live, and a Settings window, on the shell every Free Agent
//! app shares, [`free_agent_desktop`]. The app owns its settings, in
//! [`settings`]; the Models tab's commands, and the probe its models are
//! checked with, are in [`catalog`]; API keys are stored under its service
//! through [`secret`]; and [`play`](game::play) runs a game as
//! `twenty-questions play` runs it, its log folded into cues sent to the
//! main window. The [`menu`]'s File menu starts a new game, and imports
//! and exports configuration files.
//! Closing the main window stops the game, whether it is being prepared or
//! playing, and closes Settings. The opener plugin hands links to the
//! system browser, and the dialog plugin tells of settings that can't be
//! read. On launch, the app checks for an update and, when the user
//! agrees, stops any game, installs the update and restarts.

#![warn(missing_docs)]

pub mod catalog;
pub mod game;
pub mod menu;
pub mod secret;
pub mod settings;

use free_agent_desktop::catalog::Catalog;
use tauri::{AppHandle, Manager};
use twenty_questions::live::Session;

/// Stop the game being prepared or playing, if there is one.
fn stop(app: &AppHandle) {
    app.state::<Session>().stop();
}

/// Open the window and run the app until it closes.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();
    #[cfg(all(target_os = "macos", debug_assertions))]
    free_agent_desktop::dev_bundle::relaunch_in_bundle(
        &context,
        include_bytes!("../icons/icon.icns"),
    );
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Session::default())
        .setup(|app| {
            let handle = app.handle();
            app.manage(settings::Store::open(handle, settings::file())?);
            app.manage(Catalog::open(handle, catalog::prober())?);
            free_agent_desktop::catalog::check_in_background(handle);
            #[cfg(not(debug_assertions))]
            free_agent_desktop::update::check_in_background(handle, stop);
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
        .on_window_event(|window, event| free_agent_desktop::settings::closed(window, event, stop))
        .run(context)
        .expect("the Tauri application runs");
}
