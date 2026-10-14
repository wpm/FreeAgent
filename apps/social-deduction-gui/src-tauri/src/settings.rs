//! The commands behind the settings screen: the configuration file being
//! edited, as a form and as text, opened from and saved to files and the
//! presets, and a check of each of its models.
//!
//! The backend holds the [`Editing`] document. The frontend draws its view
//! and sends each edit back.

use crate::secret::keychain;
use social_deduction::editing::Editing;
use social_deduction::model::check::check;
use social_deduction_stage::form::{Check, FormView, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State, async_runtime};

/// The document last run, if there is one in the app's config directory
/// and it is TOML.
pub fn opened_last(app: &AppHandle) -> Option<Editing> {
    Editing::opened_last(&app.path().app_config_dir().ok()?)
}

/// The document as the form shows it.
#[tauri::command]
pub fn document_view(editing: State<'_, Editing>) -> FormView {
    editing.document().view()
}

/// Set the key at `path` to `value`, and give back the form.
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
#[tauri::command]
pub fn document_replace(editing: State<'_, Editing>, text: String) -> Result<FormView, String> {
    editing.edit(|document| document.replace(&text))
}

/// Open the configuration file at `path`, and give back the form.
#[tauri::command(async)]
pub fn open(editing: State<'_, Editing>, path: PathBuf) -> Result<FormView, String> {
    editing.open(&path)
}

/// Save the document to `path`.
#[tauri::command(async)]
pub fn save(editing: State<'_, Editing>, path: PathBuf) -> Result<(), String> {
    editing.save(&path)
}

/// Open the preset `name`, and give back the form.
#[tauri::command]
pub fn load_preset(editing: State<'_, Editing>, name: String) -> Result<FormView, String> {
    editing.load_preset(&name)
}

/// Ask the model `name` to play, as `social-deduction models --check`
/// does, with its key from the keychain.
#[tauri::command]
pub async fn model_check(editing: State<'_, Editing>, name: String) -> Result<Check, String> {
    let spec = editing
        .document()
        .model(&name)
        .map_err(|error| format!("{error:#}"))?;
    // Probing a model goes through futures the compiler cannot prove
    // Send, as preparing a game does.
    async_runtime::spawn_blocking(move || {
        async_runtime::block_on(async move { check(&spec, &keychain()).await })
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("{error:#}"))
}
