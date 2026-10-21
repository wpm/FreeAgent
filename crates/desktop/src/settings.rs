//! An app's settings: the [`Store`] that holds them in a file in the app's
//! config directory, and the Settings window, which closes with the main
//! window.
//!
//! The app says how its settings are kept with a [`File`]: the file's
//! name, how it is read and written, and the settings to start from.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// The label of the Settings window.
pub const WINDOW: &str = "settings";

/// How an app keeps settings of type `S` in its config directory.
pub struct File<S> {
    /// The file's name, as `settings.toml`.
    pub name: &'static str,
    /// The settings stored in a config directory, or, when there are none,
    /// the defaults; an error when they do not parse.
    pub load: fn(&Path) -> anyhow::Result<S>,
    /// Store settings in a config directory, made if it is not there.
    pub save: fn(&S, &Path) -> io::Result<()>,
    /// The settings the app starts from.
    pub defaults: fn() -> S,
}

/// The settings, as stored in the app's config directory.
pub struct Store<S> {
    dir: PathBuf,
    file: File<S>,
    settings: Mutex<S>,
}

impl<S: Clone> Store<S> {
    /// The settings stored for `app` as `file` keeps them, stored at once
    /// the first time, so that the first launch's are kept. Settings that
    /// cannot be read are the defaults, and a dialog says why.
    ///
    /// # Errors
    ///
    /// When the app has no config directory.
    pub fn open(app: &AppHandle, file: File<S>) -> tauri::Result<Self> {
        let (store, failure) = Self::at(app.path().app_config_dir()?, file);
        if let Some(message) = failure {
            app.dialog()
                .message(message)
                .title("Settings")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
        Ok(store)
    }

    /// The settings stored in `dir` as `file` keeps them, stored at once
    /// the first time, and what to tell the user when they cannot be read,
    /// in which case they are the defaults.
    fn at(dir: PathBuf, file: File<S>) -> (Self, Option<String>) {
        let path = dir.join(file.name);
        let first = !path.exists();
        let mut failure = None;
        let loaded = (file.load)(&dir).unwrap_or_else(|error| {
            tracing::warn!("{} could not be read: {error:#}", path.display());
            failure = Some(format!(
                "{} couldn't be read, so the settings are the defaults:\n\n{error:#}",
                path.display()
            ));
            (file.defaults)()
        });
        if first && let Err(error) = (file.save)(&loaded, &dir) {
            tracing::warn!("the settings could not be saved: {error}");
        }
        let store = Self {
            dir,
            file,
            settings: Mutex::new(loaded),
        };
        (store, failure)
    }

    /// The settings.
    pub fn settings(&self) -> S {
        self.settings.lock().unwrap().clone()
    }

    /// Change the settings with `change`, store them, and give back the
    /// settings after.
    pub fn change(&self, change: impl FnOnce(&mut S)) -> S {
        let mut held = self.settings.lock().unwrap();
        change(&mut held);
        if let Err(error) = (self.file.save)(&held, &self.dir) {
            tracing::warn!("the settings could not be saved: {error}");
        }
        held.clone()
    }
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

/// When the main window of `window`'s `event` is closing, call `stop`, as
/// to stop a game, and close the Settings window.
pub fn closed(window: &Window, event: &WindowEvent, stop: fn(&AppHandle)) {
    if window.label() == "main"
        && let WindowEvent::CloseRequested { .. } = event
    {
        let app = window.app_handle();
        stop(app);
        if let Some(settings) = app.get_webview_window(WINDOW) {
            let _ = settings.close();
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings kept as a line of text in `test.txt`, which start as
    /// `default`.
    fn text() -> File<String> {
        File {
            name: "test.txt",
            load: |dir| match std::fs::read_to_string(dir.join("test.txt")) {
                Ok(text) if text.starts_with('!') => anyhow::bail!("{text} is not a setting"),
                Ok(text) => Ok(text),
                Err(_) => Ok("default".to_string()),
            },
            save: |text, dir| {
                std::fs::create_dir_all(dir)?;
                std::fs::write(dir.join("test.txt"), text)
            },
            defaults: || "default".to_string(),
        }
    }

    /// A new, empty folder for the test `name`.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_first_launch_s_settings_are_stored_at_once() {
        let dir = folder("first");
        let (store, failure) = Store::at(dir.clone(), text());
        assert_eq!(failure, None);
        assert_eq!(store.settings(), "default");
        assert_eq!(
            std::fs::read_to_string(dir.join("test.txt")).unwrap(),
            "default"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_change_is_stored_and_given_back() {
        let dir = folder("change");
        let (store, _) = Store::at(dir.clone(), text());
        assert_eq!(store.change(|held| held.push('!')), "default!");
        assert_eq!(store.settings(), "default!");
        assert_eq!(
            std::fs::read_to_string(dir.join("test.txt")).unwrap(),
            "default!"
        );
        let (again, _) = Store::at(dir.clone(), text());
        assert_eq!(again.settings(), "default!");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_that_cannot_be_read_are_the_defaults_and_left_alone() {
        let dir = folder("unread");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.txt"), "!").unwrap();
        let (store, failure) = Store::at(dir.clone(), text());
        assert_eq!(store.settings(), "default");
        let failure = failure.unwrap();
        assert!(failure.contains("couldn't be read, so the settings are the defaults"));
        assert!(failure.ends_with("! is not a setting"));
        assert_eq!(std::fs::read_to_string(dir.join("test.txt")).unwrap(), "!");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_field_s_path_is_encoded_for_the_hash() {
        assert_eq!(
            encoded("werewolf/strategies/everyone"),
            "werewolf/strategies/everyone"
        );
        assert_eq!(encoded("a b#é"), "a%20b%23%C3%A9");
    }
}
