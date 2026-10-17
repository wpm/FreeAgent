//! The commands that play a game: playing the game chosen, and stopping
//! it.

use crate::secret::keychain;
use crate::settings::Store;
use social_deduction::live::Session;
use social_deduction::run::prepare;
use social_deduction::settings;
use social_deduction_stage::{Cue, GameName};
use tauri::ipc::Channel;
use tauri::{State, async_runtime};

/// Play a game of `game` under the settings, its cues sent down `cues`,
/// as [`Session::play`] does, from the configuration the settings make,
/// as `social-deduction play` would play it, with its keys from the
/// keychain. Settings with an error are not played.
#[tauri::command]
pub async fn play(
    session: State<'_, Session>,
    store: State<'_, Store>,
    game: GameName,
    cues: Channel<Cue>,
) -> Result<(), String> {
    let snapshot = store.snapshot();
    if let Some(error) = snapshot.settings.first_error(game, &snapshot.errors) {
        return Err(error.message);
    }
    let text = settings::text(&snapshot.settings, game);
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
