//! The app's settings: the [`Store`] that holds them in `settings.toml`,
//! the commands the windows read and change them through, and the
//! Settings window.
//!
//! The Settings window sends each change as the whole settings, and the
//! main window hears of it as a `settings` event carrying the
//! [`Snapshot`].

use free_agent_desktop::settings::File;
use tauri::{AppHandle, Emitter, State};
use twenty_questions::settings::{self, AppSettings, FieldError, GameSettings, Snapshot};

/// The settings, as stored in the app's config directory.
pub type Store = free_agent_desktop::settings::Store<AppSettings>;

/// How the settings are kept: in [`settings::FILE`], read with
/// [`settings::load`] and written with [`settings::save`], starting from
/// [`settings::defaults`].
pub fn file() -> File<AppSettings> {
    File {
        name: settings::FILE,
        load: settings::load,
        save: settings::save,
        defaults: settings::defaults,
    }
}

/// `settings` with what is wrong with them.
pub(crate) fn snapshot(settings: AppSettings) -> Snapshot {
    Snapshot {
        errors: settings::validate(&settings),
        settings,
    }
}

/// The settings, and what is wrong with them.
#[tauri::command]
pub fn settings_get(store: State<'_, Store>) -> Snapshot {
    snapshot(store.settings())
}

/// Make the settings `settings`, from the Settings window, and give back
/// what is wrong with them. The main window hears of it.
#[tauri::command(async)]
pub fn settings_set(
    app: AppHandle,
    store: State<'_, Store>,
    settings: AppSettings,
) -> Vec<FieldError> {
    let snapshot = snapshot(store.change(|held| *held = settings));
    if let Err(error) = app.emit_to("main", "settings", &snapshot) {
        tracing::warn!("the settings could not be announced: {error}");
    }
    snapshot.errors
}

/// The game's settings as the app came with them, which **Restore
/// defaults** brings back.
#[tauri::command]
pub fn game_defaults() -> GameSettings {
    settings::game_defaults()
}

/// Open the Settings window at the field `path`, if one is given, or
/// focus it if it is open.
#[tauri::command]
pub async fn settings_open(app: AppHandle, path: Option<Vec<String>>) -> Result<(), String> {
    free_agent_desktop::settings::show(&app, path).map_err(|error| error.to_string())
}
