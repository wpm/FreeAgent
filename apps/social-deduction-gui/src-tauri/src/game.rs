//! The commands that play a game: the presets to choose from, playing
//! one, stopping it, and revealing its log.

use crate::secret::keychain;
use social_deduction::editing::{self, PRESETS, Preset};
use social_deduction::live::Session;
use social_deduction::run::prepare;
use social_deduction_stage::Cue;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, async_runtime};

/// The example configurations.
#[tauri::command]
pub fn presets() -> Vec<Preset> {
    PRESETS.to_vec()
}

/// Play the game the configuration `text` describes, its cues sent down
/// `cues`, as [`Session::play`] does, with its keys from the keychain and
/// its log in the app's data directory. The text is kept to open with
/// next time.
#[tauri::command]
pub async fn play(
    app: AppHandle,
    session: State<'_, Session>,
    text: String,
    cues: Channel<Cue>,
) -> Result<(), String> {
    let kept = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())
        .and_then(|dir| editing::keep(&dir, &text));
    if let Err(error) = kept {
        tracing::warn!("{error}");
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    // Preparing probes models through closures whose futures the compiler
    // cannot prove Send, so it runs on a thread of its own, with the
    // runtime's handle. A stop leaves that thread to finish unheard.
    let preparing = async_runtime::spawn_blocking(move || {
        async_runtime::block_on(async move { prepare(&text, &keychain()).await })
    });
    let prepared = async { preparing.await? };
    session
        .play(prepared, &dir, move |cue| Ok(cues.send(cue)?))
        .await
        .map_err(|error| format!("{error:#}"))
}

/// Stop the game being prepared or playing, if there is one.
#[tauri::command]
pub fn stop(session: State<'_, Session>) {
    session.stop();
}

/// Reveal the last game's log in the system's file manager.
#[tauri::command]
pub fn show_log(session: State<'_, Session>) -> Result<(), String> {
    let path = session.log().ok_or("No game has been played.")?;
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|error| error.to_string())
}
