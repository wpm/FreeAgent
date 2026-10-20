//! The app updating itself to the latest release.
//!
//! On launch, a release build asks the website for the newest full
//! release's `latest.json`, through the updater plugin. When it names a newer
//! version, a dialog offers to install it. Installing downloads the update,
//! checks its signature against the public key in `tauri.conf.json`,
//! installs it, stops any game and restarts the app as the new version.
//!
//! A failed check is only logged. A failed install is shown, with the
//! website as the way to the installer. On Linux only an AppImage updates
//! itself: the package manager updates a `.deb` or an `.rpm` install.

use social_deduction::live::Session;
use tauri::{AppHandle, Manager, async_runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Whether this install updates itself, given whether it runs as an
/// AppImage.
fn supported(is_appimage: bool) -> bool {
    !cfg!(target_os = "linux") || is_appimage
}

/// Check for an update in the background, and offer to install one, if
/// this install updates itself.
pub fn check_in_background(app: &AppHandle) {
    if !supported(std::env::var_os("APPIMAGE").is_some()) {
        return;
    }
    let app = app.clone();
    async_runtime::spawn(async move {
        match check(&app).await {
            Ok(Some(update)) => offer(app, update),
            Ok(None) => {}
            Err(error) => tracing::warn!("the check for an update failed: {error}"),
        }
    });
}

/// The latest release, if it is newer than this one.
async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<Option<Update>> {
    app.updater()?.check().await
}

/// Ask the user whether to install `update`, and install it if they agree.
fn offer(app: AppHandle, update: Update) {
    let message = format!(
        "Social Deduction {} is available. You have {}. Install it now? \
         The app will restart to finish.",
        update.version, update.current_version
    );
    app.clone()
        .dialog()
        .message(message)
        .title("Update Available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install and Restart".into(),
            "Later".into(),
        ))
        .show(move |agreed| {
            if agreed {
                async_runtime::spawn(install(app, update));
            }
        });
}

/// Download and install `update`, then stop any game and restart, or say
/// why it failed.
async fn install(app: AppHandle, update: Update) {
    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        let message = format!(
            "The update failed: {error}. The installer can be downloaded from the website."
        );
        return crate::settings::error(&app, "Update Failed", message);
    }
    app.state::<Session>().stop();
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_appimage_updates_itself_on_linux() {
        assert!(supported(true));
        assert_eq!(supported(false), !cfg!(target_os = "linux"));
    }
}
