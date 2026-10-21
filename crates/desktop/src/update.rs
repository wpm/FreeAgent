//! The app updating itself to the latest release.
//!
//! On launch, a release build asks the website for the newest full
//! release's `latest.json`, through the updater plugin. When it names a newer
//! version, a dialog offers to install it. Installing downloads the update,
//! checks its signature against the public key in `tauri.conf.json`,
//! installs it, stops what the app says to, such as a game, and restarts
//! the app as the new version.
//!
//! A failed check is only logged. A failed install is shown, with the
//! website as the way to the installer. On Linux only an AppImage updates
//! itself: the package manager updates a `.deb` or an `.rpm` install.

use tauri::{AppHandle, async_runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Whether this install updates itself, given whether it runs as an
/// AppImage.
fn supported(is_appimage: bool) -> bool {
    !cfg!(target_os = "linux") || is_appimage
}

/// Check for an update in the background, and offer to install one, if
/// this install updates itself, calling `stop` before restarting.
pub fn check_in_background(app: &AppHandle, stop: fn(&AppHandle)) {
    if !supported(std::env::var_os("APPIMAGE").is_some()) {
        return;
    }
    let app = app.clone();
    async_runtime::spawn(async move {
        match check(&app).await {
            Ok(Some(update)) => offer(app, update, stop),
            Ok(None) => {}
            Err(error) => tracing::warn!("the check for an update failed: {error}"),
        }
    });
}

/// The latest release, if it is newer than this one.
async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<Option<Update>> {
    app.updater()?.check().await
}

/// Ask the user whether to install `update`, and install it, calling
/// `stop` before restarting, if they agree.
fn offer(app: AppHandle, update: Update, stop: fn(&AppHandle)) {
    let message = offered(
        &app.package_info().name,
        &update.version,
        &update.current_version,
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
                async_runtime::spawn(install(app, update, stop));
            }
        });
}

/// What the dialog offering `product`'s `version`, over the `current` one,
/// says.
fn offered(product: &str, version: &str, current: &str) -> String {
    format!(
        "{product} {version} is available. You have {current}. Install it now? \
         The app will restart to finish."
    )
}

/// Download and install `update`, then call `stop` and restart, or say why
/// it failed.
async fn install(app: AppHandle, update: Update, stop: fn(&AppHandle)) {
    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        let message = format!(
            "The update failed: {error}. The installer can be downloaded from the website."
        );
        return crate::dialog::error(&app, "Update Failed", message);
    }
    stop(&app);
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offer_names_the_product_and_both_versions() {
        assert_eq!(
            offered("Social Deduction", "0.3.0", "0.2.0"),
            "Social Deduction 0.3.0 is available. You have 0.2.0. Install it now? \
             The app will restart to finish."
        );
    }

    #[test]
    fn only_an_appimage_updates_itself_on_linux() {
        assert!(supported(true));
        assert_eq!(supported(false), !cfg!(target_os = "linux"));
    }
}
