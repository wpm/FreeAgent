//! The app's settings: the [`Store`] that holds them in `settings.toml`,
//! the commands the windows read and change them through, the Settings
//! window, and importing and exporting configuration files.
//!
//! The Settings window sends each change as the whole settings, and the
//! main window hears of it as a `settings` event carrying the
//! [`Snapshot`]. A change made from the menus reaches both windows.

use free_agent_desktop::dialog::{error, save_as};
use free_agent_desktop::settings::File;
use social_deduction::settings::{
    self, AppSettings, FieldError, GameSettings, Imported, Snapshot, game_title,
};
use social_deduction_gui_types::GameName;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogResult};

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
    free_agent_desktop::settings::show(&app, path).map_err(|error| error.to_string())
}

/// The configuration files the dialogs show.
const FILTER: (&str, &[&str]) = ("Configuration", &["toml"]);

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
        let snapshot = snapshot(
            app.state::<Store>()
                .change(|held| settings::import(held, imported)),
        );
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
    let text = settings::text(&app.state::<Store>().settings(), game);
    save_as(
        app,
        FILTER,
        &format!("{key}.toml"),
        "Export Configuration",
        text.as_bytes(),
    );
}
