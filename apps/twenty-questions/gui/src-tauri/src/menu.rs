//! The menus: **New Game**, **Open Game Log…**, **Save Game Log…**,
//! **Import Configuration…** and **Export Configuration…** in File,
//! beside the menus [`free_agent_desktop::menu`] makes, which open
//! Settings, move through a log under review, and on macOS put
//! `twenty-questions` on the PATH.

use crate::{game, review, settings};
use free_agent_desktop::menu::{self, Items};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, State, Wry};

/// The command line the app carries, which **Install Command-Line Tool…**
/// puts on the PATH.
const COMMAND: &str = "twenty-questions";

/// Enable **Open Game Log…** when `open`, **Save Game Log…** when
/// `save`, and the **Go** menu while the main window reviews a log of
/// `games` games, as [`menu::enable`] does.
#[tauri::command]
pub fn menus(items: State<'_, Items>, open: bool, save: bool, games: usize) {
    menu::enable(&items, open, save, games);
}

/// The app's menu bar.
///
/// # Errors
///
/// When a menu cannot be made.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let separator = || PredefinedMenuItem::separator(app);
    let new_game = MenuItem::with_id(app, "new_game", "New Game", true, Some("CmdOrCtrl+N"))?;
    let open = MenuItem::with_id(app, "open_log", "Open Game Log…", true, Some("CmdOrCtrl+O"))?;
    let save = MenuItem::with_id(
        app,
        "save_log",
        "Save Game Log…",
        false,
        Some("CmdOrCtrl+S"),
    )?;
    let import = MenuItem::with_id(app, "import", "Import Configuration…", true, None::<&str>)?;
    let export = MenuItem::with_id(app, "export", "Export Configuration…", true, None::<&str>)?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &new_game,
            &open,
            &save,
            &separator()?,
            &import,
            &export,
            &separator()?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    menu::build(app, &file, &open, &save, COMMAND)
}

/// Do what the menu item of `event` says. A dialog blocks, so Open, Save,
/// Import and Export run on threads of their own.
pub fn chosen(app: &AppHandle, event: MenuEvent) {
    if menu::chosen(app, &event) {
        return;
    }
    match event.id().as_ref() {
        "new_game" => {
            if let Err(error) = app.emit_to("main", "new-game", ()) {
                tracing::warn!("New Game could not be announced: {error}");
            }
        }
        "open_log" => {
            let app = app.clone();
            std::thread::spawn(move || review::open(&app));
        }
        "save_log" => game::save_log(app.clone()),
        "import" => {
            let app = app.clone();
            std::thread::spawn(move || settings::import(&app));
        }
        "export" => {
            let app = app.clone();
            std::thread::spawn(move || settings::export(&app));
        }
        _ => {}
    }
}
