//! The commands that play a game: playing the game chosen, stopping it,
//! and revealing its log.

use crate::secret::keychain;
use crate::settings::Store;
use social_deduction::live::Session;
use social_deduction::run::prepare;
use social_deduction::settings::{self, game_title};
use social_deduction_stage::{Cue, GameName};
use std::path::PathBuf;
use tauri::ipc::Channel;
use tauri::{State, async_runtime};

/// Play a game of `game` under the settings, its cues sent down `cues`,
/// as [`Session::play`] does, from the configuration the settings make,
/// as `social-deduction play` would play it, with its keys from the
/// keychain and its log in the folder the settings name. The game is
/// chosen to play next time. Settings with an error are not played.
#[tauri::command]
pub async fn play(
    session: State<'_, Session>,
    store: State<'_, Store>,
    game: GameName,
    cues: Channel<Cue>,
) -> Result<(), String> {
    let snapshot = store.change(|held| held.general.game = game);
    if let Some(error) = snapshot.settings.errors_of(game, &snapshot.errors).next() {
        return Err(format!("{}: {}", game_title(game), error.message));
    }
    let text = settings::text(&snapshot.settings, game);
    let dir = PathBuf::from(snapshot.logs);
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
