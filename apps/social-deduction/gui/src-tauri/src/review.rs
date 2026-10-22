//! **Open Game Log…**: a log the user picks, read as `social-deduction`
//! reads one, as [`free_agent_desktop::review`] opens it.

use free_agent_desktop::review;
use social_deduction::live::Session;
use social_deduction::stage;
use tauri::{AppHandle, Manager};

/// Read a game log the user picks and send its games to the main window
/// to review, unless a game is prepared or playing.
pub fn open(app: &AppHandle) {
    review::open(app, app.state::<Session>().busy(), stage::opened);
}
