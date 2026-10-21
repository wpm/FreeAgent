//! **Open Game Log…**: a log the user picks, read as `social-deduction`
//! reads one, sent to the main window to review.

use social_deduction::live::Session;
use social_deduction::stage;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use free_agent_desktop::settings::error;

/// The game logs the dialogs show.
pub(crate) const FILTER: (&str, &[&str]) = ("Game log", &["jsonl"]);

/// Read a game log the user picks and send its games, with any problems
/// and the file's name, to the main window as a `review` event. A file
/// with no game that reads is shown, with why, in a dialog naming it.
/// Nothing opens while a game is prepared or playing. It blocks, so it
/// runs off the main thread.
pub fn open(app: &AppHandle) {
    if app.state::<Session>().busy() {
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
    match stage::opened(&path) {
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
