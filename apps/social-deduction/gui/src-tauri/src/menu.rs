//! The menus, as [`free_agent_desktop::menu`] makes them, with File's
//! items doing what this app does, and on macOS an item that puts
//! `social-deduction` on the PATH.

use crate::{game, review, settings};
use free_agent_desktop::menu::{self, File, Items};
use tauri::menu::{Menu, MenuEvent};
use tauri::{AppHandle, State, Wry};

/// The command line the app carries, which **Install Command-Line Tool…**
/// puts on the PATH.
const COMMAND: &str = "social-deduction";

/// What File's items do.
const FILE: File = File {
    open: review::open,
    save: game::save_log,
    import: settings::import,
    export: settings::export,
};

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
    menu::build(app, COMMAND)
}

/// Do what the menu item of `event` says.
pub fn chosen(app: &AppHandle, event: MenuEvent) {
    menu::chosen(app, &event, &FILE);
}
