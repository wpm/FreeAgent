//! The menus every app has: **Settings…** and **Install Command-Line
//! Tool…** in the app menu on macOS, and **Preferences…** in Edit
//! elsewhere; the **Go** menu, which moves through a log under review; and
//! the standard Edit and Window items, so text fields take the usual
//! shortcuts. The File menu is the app's, holding the **Open** and
//! **Save** items the main window enables.

use free_agent_gui_types::go::{Go, MENU};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// The items the main window enables as what it shows changes.
pub struct Items {
    /// The File menu's **Open** item.
    open: MenuItem<Wry>,
    /// The File menu's **Save** item.
    save: MenuItem<Wry>,
    /// The **Go** menu's items, each with where it goes.
    go: Vec<(MenuItem<Wry>, Go)>,
}

/// Enable **Open** when `open`, **Save** when `save`, and the **Go** menu
/// while the main window reviews a log of `games` games, none outside
/// review, its **Previous Game** and **Next Game** only when there are
/// several.
pub fn enable(items: &Items, open: bool, save: bool, games: usize) {
    let enable = |item: &MenuItem<Wry>, enabled: bool| {
        if let Err(error) = item.set_enabled(enabled) {
            tracing::warn!("a menu item could not be enabled: {error}");
        }
    };
    enable(&items.open, open);
    enable(&items.save, save);
    for (item, go) in &items.go {
        enable(item, games > usize::from(go.between_games()));
    }
}

/// The app's menu bar, with the app's `file` menu, which holds `open` and
/// `save`, and on macOS an item that installs the command line `command`.
/// The items the main window enables are kept in [`Items`].
///
/// # Errors
///
/// When a menu cannot be made.
pub fn build(
    app: &AppHandle,
    file: &Submenu<Wry>,
    open: &MenuItem<Wry>,
    save: &MenuItem<Wry>,
    command: &'static str,
) -> tauri::Result<Menu<Wry>> {
    let separator = || PredefinedMenuItem::separator(app);
    let label = if cfg!(target_os = "macos") {
        "Settings…"
    } else {
        "Preferences…"
    };
    let preferences = MenuItem::with_id(app, "settings", label, true, Some("CmdOrCtrl+,"))?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &separator()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let mut go = Vec::new();
    let go_menu = Submenu::with_id(app, "go", "Go", true)?;
    for (index, group) in MENU.iter().enumerate() {
        if index > 0 {
            go_menu.append(&separator()?)?;
        }
        for item in group {
            let shortcut = item.shortcut(cfg!(target_os = "macos"));
            let made = MenuItem::with_id(
                app,
                format!("go_{:?}", item.go),
                item.label,
                false,
                Some(shortcut),
            )?;
            go_menu.append(&made)?;
            go.push((made, item.go));
        }
    }
    app.manage(Items {
        open: open.clone(),
        save: save.clone(),
        go,
    });
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    bar(app, &preferences, command, file, &edit, &go_menu, &window)
}

/// The menu bar on macOS: an app menu holding `preferences` and an item
/// that installs the command line `command`, then `file`, `edit`, `go` and
/// `window`.
#[cfg(target_os = "macos")]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    command: &'static str,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    go: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    let separator = || PredefinedMenuItem::separator(app);
    let name = app.package_info().name.clone();
    let command_line = crate::command_line::item(app, command)?;
    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &separator()?,
            preferences,
            &command_line.item,
            &separator()?,
            &PredefinedMenuItem::services(app, None)?,
            &separator()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &separator()?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    app.manage(command_line);
    Menu::with_items(app, &[&app_menu, file, edit, go, window])
}

/// The menu bar elsewhere: `file`, with Quit, `edit`, with `preferences`,
/// `go` and `window`. No item installs the command line `_command`.
#[cfg(not(target_os = "macos"))]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    _command: &'static str,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    go: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    edit.append_items(&[&PredefinedMenuItem::separator(app)?, preferences])?;
    file.append_items(&[&PredefinedMenuItem::quit(app, None)?])?;
    Menu::with_items(app, &[file, edit, go, window])
}

/// Do what the menu item of `event` says, if it is one of these menus',
/// and say whether it was. A **Go** item tells the main window where the
/// cursor goes. The command line's dialogs block, so they run on a thread
/// of their own.
pub fn chosen(app: &AppHandle, event: &MenuEvent) -> bool {
    let items = app.state::<Items>();
    let chosen = items.go.iter().find(|(item, _)| item.id() == event.id());
    if let Some(&(_, go)) = chosen {
        if let Err(error) = app.emit_to("main", "go", go) {
            tracing::warn!("Go could not be announced: {error}");
        }
        return true;
    }
    match event.id().as_ref() {
        "settings" => {
            if let Err(error) = crate::settings::show(app, None) {
                tracing::warn!("the Settings window could not be shown: {error}");
            }
        }
        #[cfg(target_os = "macos")]
        crate::command_line::ID => {
            let app = app.clone();
            std::thread::spawn(move || {
                let item = app.state::<crate::command_line::Item>();
                crate::command_line::chosen(&app, &item);
            });
        }
        _ => return false,
    }
    true
}
