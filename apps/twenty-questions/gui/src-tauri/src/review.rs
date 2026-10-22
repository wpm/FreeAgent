//! **Open Game Log…**: a log the user picks, read as `twenty-questions`
//! reads one, as [`free_agent_desktop::review`] opens it.

use free_agent_desktop::review;
use tauri::{AppHandle, Manager};
use twenty_questions::live::Session;
use twenty_questions::stage;

/// Read a game log the user picks and send its games to the main window
/// to review, unless a game is prepared or playing.
pub fn open(app: &AppHandle) {
    review::open(app, app.state::<Session>().busy(), stage::opened);
}
