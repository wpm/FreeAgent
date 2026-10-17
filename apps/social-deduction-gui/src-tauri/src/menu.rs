//! The menus: **Settings…** in the app menu on macOS, and
//! **Preferences…** in Edit elsewhere; **New Game**, **Open Game Log…**,
//! **Import Configuration…** and **Export Configuration…** in File; the
//! **Go** menu, which moves through a log under review; and the standard
//! Edit and Window items, so text fields take the usual shortcuts.

use crate::{review, settings};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State, Wry};

/// The **Go** menu's items: each one's ID, as the main window's `go`
/// event names where the cursor goes, its label, its shortcut on macOS
/// and elsewhere, and whether it moves between games.
const GO: [(&str, &str, &str, &str, bool); 6] = [
    ("first", "First Bubble", "Cmd+Up", "Ctrl+Home", false),
    ("last", "Last Bubble", "Cmd+Down", "Ctrl+End", false),
    (
        "previous_chapter",
        "Previous Chapter",
        "Alt+Up",
        "Alt+Up",
        false,
    ),
    (
        "next_chapter",
        "Next Chapter",
        "Alt+Down",
        "Alt+Down",
        false,
    ),
    ("previous_game", "Previous Game", "Cmd+[", "Ctrl+[", true),
    ("next_game", "Next Game", "Cmd+]", "Ctrl+]", true),
];

/// The items the main window enables as what it shows changes.
pub struct Items {
    /// **File ▸ Open Game Log…**
    open: MenuItem<Wry>,
    /// The **Go** menu's items, each with whether it moves between games.
    go: Vec<(MenuItem<Wry>, bool)>,
}

/// Enable **Open Game Log…** when `open`, and the **Go** menu while the
/// main window reviews a log of `games` games, none outside review, its
/// **Previous Game** and **Next Game** only when there are several.
#[tauri::command]
pub fn menus(items: State<'_, Items>, open: bool, games: usize) {
    let enable = |item: &MenuItem<Wry>, enabled: bool| {
        if let Err(error) = item.set_enabled(enabled) {
            tracing::warn!("a menu item could not be enabled: {error}");
        }
    };
    enable(&items.open, open);
    for (item, between) in &items.go {
        enable(item, games > usize::from(*between));
    }
}

/// The app's menu bar. The items the main window enables are kept in
/// [`Items`].
///
/// # Errors
///
/// When a menu cannot be made.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let separator = || PredefinedMenuItem::separator(app);
    let label = if cfg!(target_os = "macos") {
        "Settings…"
    } else {
        "Preferences…"
    };
    let preferences = MenuItem::with_id(app, "settings", label, true, Some("CmdOrCtrl+,"))?;
    let new_game = MenuItem::with_id(app, "new_game", "New Game", true, Some("CmdOrCtrl+N"))?;
    let open = MenuItem::with_id(app, "open_log", "Open Game Log…", true, Some("CmdOrCtrl+O"))?;
    let import = MenuItem::with_id(app, "import", "Import Configuration…", true, None::<&str>)?;
    let export = MenuItem::with_id(app, "export", "Export Configuration…", true, None::<&str>)?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &new_game,
            &open,
            &separator()?,
            &import,
            &export,
            &separator()?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
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
    for (id, label, mac, elsewhere, between) in GO {
        let shortcut = if cfg!(target_os = "macos") {
            mac
        } else {
            elsewhere
        };
        let item = MenuItem::with_id(app, format!("go_{id}"), label, false, Some(shortcut))?;
        go.push((item, between));
    }
    let go_menu = Submenu::with_id(app, "go", "Go", true)?;
    for (index, (item, _)) in go.iter().enumerate() {
        if index > 0 && index % 2 == 0 {
            go_menu.append(&separator()?)?;
        }
        go_menu.append(item)?;
    }
    app.manage(Items {
        open: open.clone(),
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
    if cfg!(target_os = "macos") {
        let name = app.package_info().name.clone();
        let app_menu = Submenu::with_items(
            app,
            &name,
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &separator()?,
                &preferences,
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
        Menu::with_items(app, &[&app_menu, &file, &edit, &go_menu, &window])
    } else {
        edit.append_items(&[&separator()?, &preferences])?;
        file.append_items(&[&PredefinedMenuItem::quit(app, None)?])?;
        Menu::with_items(app, &[&file, &edit, &go_menu, &window])
    }
}

/// Do what the menu item of `event` says. A dialog blocks, so Open, Import
/// and Export run on threads of their own. A **Go** item tells the main
/// window where the cursor goes.
pub fn chosen(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    if let Some(go) = id.strip_prefix("go_") {
        if let Err(error) = app.emit_to("main", "go", go) {
            tracing::warn!("Go could not be announced: {error}");
        }
        return;
    }
    match id {
        "settings" => {
            if let Err(error) = settings::show(app, None) {
                tracing::warn!("the Settings window could not be shown: {error}");
            }
        }
        "new_game" => {
            if let Err(error) = app.emit_to("main", "new-game", ()) {
                tracing::warn!("New Game could not be announced: {error}");
            }
        }
        "open_log" => {
            let app = app.clone();
            std::thread::spawn(move || review::open(&app));
        }
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
