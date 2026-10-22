//! The menus: **New Game** in File, beside the menus
//! [`free_agent_desktop::menu`] makes, which open Settings and on macOS
//! put `twenty-questions` on the PATH.

use free_agent_desktop::menu;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Wry};

/// The command line the app carries, which **Install Command-Line Tool…**
/// puts on the PATH.
const COMMAND: &str = "twenty-questions";

/// The app's menu bar.
///
/// # Errors
///
/// When a menu cannot be made.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let new_game = MenuItem::with_id(app, "new_game", "New Game", true, Some("CmdOrCtrl+N"))?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &new_game,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    // The shared menus enable an Open and a Save item as the main window
    // asks. Twenty Questions opens and saves no log yet, so they are made
    // and never shown.
    let open = MenuItem::with_id(app, "open_log", "Open Game Log…", false, None::<&str>)?;
    let save = MenuItem::with_id(app, "save_log", "Save Game Log…", false, None::<&str>)?;
    menu::build(app, &file, &open, &save, COMMAND)
}

/// Do what the menu item of `event` says.
pub fn chosen(app: &AppHandle, event: MenuEvent) {
    if menu::chosen(app, &event) {
        return;
    }
    if event.id().as_ref() == "new_game"
        && let Err(error) = app.emit_to("main", "new-game", ())
    {
        tracing::warn!("New Game could not be announced: {error}");
    }
}
