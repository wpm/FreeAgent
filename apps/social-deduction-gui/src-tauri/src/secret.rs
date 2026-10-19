//! API keys, kept in the OS credential store: the macOS Keychain, the
//! Windows Credential Manager or the Linux Secret Service, through
//! [`keyring`]. A key is encrypted at rest and keyed to the user's login.
//!
//! Each key variable has an entry of its own: its service is [`SERVICE`]
//! and its account the variable's name, such as `OPENAI_API_KEY`.
//! [`keychain`] looks a key up there first, then in the environment, as
//! [`Keychain`] does.
//!
//! The Models tab stores and removes each provider's key through
//! [`key_store`] and [`key_remove`], and asks where it is through
//! [`key_status`], which never answers with the key. A key from the
//! window is a [`SecretString`] from the first statement that holds it.

use keyring::Entry;
use secrecy::{ExposeSecret, SecretString};
use social_deduction::secret::{Keychain, SecretError, Store};
use social_deduction_cues::settings::KeyStatus;

/// The service every key's entry is stored under: the app's identifier.
pub const SERVICE: &str = "io.github.wpm.social-deduction";

/// The OS credential store.
#[derive(Debug, Clone, Copy, Default)]
pub struct Os;

/// The OS credential store, then the process environment: where the
/// desktop app's keys come from.
pub fn keychain() -> Keychain<Os> {
    Keychain(Os)
}

/// `error` as a [`SecretError`]: a store that is not there, or one that
/// failed. `keyring`'s messages describe the store, not the secret.
fn failed(error: keyring::Error) -> SecretError {
    match error {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            SecretError::StoreUnavailable(error.to_string())
        }
        other => SecretError::Backend(other.to_string()),
    }
}

/// The entry that holds the key `variable` names.
fn entry(variable: &str) -> Result<Entry, SecretError> {
    Entry::new(SERVICE, variable).map_err(failed)
}

impl Store for Os {
    fn read(&self, variable: &str) -> Result<Option<SecretString>, SecretError> {
        match entry(variable)?.get_password() {
            Ok(key) => Ok(Some(SecretString::from(key))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(failed(error)),
        }
    }

    fn write(&self, variable: &str, key: &SecretString) -> Result<(), SecretError> {
        entry(variable)?
            .set_password(key.expose_secret())
            .map_err(failed)
    }

    fn delete(&self, variable: &str) -> Result<(), SecretError> {
        match entry(variable)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }
}

/// Store `key` for `variable`, from the Settings window. The key arrives
/// as a plain string, which the first statement makes a secret.
#[tauri::command(async)]
pub fn key_store(variable: String, key: String) -> Result<(), SecretError> {
    let key = SecretString::from(key);
    keychain().store(&variable, &key)
}

/// Remove the key stored for `variable`, from the Settings window.
#[tauri::command(async)]
pub fn key_remove(variable: String) -> Result<(), SecretError> {
    keychain().remove(&variable)
}

/// Where the key for `variable` is: in the keychain, in the environment,
/// or nowhere.
#[tauri::command(async)]
pub fn key_status(variable: String) -> KeyStatus {
    keychain().status(&variable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction::model::spec::Keys;

    /// Point `keyring` at its mock store, in which each entry starts
    /// empty, so no test touches the real one.
    fn mock() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }

    #[test]
    fn keyring_errors_map_to_an_unavailable_store_or_a_backend_failure() {
        let missing = || Box::<dyn std::error::Error + Send + Sync>::from("no store");
        for error in [
            keyring::Error::PlatformFailure(missing()),
            keyring::Error::NoStorageAccess(missing()),
        ] {
            assert!(matches!(failed(error), SecretError::StoreUnavailable(_)));
        }
        assert!(matches!(
            failed(keyring::Error::Invalid("a".into(), "b".into())),
            SecretError::Backend(_)
        ));
    }

    #[test]
    fn the_commands_reach_the_os_store() {
        mock();
        // Each mock entry starts empty, so nothing stored is read back.
        key_store("OPENAI_API_KEY".to_string(), "sk-test".to_string()).unwrap();
        assert!(Os.read("OPENAI_API_KEY").unwrap().is_none());
        key_remove("OPENAI_API_KEY".to_string()).unwrap();
        // Cargo sets this variable.
        assert_eq!(
            key_status("CARGO_PKG_NAME".to_string()),
            KeyStatus::InEnvironment
        );
        assert!(keychain().key("CARGO_PKG_NAME").unwrap().is_some());
    }
}
