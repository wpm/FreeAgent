//! Social Deduction's commands to the desktop app's backend, through
//! [`free_agent_gui::tauri`]. A game's cues come back down a
//! [`Channel`].

use free_agent_gui::tauri::{Channel, background, call, command, invoke, object};
use serde_json::json;
use social_deduction_gui_types::settings::{
    AppSettings, FieldError, GameSettings, KeyStatus, Listing, Progress, Provider, Snapshot,
};
use social_deduction_gui_types::{Cue, GameName};

/// The settings, what is wrong with them, and where logs go.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn settings_get() -> Result<Snapshot, String> {
    call("settings_get", json!({})).await
}

/// Make the settings `settings`, and what is wrong with them.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn settings_set(settings: &AppSettings) -> Result<Vec<FieldError>, String> {
    call("settings_set", json!({ "settings": settings })).await
}

/// Open the Settings window at the field `path`, or as it was.
pub fn settings_open(path: Option<Vec<String>>) {
    background("settings_open", json!({ "path": path }));
}

/// The settings `game` starts from.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn game_defaults(game: GameName) -> Result<GameSettings, String> {
    call("game_defaults", json!({ "game": game })).await
}

/// The providers, local servers that answer among them.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn providers() -> Result<Vec<Provider>, String> {
    call("providers", json!({})).await
}

/// The models the provider at `base_url` lists now, with what probing
/// found of each, its probes forgotten first when `forget` says. Its
/// models not yet probed are probed in the background.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn listing(
    base_url: String,
    api_key_env: Option<String>,
    forget: bool,
) -> Result<Listing, String> {
    let args = json!({ "baseUrl": base_url, "apiKeyEnv": api_key_env, "forget": forget });
    call("listing", args).await
}

/// How far probing each provider has got.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn checking() -> Result<Vec<Progress>, String> {
    call("checking", json!({})).await
}

/// Store `key` in the keychain for `variable`.
///
/// # Errors
///
/// When the keychain cannot store it.
pub async fn key_store(variable: String, key: String) -> Result<(), String> {
    call("key_store", json!({ "variable": variable, "key": key })).await
}

/// Remove the key stored for `variable`.
///
/// # Errors
///
/// When the keychain cannot remove it.
pub async fn key_remove(variable: String) -> Result<(), String> {
    call("key_remove", json!({ "variable": variable })).await
}

/// Where the key for `variable` is.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn key_status(variable: String) -> Result<KeyStatus, String> {
    call("key_status", json!({ "variable": variable })).await
}

/// Play a game of `game` under the settings, its cues sent to `cues`. It
/// returns once the game has started.
///
/// # Errors
///
/// Why the game could not start, in full.
pub async fn play(game: GameName, cues: &Channel<Cue>) -> Result<(), String> {
    let game = serde_wasm_bindgen::to_value(&game).map_err(|error| error.to_string())?;
    let args = object(&[("game", &game), ("cues", cues.as_ref())]);
    invoke("play", args).await.map(drop)
}

/// Stop the game being prepared or playing.
pub fn stop() {
    command("stop", object(&[]));
}

/// Enable the menus for what the main window shows: **Open Game Log…**
/// when `open`, **Save Game Log…** when `save`, and the **Go** menu while
/// it reviews a log of `games` games, none outside review, its **Previous
/// Game** and **Next Game** when there are several.
pub fn menus(open: bool, save: bool, games: usize) {
    background(
        "menus",
        json!({ "open": open, "save": save, "games": games }),
    );
}

/// Save the log of the game just played where the user picks.
pub fn save_log() {
    command("save_log", object(&[]));
}
