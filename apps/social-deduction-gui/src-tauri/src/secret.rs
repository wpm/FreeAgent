//! API keys, kept in the OS credential store: the macOS Keychain, the
//! Windows Credential Manager or the Linux Secret Service, through
//! [`keyring`]. A key is encrypted at rest, keyed to the user's login, and
//! never written to a settings file or a log.
//!
//! Each key variable has an entry of its own: its service is [`SERVICE`]
//! and its account the variable's name, such as `OPENAI_API_KEY`.
//! [`Keychain`] looks a key up there first, then in the process
//! environment variable of the same name, a dev and CI override. A machine
//! with no credential store, such as a headless Linux box, takes its keys
//! from the environment alone. There is never a plaintext fallback.

use keyring::Entry;
use secrecy::{ExposeSecret, SecretString};
use social_deduction::model::spec::{Environment, Keys};
use std::fmt;

/// The service every key's entry is stored under: the app's identifier.
pub const SERVICE: &str = "io.github.wpm.social-deduction";

/// What can go wrong with the credential store. Neither variant carries a
/// key: `keyring`'s messages describe the store, not the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The platform has no credential store, such as a Linux box with no
    /// Secret Service provider.
    StoreUnavailable(String),
    /// Any other failure of the store: it is locked, or access is denied.
    Backend(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreUnavailable(detail) => write!(
                f,
                "the OS credential store is unavailable: {detail}. Set the key's environment \
                 variable, such as OPENAI_API_KEY, instead"
            ),
            Self::Backend(detail) => write!(f, "credential store error: {detail}"),
        }
    }
}

impl std::error::Error for SecretError {}

impl From<keyring::Error> for SecretError {
    fn from(error: keyring::Error) -> Self {
        match error {
            keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
                Self::StoreUnavailable(error.to_string())
            }
            other => Self::Backend(other.to_string()),
        }
    }
}

/// The entry that holds the key `variable` names.
fn entry(variable: &str) -> Result<Entry, SecretError> {
    Ok(Entry::new(SERVICE, variable)?)
}

/// The key `entry` holds, or `None` when it holds none.
fn read(entry: &Entry) -> Result<Option<SecretString>, SecretError> {
    match entry.get_password().map(SecretString::from) {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Store `key` in `entry`, replacing what it held.
fn store(entry: &Entry, key: &SecretString) -> Result<(), SecretError> {
    Ok(entry.set_password(key.expose_secret())?)
}

/// Empty `entry`. An entry already empty is left so.
fn delete(entry: &Entry) -> Result<(), SecretError> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The key stored for `variable`, or `None` when none is.
///
/// # Errors
///
/// When the credential store is unavailable or fails.
pub fn read_key(variable: &str) -> Result<Option<SecretString>, SecretError> {
    read(&entry(variable)?)
}

/// Store `key` for `variable`, replacing any key stored for it.
///
/// # Errors
///
/// When the credential store is unavailable or fails.
pub fn store_key(variable: &str, key: &SecretString) -> Result<(), SecretError> {
    store(&entry(variable)?, key)
}

/// Remove the key stored for `variable`, if there is one.
///
/// # Errors
///
/// When the credential store is unavailable or fails.
pub fn delete_key(variable: &str) -> Result<(), SecretError> {
    delete(&entry(variable)?)
}

/// The OS credential store, then the process environment: where the
/// desktop app's keys come from. An empty key counts as none.
#[derive(Debug, Clone, Copy, Default)]
pub struct Keychain;

impl Keys for Keychain {
    fn key(&self, variable: &str) -> anyhow::Result<Option<SecretString>> {
        found(variable, read_key(variable), || Environment.key(variable))
    }
}

/// The key for `variable`: `stored`, unless it is empty or missing, and
/// otherwise what `environment` holds. A store that failed is logged as a
/// warning, without any key, and the environment is asked.
fn found(
    variable: &str,
    stored: Result<Option<SecretString>, SecretError>,
    environment: impl FnOnce() -> anyhow::Result<Option<SecretString>>,
) -> anyhow::Result<Option<SecretString>> {
    let present = |key: &SecretString| !key.expose_secret().is_empty();
    match stored {
        Ok(Some(key)) if present(&key) => return Ok(Some(key)),
        Ok(_) => {}
        Err(error) => tracing::warn!("could not read {variable} from the keychain: {error}"),
    }
    Ok(environment()?.filter(present))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    /// Point `keyring` at its mock store, in which each entry starts
    /// empty, so no test touches the real one.
    fn mock() {
        static MOCK: Once = Once::new();
        MOCK.call_once(|| {
            keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        });
    }

    /// `key` as a secret.
    fn secret(key: &str) -> SecretString {
        SecretString::from(key)
    }

    /// What `found` gives for `stored` and `environment`, exposed.
    fn exposed(
        stored: Result<Option<SecretString>, SecretError>,
        environment: Option<&str>,
    ) -> Option<String> {
        found("OPENAI_API_KEY", stored, || Ok(environment.map(secret)))
            .unwrap()
            .map(|key| key.expose_secret().to_string())
    }

    #[test]
    fn a_secret_errors_display_never_carries_a_key() {
        let unavailable = SecretError::StoreUnavailable("no Secret Service".to_string());
        let backend = SecretError::Backend("locked".to_string());
        assert_eq!(
            backend.to_string(),
            "credential store error: locked",
            "{backend}"
        );
        assert!(
            unavailable
                .to_string()
                .starts_with("the OS credential store is unavailable")
        );
    }

    #[test]
    fn a_store_unavailable_message_names_the_environment_variable_to_set() {
        let unavailable = SecretError::StoreUnavailable("no Secret Service".to_string());
        assert!(unavailable.to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn keyring_errors_map_to_an_unavailable_store_or_a_backend_failure() {
        let missing = || Box::<dyn std::error::Error + Send + Sync>::from("no store");
        for error in [
            keyring::Error::PlatformFailure(missing()),
            keyring::Error::NoStorageAccess(missing()),
        ] {
            assert!(matches!(
                SecretError::from(error),
                SecretError::StoreUnavailable(_)
            ));
        }
        assert!(matches!(
            SecretError::from(keyring::Error::Invalid("a".into(), "b".into())),
            SecretError::Backend(_)
        ));
    }

    #[test]
    fn the_keychain_falls_through_to_the_environment_when_the_store_has_no_entry() {
        mock();
        assert!(read_key("PATH").unwrap().is_none());
        let path = std::env::var("PATH").unwrap();
        let key = Keychain.key("PATH").unwrap().unwrap();
        assert_eq!(key.expose_secret(), path);
        let unset = "SOCIAL_DEDUCTION_DESKTOP_TEST_UNSET_KEY";
        assert!(Keychain.key(unset).unwrap().is_none());
    }

    #[test]
    fn a_stored_key_comes_before_the_environment() {
        assert_eq!(
            exposed(Ok(Some(secret("stored"))), Some("environment")),
            Some("stored".to_string())
        );
    }

    #[test]
    fn an_empty_key_counts_as_none() {
        assert_eq!(
            exposed(Ok(Some(secret(""))), Some("environment")),
            Some("environment".to_string())
        );
        assert_eq!(exposed(Ok(None), Some("")), None);
    }

    #[test]
    fn a_failed_store_falls_through_to_the_environment() {
        let failed = Err(SecretError::Backend("locked".to_string()));
        assert_eq!(
            exposed(failed, Some("environment")),
            Some("environment".to_string())
        );
    }

    #[test]
    fn a_key_is_stored_read_back_and_deleted() {
        mock();
        let entry = entry("OPENAI_API_KEY").unwrap();
        assert!(read(&entry).unwrap().is_none());
        store(&entry, &secret("sk-test")).unwrap();
        assert_eq!(read(&entry).unwrap().unwrap().expose_secret(), "sk-test");
        delete(&entry).unwrap();
        assert!(read(&entry).unwrap().is_none());
        delete(&entry).unwrap();
    }

    #[test]
    fn the_public_operations_reach_the_store() {
        mock();
        store_key("OPENAI_API_KEY", &secret("sk-test")).unwrap();
        delete_key("OPENAI_API_KEY").unwrap();
        assert!(read_key("OPENAI_API_KEY").unwrap().is_none());
    }
}
