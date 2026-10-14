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
//!
//! The settings screen stores and removes keys through [`key_store`] and
//! [`key_remove`], and asks where each is through [`key_status`], which
//! never answers with the key. A key from the screen, the store or the
//! environment is a [`SecretString`] from the first statement that holds
//! it.

use keyring::Entry;
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, Serializer};
use social_deduction::model::spec::{Environment, Keys};
use social_deduction_stage::form::KeyStatus;
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

/// A command's error crosses to the frontend as its message.
impl Serialize for SecretError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

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

/// Store `key` for `variable`, from the settings screen. The key arrives
/// as a plain string, which the first statement makes a secret.
///
/// # Errors
///
/// When the credential store is unavailable or fails.
#[tauri::command(async)]
pub fn key_store(variable: String, key: String) -> Result<(), SecretError> {
    let key = SecretString::from(key);
    store_key(&variable, &key)
}

/// Remove the key stored for `variable`, from the settings screen. A key
/// not stored is removed already.
///
/// # Errors
///
/// When the credential store is unavailable or fails.
#[tauri::command(async)]
pub fn key_remove(variable: String) -> Result<(), SecretError> {
    delete_key(&variable)
}

/// Where the key for `variable` is: in the keychain, in the environment,
/// or nowhere.
#[tauri::command(async)]
pub fn key_status(variable: String) -> KeyStatus {
    status(read_key(&variable), || Environment.key(&variable))
}

/// Where a key is, `stored` being what the store holds and `environment`
/// what the environment does. A store that failed is unavailable, unless
/// the environment holds the key.
fn status(
    stored: Result<Option<SecretString>, SecretError>,
    environment: impl FnOnce() -> anyhow::Result<Option<SecretString>>,
) -> KeyStatus {
    let in_environment = || matches!(environment(), Ok(Some(key)) if present(&key));
    match stored {
        Ok(Some(key)) if present(&key) => KeyStatus::InKeychain,
        _ if in_environment() => KeyStatus::InEnvironment,
        Ok(_) => KeyStatus::Missing,
        Err(error) => KeyStatus::Unavailable(error.to_string()),
    }
}

/// Whether `key` counts as a key: an empty one counts as none.
fn present(key: &SecretString) -> bool {
    !key.expose_secret().is_empty()
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

    /// Make the next call to `entry`, a mock's, fail with `error`.
    fn failing(entry: &Entry, error: keyring::Error) {
        entry
            .get_credential()
            .downcast_ref::<keyring::mock::MockCredential>()
            .unwrap()
            .set_error(error);
    }

    /// No key from the environment.
    fn unset() -> anyhow::Result<Option<SecretString>> {
        Ok(None)
    }

    #[test]
    fn a_key_s_status_is_where_it_is_and_never_the_key() {
        mock();
        let entry = entry("OPENAI_API_KEY").unwrap();
        store(&entry, &secret("sk-test")).unwrap();
        assert_eq!(status(read(&entry), unset), KeyStatus::InKeychain);
        delete(&entry).unwrap();
        assert_eq!(status(read(&entry), unset), KeyStatus::Missing);
        let environment = || Ok(Some(secret("sk-environment")));
        assert_eq!(status(read(&entry), environment), KeyStatus::InEnvironment);
        delete(&entry).unwrap();
        // An empty key counts as none.
        store(&entry, &secret("")).unwrap();
        assert_eq!(
            status(read(&entry), || Ok(Some(secret("")))),
            KeyStatus::Missing
        );
        assert_eq!(
            status(read(&entry), || Err(anyhow::anyhow!("not UTF-8"))),
            KeyStatus::Missing
        );
    }

    #[test]
    fn a_store_that_fails_is_unavailable_unless_the_environment_holds_the_key() {
        mock();
        let entry = entry("OPENAI_API_KEY").unwrap();
        let missing = || Box::<dyn std::error::Error + Send + Sync>::from("no store");
        failing(&entry, keyring::Error::PlatformFailure(missing()));
        let KeyStatus::Unavailable(shown) = status(read(&entry), unset) else {
            panic!("the store is available");
        };
        assert!(shown.contains("OPENAI_API_KEY"), "{shown}");
        failing(&entry, keyring::Error::PlatformFailure(missing()));
        let environment = || Ok(Some(secret("sk-environment")));
        assert_eq!(status(read(&entry), environment), KeyStatus::InEnvironment);
    }

    #[test]
    fn the_commands_store_remove_and_find_a_key() {
        mock();
        key_store("OPENAI_API_KEY".to_string(), "sk-test".to_string()).unwrap();
        key_remove("OPENAI_API_KEY".to_string()).unwrap();
        key_remove("OPENAI_API_KEY".to_string()).unwrap();
        // Each mock entry starts empty, and Cargo sets this variable.
        assert_eq!(
            key_status("CARGO_PKG_NAME".to_string()),
            KeyStatus::InEnvironment
        );
        assert_eq!(
            key_status("SOCIAL_DEDUCTION_DESKTOP_TEST_UNSET_KEY".to_string()),
            KeyStatus::Missing
        );
    }

    #[test]
    fn a_secret_error_crosses_as_its_message() {
        let error = SecretError::Backend("locked".to_string());
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            "credential store error: locked"
        );
    }

    /// A key no test or message has any other reason to hold.
    const SENTINEL: &str = "sk-test-sentinel";

    /// What every event logged at any level while `act` runs says, as
    /// text.
    fn logged(act: impl FnOnce()) -> String {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, act);
        let bytes = captured.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn a_key_never_leaks_into_a_log_an_error_or_a_status() {
        mock();
        let variable = "SOCIAL_DEDUCTION_SENTINEL_KEY";
        let mut shown = Vec::new();
        let log = logged(|| {
            key_store(variable.to_string(), SENTINEL.to_string()).unwrap();
            shown.push(format!("{:?}", key_status(variable.to_string())));
            shown.push(format!("{:?}", read_key(variable)));
            shown.push(format!("{:?}", Keychain.key(variable)));
            // A store that fails, on the way in and on the way out.
            let entry = entry(variable).unwrap();
            store(&entry, &secret(SENTINEL)).unwrap();
            let unavailable = || keyring::Error::NoStorageAccess("locked".into());
            failing(&entry, unavailable());
            let error = store(&entry, &secret(SENTINEL)).unwrap_err();
            shown.push(format!("{error} {error:?}"));
            failing(&entry, unavailable());
            let error = read(&entry).unwrap_err();
            shown.push(format!("{error} {error:?}"));
            shown.push(format!("{:?}", status(Err(error.clone()), unset)));
            let found = found(variable, Err(error), || Ok(Some(secret(SENTINEL))));
            shown.push(format!("{found:?}"));
        });
        assert!(log.contains("could not read"), "{log}");
        assert!(!log.contains(SENTINEL), "{log}");
        for shown in shown {
            assert!(!shown.contains(SENTINEL), "{shown}");
        }
    }

    #[test]
    fn the_public_operations_reach_the_store() {
        mock();
        store_key("OPENAI_API_KEY", &secret("sk-test")).unwrap();
        delete_key("OPENAI_API_KEY").unwrap();
        assert!(read_key("OPENAI_API_KEY").unwrap().is_none());
    }
}
