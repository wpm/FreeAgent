//! The commands that play a game: playing it, stopping it, and saving its
//! log once it is over.

use crate::review::FILTER;
use crate::secret::keychain;
use crate::settings::{Store, snapshot};
use chrono::{DateTime, TimeZone};
use free_agent_desktop::dialog::save_as;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, async_runtime};
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

/// **Save Game Log…**, from its menu item and the review's **Save Log…**.
#[tauri::command]
pub fn save_log(app: AppHandle) {
    std::thread::spawn(move || save(&app));
}

/// Write the log of the game just played where the user picks, named for
/// when it started by default. A save that fails is shown, with why, in a
/// dialog. Nothing is saved before the game is over. It blocks, so it runs
/// off the main thread.
fn save(app: &AppHandle) {
    let Some(log) = app.state::<Session>().log() else {
        return;
    };
    let started = DateTime::<chrono::Local>::from(log.started);
    save_as(
        app,
        FILTER,
        &file_name(&started),
        "Save Game Log",
        &log.bytes,
    );
}

/// The name a log of a game started at `started` is saved under:
/// `twenty-questions-2026-10-21-1530.jsonl`.
fn file_name<Z: TimeZone>(started: &DateTime<Z>) -> String
where
    Z::Offset: std::fmt::Display,
{
    format!("twenty-questions-{}.jsonl", started.format("%Y-%m-%d-%H%M"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn a_log_is_named_for_the_local_time_its_game_started() {
        let pacific = FixedOffset::west_opt(7 * 3600).unwrap();
        let started = pacific.with_ymd_and_hms(2026, 10, 21, 15, 30, 59).unwrap();
        assert_eq!(
            file_name(&started),
            "twenty-questions-2026-10-21-1530.jsonl"
        );
    }
}
