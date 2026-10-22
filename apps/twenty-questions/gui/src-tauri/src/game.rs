//! The commands that play a game: playing it, and stopping it.

use crate::secret::keychain;
use crate::settings::{Store, snapshot};
use tauri::ipc::Channel;
use tauri::{State, async_runtime};
use twenty_questions::live::Session;
use twenty_questions::run::prepare;
use twenty_questions::settings;
use twenty_questions_gui_types::Cue;

/// Play a game under the settings, its cues sent down `cues`, as
/// [`Session::play`] does, from the configuration the settings make, as
/// `twenty-questions play` would play it, with its keys from the keychain.
/// Settings with an error are not played.
#[tauri::command]
pub async fn play(
    session: State<'_, Session>,
    store: State<'_, Store>,
    cues: Channel<Cue>,
) -> Result<(), String> {
    let snapshot = snapshot(store.settings());
    if let Some(error) = snapshot.settings.first_error(&snapshot.errors) {
        return Err(error.message);
    }
    let text = settings::text(&snapshot.settings);
    // Preparing probes models through closures whose futures the compiler
    // cannot prove Send, so it runs on a thread of its own, with the
    // runtime's handle. A stop leaves that thread to finish unheard.
    let preparing = async_runtime::spawn_blocking(move || {
        async_runtime::block_on(async move { prepare(&text, &keychain()).await })
    });
    let prepared = async { preparing.await? };
    session
        .play(prepared, move |cue| Ok(cues.send(cue)?))
        .await
        .map_err(|error| format!("{error:#}"))
}

/// Stop the game being prepared or playing, if there is one.
#[tauri::command]
pub fn stop(session: State<'_, Session>) {
    session.stop();
}
