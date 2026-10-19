//! The menus: **Settings…** and **Install Command-Line Tool…** in the app
//! menu on macOS, and **Preferences…** in Edit elsewhere; **New Game**,
//! **Open Game Log…**, **Save Game Log…**, **Import Configuration…** and
//! **Export Configuration…** in File; the **Go** menu, which moves through
//! a log under review; and the standard Edit and Window items, so text
//! fields take the usual shortcuts.

use crate::{game, review, settings};
use social_deduction_cues::go::{Go, MENU};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State, Wry};

/// The ID of the **Go** menu's item that goes where `go` says.
fn id(go: Go) -> String {
    format!("go_{go:?}")
}

/// Where the **Go** menu's item with the ID `id` goes, or nothing for any
/// other item.
fn go(id: &str) -> Option<Go> {
    MENU.iter()
        .flatten()
        .map(|item| item.go)
        .find(|&go| self::id(go) == id)
}

/// The items the main window enables as what it shows changes.
pub struct Items {
    /// **File ▸ Open Game Log…**
    open: MenuItem<Wry>,
    /// **File ▸ Save Game Log…**
    save: MenuItem<Wry>,
    /// The **Go** menu's items, each with where it goes.
    go: Vec<(MenuItem<Wry>, Go)>,
}

/// Enable **Open Game Log…** when `open`, **Save Game Log…** when
/// `save`, and the **Go** menu while the main window reviews a log of
/// `games` games, none outside review, its **Previous Game** and **Next
/// Game** only when there are several.
#[tauri::command]
pub fn menus(items: State<'_, Items>, open: bool, save: bool, games: usize) {
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
            let made = MenuItem::with_id(app, id(item.go), item.label, false, Some(shortcut))?;
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
    bar(app, &preferences, &file, &edit, &go_menu, &window)
}

/// The menu bar on macOS: an app menu holding `preferences` and
/// **Install Command-Line Tool…**, then `file`, `edit`, `go` and `window`.
#[cfg(target_os = "macos")]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    go: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    let separator = || PredefinedMenuItem::separator(app);
    let name = app.package_info().name.clone();
    let command_line = crate::command_line::item(app)?;
    app.manage(crate::command_line::Item(command_line.clone()));
    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &separator()?,
            preferences,
            &command_line,
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
    Menu::with_items(app, &[&app_menu, file, edit, go, window])
}

/// The menu bar elsewhere: `file`, with Quit, `edit`, with `preferences`,
/// `go` and `window`.
#[cfg(not(target_os = "macos"))]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    go: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    edit.append_items(&[&PredefinedMenuItem::separator(app)?, preferences])?;
    file.append_items(&[&PredefinedMenuItem::quit(app, None)?])?;
    Menu::with_items(app, &[file, edit, go, window])
}

/// Do what the menu item of `event` says. A dialog blocks, so Open, Save,
/// Import and Export run on threads of their own. A **Go** item tells the main
/// window where the cursor goes.
pub fn chosen(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    if let Some(go) = go(id) {
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
        "save_log" => game::save_log(app.clone()),
        "import" => {
            let app = app.clone();
            std::thread::spawn(move || settings::import(&app));
        }
        "export" => {
            let app = app.clone();
            std::thread::spawn(move || settings::export(&app));
        }
        #[cfg(target_os = "macos")]
        crate::command_line::ID => {
            let app = app.clone();
            std::thread::spawn(move || {
                use tauri::Manager;
                let item = app.state::<crate::command_line::Item>();
                crate::command_line::chosen(&app, &item);
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_go_item_is_known_by_its_id_and_no_other_item_is() {
        for item in MENU.iter().flatten() {
            assert_eq!(go(&id(item.go)), Some(item.go));
        }
        assert_eq!(id(Go::PreviousChapter), "go_PreviousChapter");
        assert_eq!(go("settings"), None);
        assert_eq!(go("go"), None);
    }
}
