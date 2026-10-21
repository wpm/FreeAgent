//! The commands that play a game: playing the game chosen, stopping it,
//! and saving its log once it is over.

use crate::review::FILTER;
use crate::secret::keychain;
use crate::settings::{Store, snapshot};
use chrono::{DateTime, TimeZone};
use free_agent_desktop::dialog::save_as;
use social_deduction::config::Name;
use social_deduction::live::Session;
use social_deduction::run::prepare;
use social_deduction::settings;
use social_deduction_gui_types::{Cue, GameName};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, async_runtime};

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
    let snapshot = snapshot(store.settings());
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

/// **Save Game Log…**, from its menu item and the game-over screen's
/// **Save Log…**.
#[tauri::command]
pub fn save_log(app: AppHandle) {
    std::thread::spawn(move || save(&app));
}

/// Write the log of the game just played where the user picks, named
/// for the game and when it started by default. A save that fails is
/// shown, with why, in a dialog. Nothing is saved before the game is
/// over. It blocks, so it runs off the main thread.
fn save(app: &AppHandle) {
    let Some(log) = app.state::<Session>().log() else {
        return;
    };
    let started = DateTime::<chrono::Local>::from(log.started);
    let name = file_name(log.game, &started);
    save_as(app, FILTER, &name, "Save Game Log", &log.bytes);
}

/// The name a log of `game`, started at `started`, is saved under:
/// `werewolf-2026-10-17-1346.jsonl`.
fn file_name<Z: TimeZone>(game: Name, started: &DateTime<Z>) -> String
where
    Z::Offset: std::fmt::Display,
{
    format!("{game}-{}.jsonl", started.format("%Y-%m-%d-%H%M"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn a_log_is_named_for_its_game_and_the_local_time_it_started() {
        let pacific = FixedOffset::west_opt(7 * 3600).unwrap();
        let started = pacific.with_ymd_and_hms(2026, 10, 17, 13, 46, 59).unwrap();
        assert_eq!(
            file_name(Name::Werewolf, &started),
            "werewolf-2026-10-17-1346.jsonl"
        );
        assert_eq!(
            file_name(Name::Avalon, &started),
            "avalon-2026-10-17-1346.jsonl"
        );
    }
}
