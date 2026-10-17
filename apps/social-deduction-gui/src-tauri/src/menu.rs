//! The menus: **Settings…** and **Install Command-Line Tool…** in the app
//! menu on macOS, and **Preferences…** in Edit elsewhere; **New Game**, **Import
//! Configuration…** and **Export Configuration…** in File; and the
//! standard Edit and Window items, so text fields take the usual
//! shortcuts.

use crate::settings;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Wry};

/// The app's menu bar.
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
    let import = MenuItem::with_id(app, "import", "Import Configuration…", true, None::<&str>)?;
    let export = MenuItem::with_id(app, "export", "Export Configuration…", true, None::<&str>)?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &new_game,
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
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    bar(app, &preferences, &file, &edit, &window)
}

/// The menu bar on macOS: an app menu holding `preferences` and
/// **Install Command-Line Tool…**, then `file`, `edit` and `window`.
#[cfg(target_os = "macos")]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    use tauri::Manager;
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
    Menu::with_items(app, &[&app_menu, file, edit, window])
}

/// The menu bar elsewhere: `file`, with Quit, `edit`, with `preferences`,
/// and `window`.
#[cfg(not(target_os = "macos"))]
fn bar(
    app: &AppHandle,
    preferences: &MenuItem<Wry>,
    file: &Submenu<Wry>,
    edit: &Submenu<Wry>,
    window: &Submenu<Wry>,
) -> tauri::Result<Menu<Wry>> {
    edit.append_items(&[&PredefinedMenuItem::separator(app)?, preferences])?;
    file.append_items(&[&PredefinedMenuItem::quit(app, None)?])?;
    Menu::with_items(app, &[file, edit, window])
}

/// Do what the menu item of `event` says. A dialog blocks, so Import and
/// Export run on threads of their own.
pub fn chosen(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
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
