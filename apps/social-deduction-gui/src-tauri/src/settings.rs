//! The commands behind the settings screen: the configuration file being
//! edited, as a form and as text, opened from and saved to files and the
//! presets, and a check of each of its models.
//!
//! The backend holds the document. The frontend draws its view and sends
//! each edit back. The document is saved to `last.toml` in the app's
//! config directory on every **Run**, and opened from there at the start.

use crate::game::PRESETS;
use crate::secret::Keychain;
use social_deduction::document::Document;
use social_deduction::model::check::check;
use social_deduction_stage::form::{Check, FormView, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tauri::{AppHandle, Manager, State, async_runtime};

/// The configuration being edited.
pub struct Editing(Mutex<Document>);

impl Default for Editing {
    /// The first preset.
    fn default() -> Self {
        Editing::new(PRESETS[0].text).expect("every preset is TOML")
    }
}

impl Editing {
    /// The document `text` holds, being edited.
    fn new(text: &str) -> Result<Self, String> {
        Ok(Editing(Mutex::new(Document::parse(text)?)))
    }

    /// The document.
    fn document(&self) -> MutexGuard<'_, Document> {
        self.0.lock().unwrap()
    }

    /// Edit the document with `edit`, and give back its view.
    fn edit(
        &self,
        edit: impl FnOnce(&mut Document) -> Result<(), String>,
    ) -> Result<FormView, String> {
        let mut document = self.document();
        edit(&mut document)?;
        Ok(document.view())
    }
}

/// Where the document last run is kept, in the config directory `dir`.
fn last(dir: &Path) -> PathBuf {
    dir.join("last.toml")
}

/// The document last run, if there is one in the app's config directory
/// and it is TOML.
pub fn opened_last(app: &AppHandle) -> Option<Editing> {
    let dir = app.path().app_config_dir().ok()?;
    Editing::new(&std::fs::read_to_string(last(&dir)).ok()?).ok()
}

/// Keep `text`, the document being run, in the app's config directory,
/// to open with next time.
///
/// # Errors
///
/// When it cannot be written.
pub fn keep(app: &AppHandle, text: &str) -> Result<(), String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    write(&last(&dir), text)
}

/// Write `text` to `path`, making its directory first.
fn write(path: &Path, text: &str) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{} could not be saved: {error}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(failed)?;
    }
    std::fs::write(path, text).map_err(failed)
}

/// The document as the form shows it.
#[tauri::command]
pub fn document_view(editing: State<'_, Editing>) -> FormView {
    editing.document().view()
}

/// Set the key at `path` to `value`, and give back the form.
///
/// # Errors
///
/// When `path` names no key or passes through a value.
#[tauri::command]
pub fn document_set(
    editing: State<'_, Editing>,
    path: Vec<String>,
    value: Value,
) -> Result<FormView, String> {
    editing.edit(|document| document.set(&path, value))
}

/// The document's text.
#[tauri::command]
pub fn document_text(editing: State<'_, Editing>) -> String {
    editing.document().text()
}

/// Replace the document with `text`, and give back the form.
///
/// # Errors
///
/// When `text` is not TOML, with the parser's message; the document is
/// left as it was.
#[tauri::command]
pub fn document_replace(editing: State<'_, Editing>, text: String) -> Result<FormView, String> {
    editing.edit(|document| document.replace(&text))
}

/// Open the configuration file at `path`, and give back the form.
///
/// # Errors
///
/// When it cannot be read or is not TOML.
#[tauri::command(async)]
pub fn open(editing: State<'_, Editing>, path: PathBuf) -> Result<FormView, String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    editing.edit(|document| document.replace(&text))
}

/// Save the document to `path`.
///
/// # Errors
///
/// When it cannot be written.
#[tauri::command(async)]
pub fn save(editing: State<'_, Editing>, path: PathBuf) -> Result<(), String> {
    write(&path, &editing.document().text())
}

/// Open the preset `name`, and give back the form.
///
/// # Errors
///
/// When there is no such preset.
#[tauri::command]
pub fn load_preset(editing: State<'_, Editing>, name: String) -> Result<FormView, String> {
    let preset = PRESETS
        .iter()
        .find(|preset| preset.name == name)
        .ok_or_else(|| format!("There is no preset {name}."))?;
    editing.edit(|document| document.replace(preset.text))
}

/// Ask the model `name` to play, as `social-deduction models --check`
/// does, with its key from the keychain.
///
/// # Errors
///
/// When the document has no such model, its key is not set, its provider
/// does not serve it, or it could not be asked.
#[tauri::command]
pub async fn model_check(editing: State<'_, Editing>, name: String) -> Result<Check, String> {
    let spec = editing
        .document()
        .model(&name)
        .map_err(|error| format!("{error:#}"))?;
    // Probing a model goes through futures the compiler cannot prove
    // Send, as preparing a game does.
    async_runtime::spawn_blocking(move || {
        async_runtime::block_on(async move { check(&spec, &Keychain).await })
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own for the test `name`.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("social-deduction-desktop-tests")
            .join(name)
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// `text` with Unix line endings. A Windows checkout gives the presets
    /// CRLF, which the document writes as LF outside its strings.
    fn unix(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    /// The text of the document `editing` holds, with Unix line endings.
    fn text(editing: &Editing) -> String {
        unix(&editing.document().text())
    }

    #[test]
    fn every_preset_is_edited_as_it_is_written() {
        for preset in PRESETS {
            let editing = Editing::new(preset.text).unwrap();
            assert_eq!(text(&editing), unix(preset.text), "{}", preset.name);
        }
        assert_eq!(text(&Editing::default()), unix(PRESETS[0].text));
    }

    #[test]
    fn an_edit_gives_back_the_form_or_leaves_the_document_as_it_was() {
        let editing = Editing::default();
        let view = editing
            .edit(|document| {
                document.set(
                    &["phases".to_string(), "day_limit".to_string()],
                    Value::Text("90s".to_string()),
                )
            })
            .unwrap();
        assert_eq!(view.phases[1].value.as_deref(), Some("90s"));
        let before = editing.document().text();
        let error = editing
            .edit(|document| document.replace("game = "))
            .unwrap_err();
        assert!(error.contains("line 1"), "{error}");
        assert_eq!(editing.document().text(), before);
    }

    #[test]
    fn a_document_saved_is_written_with_its_directory_and_read_back_the_same() {
        let dir = scratch("saved");
        let path = last(&dir.join("config"));
        write(&path, PRESETS[1].text).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text(&Editing::new(&saved).unwrap()), unix(PRESETS[1].text));
        let error = write(&dir.join("config"), "x").unwrap_err();
        assert!(error.contains("could not be saved"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
