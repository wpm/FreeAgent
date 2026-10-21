//! API keys, kept in the OS credential store: the macOS Keychain, the
//! Windows Credential Manager or the Linux Secret Service, through
//! [`keyring`]. A key is encrypted at rest and keyed to the user's login.
//!
//! Each key variable has an entry of its own: its service is the app's,
//! such as `io.github.wpm.social-deduction`, and its account the
//! variable's name, such as `OPENAI_API_KEY`. [`keychain`] looks a key up
//! there first, then in the environment, as [`Keychain`] does.
//!
//! The Models tab stores and removes each provider's key through
//! [`store`] and [`remove`], and asks where it is through [`status`],
//! which never answers with the key. A key from the window is a
//! [`SecretString`] from the first statement that holds it.

use free_agent_gui_types::settings::KeyStatus;
use free_agent_model::secret::{Keychain, SecretError, Store};
use keyring::Entry;
use secrecy::{ExposeSecret, SecretString};

/// The OS credential store, its entries under the service it names.
#[derive(Debug, Clone, Copy)]
pub struct Os(pub &'static str);

/// The OS credential store, its entries under `service`, then the process
/// environment: where a desktop app's keys come from.
pub fn keychain(service: &'static str) -> Keychain<Os> {
    Keychain(Os(service))
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

impl Os {
    /// The entry that holds the key `variable` names.
    fn entry(&self, variable: &str) -> Result<Entry, SecretError> {
        Entry::new(self.0, variable).map_err(failed)
    }
}

impl Store for Os {
    fn read(&self, variable: &str) -> Result<Option<SecretString>, SecretError> {
        match self.entry(variable)?.get_password() {
            Ok(key) => Ok(Some(SecretString::from(key))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(failed(error)),
        }
    }

    fn write(&self, variable: &str, key: &SecretString) -> Result<(), SecretError> {
        self.entry(variable)?
            .set_password(key.expose_secret())
            .map_err(failed)
    }

    fn delete(&self, variable: &str) -> Result<(), SecretError> {
        match self.entry(variable)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }
}

/// Store `key` for `variable` under `service`, from the Settings window.
/// The key arrives as a plain string, which the first statement makes a
/// secret.
///
/// # Errors
///
/// When the OS credential store is not there or fails.
pub fn store(service: &'static str, variable: &str, key: String) -> Result<(), SecretError> {
    let key = SecretString::from(key);
    keychain(service).store(variable, &key)
}

/// Remove the key stored for `variable` under `service`, from the Settings
/// window.
///
/// # Errors
///
/// When the OS credential store is not there or fails.
pub fn remove(service: &'static str, variable: &str) -> Result<(), SecretError> {
    keychain(service).remove(variable)
}

/// Where the key for `variable` is: in the keychain under `service`, in
/// the environment, or nowhere.
pub fn status(service: &'static str, variable: &str) -> KeyStatus {
    keychain(service).status(variable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use free_agent_model::spec::Keys;

    /// The service the tests store under.
    const SERVICE: &str = "io.github.wpm.free-agent-desktop-test";

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
    fn the_commands_reach_the_os_store_under_the_app_s_service() {
        mock();
        // Each mock entry starts empty, so nothing stored is read back.
        store(SERVICE, "OPENAI_API_KEY", "sk-test".to_string()).unwrap();
        assert!(Os(SERVICE).read("OPENAI_API_KEY").unwrap().is_none());
        remove(SERVICE, "OPENAI_API_KEY").unwrap();
        // Cargo sets this variable.
        assert_eq!(status(SERVICE, "CARGO_PKG_NAME"), KeyStatus::InEnvironment);
        assert!(keychain(SERVICE).key("CARGO_PKG_NAME").unwrap().is_some());
    }
}
