//! The menus every app has: **New Game**, **Open Game Log…**, **Save Game
//! Log…**, **Import Configuration…** and **Export Configuration…** in
//! File, each but New Game doing what the app's [`File`] says;
//! **Settings…** and **Install Command-Line Tool…** in the app menu on
//! macOS, and **Preferences…** in Edit elsewhere; the **Go** menu, which
//! moves through a log under review; and the standard Edit and Window
//! items, so text fields take the usual shortcuts. The main window enables
//! Open, Save and Go as what it shows changes.

use free_agent_gui_types::go::{Go, MENU};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// What the app's File menu does. Open, Import and Export show dialogs,
/// which block, so each runs on a thread of its own.
pub struct File {
    /// **Open Game Log…**
    pub open: fn(&AppHandle),
    /// **Save Game Log…**
    pub save: fn(AppHandle),
    /// **Import Configuration…**
    pub import: fn(&AppHandle),
    /// **Export Configuration…**
    pub export: fn(&AppHandle),
}

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

/// The app's menu bar, with on macOS an item that installs the command
/// line `command`. The items the main window enables are kept in
/// [`Items`].
///
/// # Errors
///
/// When a menu cannot be made.
pub fn build(app: &AppHandle, command: &'static str) -> tauri::Result<Menu<Wry>> {
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
    app.manage(Items { open, save, go });
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    #[cfg(target_os = "macos")]
    let command_line = crate::command_line::item(app, command)?;
    #[cfg(not(target_os = "macos"))]
    let _ = command;
    bar(
        app,
        &preferences,
        #[cfg(target_os = "macos")]
        command_line,
        &file,
        &edit,
        &go_menu,
        &window,
    )
}

/// The menu bar on macOS: an app menu holding `preferences` and
/// `command_line`, then `file`, `edit`, `go` and `window`.
#[cfg(target_os = "macos")]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    command_line: crate::command_line::Item,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    go: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    let separator = || PredefinedMenuItem::separator(app);
    let name = app.package_info().name.clone();
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

/// Do what the menu item of `event` says, File's items as `file` says.
/// **New Game** tells the main window to start over, and a **Go** item
/// tells it where the cursor goes. Dialogs block, so those that show them
/// run on threads of their own.
pub fn chosen(app: &AppHandle, event: &MenuEvent, file: &File) {
    let items = app.state::<Items>();
    let chosen = items.go.iter().find(|(item, _)| item.id() == event.id());
    if let Some(&(_, go)) = chosen {
        if let Err(error) = app.emit_to("main", "go", go) {
            tracing::warn!("Go could not be announced: {error}");
        }
        return;
    }
    let apart = |action: fn(&AppHandle)| {
        let app = app.clone();
        std::thread::spawn(move || action(&app));
    };
    match event.id().as_ref() {
        "new_game" => {
            if let Err(error) = app.emit_to("main", "new-game", ()) {
                tracing::warn!("New Game could not be announced: {error}");
            }
        }
        "open_log" => apart(file.open),
        "save_log" => (file.save)(app.clone()),
        "import" => apart(file.import),
        "export" => apart(file.export),
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
        _ => {}
    }
}
