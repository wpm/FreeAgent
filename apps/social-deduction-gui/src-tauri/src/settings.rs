//! The app's settings: the [`Store`] that holds them in `settings.toml`,
//! the commands the windows read and change them through, the Settings
//! window, and importing and exporting configuration files.
//!
//! The Settings window sends each change as the whole settings, and the
//! main window hears of it as a `settings` event carrying the
//! [`Snapshot`]. A change made from the menus reaches both windows.

use social_deduction::settings::{
    self, AppSettings, FieldError, GameSettings, Imported, Snapshot, game_title,
};
use social_deduction_stage::GameName;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

/// The label of the Settings window.
pub const WINDOW: &str = "settings";

/// The settings, as stored in the app's config directory.
pub struct Store {
    dir: PathBuf,
    settings: Mutex<AppSettings>,
}

impl Store {
    /// The settings stored for `app`, as [`settings::load`] reads them,
    /// stored at once the first time, so that the first launch's are kept.
    ///
    /// # Errors
    ///
    /// When the app has no config directory.
    pub fn open(app: &AppHandle) -> tauri::Result<Self> {
        let dir = app.path().app_config_dir()?;
        let first = !dir.join(settings::FILE).exists();
        let loaded = settings::load(&dir);
        if first && let Err(error) = settings::save(&loaded, &dir) {
            tracing::warn!("the settings could not be saved: {error}");
        }
        Ok(Self {
            dir,
            settings: Mutex::new(loaded),
        })
    }

    /// The settings, and what is wrong with them.
    pub fn snapshot(&self) -> Snapshot {
        snapshot(self.settings())
    }

    /// The settings.
    pub fn settings(&self) -> AppSettings {
        self.settings.lock().unwrap().clone()
    }

    /// Change the settings with `change`, store them, and give back the
    /// snapshot after.
    pub fn change(&self, change: impl FnOnce(&mut AppSettings)) -> Snapshot {
        let mut held = self.settings.lock().unwrap();
        change(&mut held);
        if let Err(error) = settings::save(&held, &self.dir) {
            tracing::warn!("the settings could not be saved: {error}");
        }
        snapshot(held.clone())
    }
}

/// `settings` with what is wrong with them.
fn snapshot(settings: AppSettings) -> Snapshot {
    Snapshot {
        errors: settings::validate(&settings),
        settings,
    }
}

/// Tell the window `to`, or every window when `to` is `None`, that the
/// settings are now `snapshot`.
fn announce(app: &AppHandle, to: Option<&str>, snapshot: &Snapshot) {
    let sent = match to {
        Some(to) => app.emit_to(to, "settings", snapshot),
        None => app.emit("settings", snapshot),
    };
    if let Err(error) = sent {
        tracing::warn!("the settings could not be announced: {error}");
    }
}

/// The settings, and what is wrong with them.
#[tauri::command]
pub fn settings_get(store: State<'_, Store>) -> Snapshot {
    store.snapshot()
}

/// Make the settings `settings`, from the Settings window, and give back
/// what is wrong with them. The main window hears of it.
#[tauri::command(async)]
pub fn settings_set(
    app: AppHandle,
    store: State<'_, Store>,
    settings: AppSettings,
) -> Vec<FieldError> {
    let snapshot = store.change(|held| *held = settings);
    announce(&app, Some("main"), &snapshot);
    snapshot.errors
}

/// The settings `game` starts from, which **Restore defaults** brings
/// back.
#[tauri::command]
pub fn game_defaults(game: GameName) -> GameSettings {
    settings::game_defaults(game)
}

/// Open the Settings window at the field `path`, if one is given, or
/// focus it if it is open.
#[tauri::command]
pub async fn settings_open(app: AppHandle, path: Option<Vec<String>>) -> Result<(), String> {
    show(&app, path).map_err(|error| error.to_string())
}

/// Open the Settings window, or focus it if it is open, at the field
/// `path` if one is given. Opening it tries again to check the models
/// that could not be checked, as those of a provider whose account failed.
///
/// # Errors
///
/// When the window cannot be made or shown.
pub fn show(app: &AppHandle, path: Option<Vec<String>>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW) {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        if let Some(path) = path {
            app.emit_to(WINDOW, "focus", path)?;
        }
        return Ok(());
    }
    let hash = match path {
        Some(path) => format!("settings={}", encoded(&path.join("/"))),
        None => "settings".to_string(),
    };
    WebviewWindowBuilder::new(
        app,
        WINDOW,
        WebviewUrl::App(format!("index.html#{hash}").into()),
    )
    .title("Settings")
    .inner_size(760.0, 560.0)
    .min_inner_size(520.0, 400.0)
    .resizable(true)
    .build()?;
    crate::catalog::check_unchecked(app);
    Ok(())
}

/// `text` percent-encoded, as `encodeURIComponent` encodes it.
fn encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                char::from(byte).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

/// The configuration files the dialogs show.
const FILTER: (&str, &[&str]) = ("Configuration", &["toml"]);

/// Tell the user `message`, titled `title`, as an error.
fn error(app: &AppHandle, title: &str, message: String) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Error)
        .blocking_show();
}

/// **Import Configuration…**: read a configuration file the user picks,
/// and, once they confirm, make its game's settings those in the file and
/// add its models. A file that does not parse is shown, with its error,
/// in a dialog naming it. It blocks, so it runs off the main thread.
pub fn import(app: &AppHandle) {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter(FILTER.0, FILTER.1)
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
    else {
        return;
    };
    let name = path.display().to_string();
    let imported = std::fs::read_to_string(&path)
        .map_err(anyhow::Error::from)
        .and_then(|text| Imported::parse(&text));
    let imported = match imported {
        Ok(imported) => imported,
        Err(failure) => {
            return error(
                app,
                "Import Configuration",
                format!("{name} couldn't be imported:\n\n{failure:#}"),
            );
        }
    };
    let title = game_title(imported.game);
    let confirmed = app
        .dialog()
        .message(format!(
            "Replace the {title} settings with those in {name}? Its models join the shared models."
        ))
        .title("Import Configuration")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Replace".into(),
            "Cancel".into(),
        ))
        .blocking_show();
    if confirmed {
        let snapshot = app
            .state::<Store>()
            .change(|held| settings::import(held, imported));
        announce(app, None, &snapshot);
    }
}

/// **Export Configuration…**: ask which game, then write that game's
/// configuration, with its models, to a file the user picks. It blocks, so
/// it runs off the main thread.
pub fn export(app: &AppHandle) {
    let asked = app
        .dialog()
        .message("Which game's configuration?")
        .title("Export Configuration")
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            "Werewolf".into(),
            "Avalon".into(),
            "Cancel".into(),
        ))
        .blocking_show_with_result();
    let game = match asked {
        MessageDialogResult::Yes => GameName::Werewolf,
        MessageDialogResult::No => GameName::Avalon,
        MessageDialogResult::Custom(label) if label == "Werewolf" => GameName::Werewolf,
        MessageDialogResult::Custom(label) if label == "Avalon" => GameName::Avalon,
        _ => return,
    };
    let key = social_deduction::settings::game_key(game);
    let Some(path) = app
        .dialog()
        .file()
        .add_filter(FILTER.0, FILTER.1)
        .set_file_name(format!("{key}.toml"))
        .blocking_save_file()
        .and_then(|path| path.into_path().ok())
    else {
        return;
    };
    let text = settings::text(&app.state::<Store>().settings(), game);
    if let Err(failure) = std::fs::write(&path, text) {
        let message = format!("{} couldn't be written: {failure}", path.display());
        error(app, "Export Configuration", message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_s_path_is_encoded_for_the_hash() {
        assert_eq!(
            encoded("werewolf/strategies/everyone"),
            "werewolf/strategies/everyone"
        );
        assert_eq!(encoded("a b#é"), "a%20b%23%C3%A9");
    }
}
