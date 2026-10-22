//! **Open Game Log…**: a log the user picks, read as the app reads one,
//! sent to the main window to review.

use crate::dialog::error;
use serde::Serialize;
use std::path::Path;
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

/// The game logs the dialogs show.
pub const FILTER: (&str, &[&str]) = ("Game log", &["jsonl"]);

/// Read a game log the user picks with `opened` and send its games, with
/// any problems and the file's name, to the main window as a `review`
/// event. A file with no game that reads is shown, with why, in a dialog
/// naming it. Nothing opens while the app is `busy` preparing or playing
/// a game. It blocks, so it runs off the main thread.
pub fn open<T: Serialize + Clone>(
    app: &AppHandle,
    busy: bool,
    opened: impl FnOnce(&Path) -> anyhow::Result<T>,
) {
    if busy {
        return;
    }
    let Some(path) = app
        .dialog()
        .file()
        .add_filter(FILTER.0, FILTER.1)
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
    else {
        return;
    };
    match opened(&path) {
        Ok(opened) => {
            if let Err(error) = app.emit_to("main", "review", opened) {
                tracing::warn!("the log could not be sent to the main window: {error}");
            }
        }
        Err(failure) => {
            let message = format!("{} couldn't be opened:\n\n{failure:#}", path.display());
            error(app, "Open Game Log", message);
        }
    }
}
