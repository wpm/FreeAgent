//! The configuration file the desktop app's settings screen edits: a
//! [`Document`] opened from the [`PRESETS`] or from files and saved to
//! files. The document run last is kept in `last.toml`, in the app's
//! config directory, and opened from there at the start.

use crate::document::Document;
use serde::Serialize;
use social_deduction_stage::form::FormView;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// An example configuration to play.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    /// Where it is among the examples: `werewolf/random`.
    pub name: &'static str,
    /// The configuration file's text.
    pub text: &'static str,
}

/// The example configurations, in the order the settings screen lists
/// them.
pub const PRESETS: [Preset; 4] = [
    Preset {
        name: "werewolf/random",
        text: include_str!("../configs/werewolf/random.toml"),
    },
    Preset {
        name: "werewolf/prompted",
        text: include_str!("../configs/werewolf/prompted.toml"),
    },
    Preset {
        name: "avalon/random",
        text: include_str!("../configs/avalon/random.toml"),
    },
    Preset {
        name: "avalon/prompted",
        text: include_str!("../configs/avalon/prompted.toml"),
    },
];

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
    ///
    /// # Errors
    ///
    /// When `text` is not TOML, with the parser's message.
    pub fn new(text: &str) -> Result<Self, String> {
        Ok(Editing(Mutex::new(Document::parse(text)?)))
    }

    /// The document run last, if there is one in the config directory
    /// `dir` and it is TOML.
    pub fn opened_last(dir: &Path) -> Option<Self> {
        Editing::new(&std::fs::read_to_string(last(dir)).ok()?).ok()
    }

    /// The document.
    pub fn document(&self) -> MutexGuard<'_, Document> {
        self.0.lock().unwrap()
    }

    /// Edit the document with `edit`, and give back its view.
    ///
    /// # Errors
    ///
    /// What `edit` fails with.
    pub fn edit(
        &self,
        edit: impl FnOnce(&mut Document) -> Result<(), String>,
    ) -> Result<FormView, String> {
        let mut document = self.document();
        edit(&mut document)?;
        Ok(document.view())
    }

    /// Open the configuration file at `path`, and give back the form.
    ///
    /// # Errors
    ///
    /// When it cannot be read or is not TOML; the document is left as it
    /// was.
    pub fn open(&self, path: &Path) -> Result<FormView, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
        self.edit(|document| document.replace(&text))
    }

    /// Save the document to `path`, making its directory first.
    ///
    /// # Errors
    ///
    /// When it cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        write(path, &self.document().text())
    }

    /// Open the preset `name`, and give back the form.
    ///
    /// # Errors
    ///
    /// When there is no such preset.
    pub fn load_preset(&self, name: &str) -> Result<FormView, String> {
        let preset = PRESETS
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| format!("There is no preset {name}."))?;
        self.edit(|document| document.replace(preset.text))
    }
}

/// Keep `text`, the document being run, in the config directory `dir`, to
/// open with next time.
///
/// # Errors
///
/// When it cannot be written.
pub fn keep(dir: &Path, text: &str) -> Result<(), String> {
    write(&last(dir), text)
}

/// Where the document run last is kept, in the config directory `dir`.
fn last(dir: &Path) -> PathBuf {
    dir.join("last.toml")
}

/// Write `text` to `path`, making its directory first.
fn write(path: &Path, text: &str) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{} could not be saved: {error}", path.display());
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(""))).map_err(failed)?;
    std::fs::write(path, text).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use social_deduction_stage::form::Value;

    /// A directory of its own for the test `name`, empty and not yet made.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("editing-{}", std::process::id()))
            .join(name);
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
    fn the_presets_are_the_four_examples_and_each_parses_as_the_game_it_names() {
        let names: Vec<&str> = PRESETS.iter().map(|preset| preset.name).collect();
        assert_eq!(
            names,
            [
                "werewolf/random",
                "werewolf/prompted",
                "avalon/random",
                "avalon/prompted"
            ]
        );
        for preset in PRESETS {
            let game = match config::parse(preset.text).unwrap() {
                config::Settled::Werewolf(_) => "werewolf",
                config::Settled::Avalon(_) => "avalon",
            };
            assert!(preset.name.starts_with(game), "{}", preset.name);
        }
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
    fn a_preset_is_opened_by_name() {
        let editing = Editing::default();
        editing.load_preset("avalon/prompted").unwrap();
        assert_eq!(text(&editing), unix(PRESETS[3].text));
        let error = editing.load_preset("chess/random").unwrap_err();
        assert_eq!(error, "There is no preset chess/random.");
        assert_eq!(text(&editing), unix(PRESETS[3].text));
    }

    #[test]
    fn a_document_saved_is_written_with_its_directory_and_opened_back_the_same() {
        let dir = scratch("saved");
        let path = dir.join("configs").join("game.toml");
        Editing::new(PRESETS[1].text).unwrap().save(&path).unwrap();
        let editing = Editing::default();
        editing.open(&path).unwrap();
        assert_eq!(text(&editing), unix(PRESETS[1].text));
        let error = editing.save(&dir.join("configs")).unwrap_err();
        assert!(error.contains("could not be saved"), "{error}");
        let error = editing.open(&dir.join("missing.toml")).unwrap_err();
        assert!(error.contains("could not be read"), "{error}");
        std::fs::write(dir.join("configs").join("bad.toml"), "game = ").unwrap();
        let error = editing
            .open(&dir.join("configs").join("bad.toml"))
            .unwrap_err();
        assert!(error.contains("line 1"), "{error}");
        assert_eq!(text(&editing), unix(PRESETS[1].text));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_document_kept_is_opened_last_next_time() {
        let dir = scratch("kept").join("config");
        assert!(Editing::opened_last(&dir).is_none());
        keep(&dir, PRESETS[2].text).unwrap();
        let opened = Editing::opened_last(&dir).unwrap();
        assert_eq!(text(&opened), unix(PRESETS[2].text));
        keep(&dir, "game = ").unwrap();
        assert!(Editing::opened_last(&dir).is_none());
        let error = keep(&dir.join("last.toml"), "x").unwrap_err();
        assert!(error.contains("could not be saved"), "{error}");
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
