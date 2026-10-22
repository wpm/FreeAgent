//! Twenty Questions' commands to the desktop app's backend, through
//! [`free_agent_gui::tauri`]. A game's cues come back down a
//! [`Channel`]. The commands every app's Settings window sends are
//! [`free_agent_gui::settings`]'s.

use free_agent_gui::tauri::{Channel, background, call, command, invoke, object};
use serde_json::json;
use twenty_questions_gui_types::Cue;
use twenty_questions_gui_types::settings::GameSettings;

/// Open the Settings window at the field `path`, or as it was.
pub fn settings_open(path: Option<Vec<String>>) {
    background("settings_open", json!({ "path": path }));
}

/// The settings the game starts from.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn game_defaults() -> Result<GameSettings, String> {
    call("game_defaults", json!({})).await
}

/// Play a game under the settings, its cues sent to `cues`. It returns
/// once the game has started.
///
/// # Errors
///
/// Why the game could not start, in full.
pub async fn play(cues: &Channel<Cue>) -> Result<(), String> {
    invoke("play", object(&[("cues", cues.as_ref())]))
        .await
        .map(drop)
}

/// Stop the game being prepared or playing.
pub fn stop() {
    command("stop", object(&[]));
}
