//! API keys, kept in the OS credential store under Social Deduction's
//! [`SERVICE`], as [`free_agent_desktop::secret`] keeps them, and the
//! Models tab's commands that store, remove and find them.

use free_agent_desktop::secret::{self, Os};
use free_agent_model::secret::{Keychain, SecretError};
use social_deduction_gui_types::settings::KeyStatus;

/// The service every key's entry is stored under: the app's identifier.
pub const SERVICE: &str = "io.github.wpm.social-deduction";

/// The OS credential store, then the process environment: where the
/// desktop app's keys come from.
pub fn keychain() -> Keychain<Os> {
    secret::keychain(SERVICE)
}

/// Store `key` for `variable`, from the Settings window.
#[tauri::command(async)]
pub fn key_store(variable: String, key: String) -> Result<(), SecretError> {
    secret::store(SERVICE, &variable, key)
}

/// Remove the key stored for `variable`, from the Settings window.
#[tauri::command(async)]
pub fn key_remove(variable: String) -> Result<(), SecretError> {
    secret::remove(SERVICE, &variable)
}

/// Where the key for `variable` is: in the keychain, in the environment,
/// or nowhere.
#[tauri::command(async)]
pub fn key_status(variable: String) -> KeyStatus {
    secret::status(SERVICE, &variable)
}
