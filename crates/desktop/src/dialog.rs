//! The dialogs every app shows: an error, and saving a file where the
//! user picks. They block, so they run off the main thread.

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// Tell the user `message`, titled `title`, as an error. It blocks, so it
/// runs off the main thread.
pub fn error(app: &AppHandle, title: &str, message: String) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Error)
        .blocking_show();
}

/// Write `contents` where the user picks, among the files `filter` shows,
/// named `name` by default. A write that fails is shown, with why, in a
/// dialog titled `title`. It blocks, so it runs off the main thread.
pub fn save_as(app: &AppHandle, filter: (&str, &[&str]), name: &str, title: &str, contents: &[u8]) {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter(filter.0, filter.1)
        .set_file_name(name)
        .blocking_save_file()
        .and_then(|path| path.into_path().ok())
    else {
        return;
    };
    if let Err(failure) = std::fs::write(&path, contents) {
        let message = format!("{} couldn't be written: {failure}", path.display());
        error(app, title, message);
    }
}
