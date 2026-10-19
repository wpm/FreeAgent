//! API keys kept in a credential [`Store`], such as the OS keychain the
//! desktop app keeps them in, then in the process environment. A key is
//! never written to a settings file or a log.
//!
//! [`Keychain`] looks a key up in its store first, and in the environment
//! variable of the same name when the store has none or cannot be read.
//! A machine with no credential store, such as a headless Linux box,
//! takes its keys from the environment alone. There is never a plaintext
//! fallback. Where a key is comes back as a [`KeyStatus`], which never
//! holds the key.

use crate::model::spec::{Environment, Keys};
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, Serializer};
use social_deduction_stage::settings::KeyStatus;
use std::fmt;

/// What can go wrong with a credential store. Neither variant carries a
/// key: the details describe the store, not the secret.
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

/// An error crosses to the desktop app's frontend as its message.
impl Serialize for SecretError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A credential store, holding one key for each key variable, such as
/// `OPENAI_API_KEY`.
pub trait Store: Send + Sync {
    /// The key stored for `variable`, or `None` when none is.
    ///
    /// # Errors
    ///
    /// When the store is unavailable or fails.
    fn read(&self, variable: &str) -> Result<Option<SecretString>, SecretError>;

    /// Store `key` for `variable`, replacing any key stored for it.
    ///
    /// # Errors
    ///
    /// When the store is unavailable or fails.
    fn write(&self, variable: &str, key: &SecretString) -> Result<(), SecretError>;

    /// Remove the key stored for `variable`. A key not stored is removed
    /// already.
    ///
    /// # Errors
    ///
    /// When the store is unavailable or fails.
    fn delete(&self, variable: &str) -> Result<(), SecretError>;
}

/// The keys in a [`Store`], then in the process environment. An empty key
/// counts as none.
#[derive(Debug, Clone, Copy, Default)]
pub struct Keychain<S>(pub S);

impl<S: Store> Keychain<S> {
    /// Where the key for `variable` is: in the store, in the environment,
    /// or nowhere. A store that fails is unavailable, unless the
    /// environment holds the key.
    pub fn status(&self, variable: &str) -> KeyStatus {
        status(self.0.read(variable), || Environment.key(variable))
    }

    /// Store `key` for `variable`, replacing any key stored for it.
    ///
    /// # Errors
    ///
    /// When the store is unavailable or fails.
    pub fn store(&self, variable: &str, key: &SecretString) -> Result<(), SecretError> {
        self.0.write(variable, key)
    }

    /// Remove the key stored for `variable`, if there is one.
    ///
    /// # Errors
    ///
    /// When the store is unavailable or fails.
    pub fn remove(&self, variable: &str) -> Result<(), SecretError> {
        self.0.delete(variable)
    }
}

impl<S: Store> Keys for Keychain<S> {
    fn key(&self, variable: &str) -> anyhow::Result<Option<SecretString>> {
        found(variable, self.0.read(variable), || {
            Environment.key(variable)
        })
    }
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
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A store in memory, which fails every call once it has an error.
    #[derive(Default)]
    pub(crate) struct Fake {
        keys: Mutex<HashMap<String, String>>,
        error: Mutex<Option<SecretError>>,
    }

    impl Fake {
        /// Fail every call from now on with `error`.
        fn fail(&self, error: SecretError) {
            *self.error.lock().unwrap() = Some(error);
        }

        /// The error to fail with, if there is one.
        fn failed(&self) -> Result<(), SecretError> {
            self.error.lock().unwrap().clone().map_or(Ok(()), Err)
        }
    }

    impl Store for Fake {
        fn read(&self, variable: &str) -> Result<Option<SecretString>, SecretError> {
            self.failed()?;
            let keys = self.keys.lock().unwrap();
            Ok(keys
                .get(variable)
                .map(|key| SecretString::from(key.as_str())))
        }

        fn write(&self, variable: &str, key: &SecretString) -> Result<(), SecretError> {
            self.failed()?;
            let key = key.expose_secret().to_string();
            self.keys.lock().unwrap().insert(variable.to_string(), key);
            Ok(())
        }

        fn delete(&self, variable: &str) -> Result<(), SecretError> {
            self.failed()?;
            self.keys.lock().unwrap().remove(variable);
            Ok(())
        }
    }

    /// `key` as a secret.
    fn secret(key: &str) -> SecretString {
        SecretString::from(key)
    }

    /// A failure of the store.
    fn locked() -> SecretError {
        SecretError::Backend("locked".to_string())
    }

    /// A store that is not there.
    fn unavailable() -> SecretError {
        SecretError::StoreUnavailable("no Secret Service".to_string())
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

    /// No key from the environment.
    fn unset() -> anyhow::Result<Option<SecretString>> {
        Ok(None)
    }

    #[test]
    fn a_secret_errors_display_never_carries_a_key() {
        assert_eq!(locked().to_string(), "credential store error: locked");
        assert!(
            unavailable()
                .to_string()
                .starts_with("the OS credential store is unavailable")
        );
    }

    #[test]
    fn a_store_unavailable_message_names_the_environment_variable_to_set() {
        assert!(unavailable().to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn a_secret_error_crosses_as_its_message() {
        assert_eq!(
            serde_json::to_value(locked()).unwrap(),
            "credential store error: locked"
        );
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
        assert_eq!(
            exposed(Err(locked()), Some("environment")),
            Some("environment".to_string())
        );
    }

    #[test]
    fn the_keychain_falls_through_to_the_environment_when_the_store_has_no_key() {
        let keychain = Keychain(Fake::default());
        let path = std::env::var("PATH").unwrap();
        let key = keychain.key("PATH").unwrap().unwrap();
        assert_eq!(key.expose_secret(), path);
        let unset = "SOCIAL_DEDUCTION_TEST_UNSET_KEY";
        assert!(keychain.key(unset).unwrap().is_none());
        keychain.store("PATH", &secret("stored")).unwrap();
        let key = keychain.key("PATH").unwrap().unwrap();
        assert_eq!(key.expose_secret(), "stored");
    }

    #[test]
    fn a_key_is_stored_found_and_removed() {
        let keychain = Keychain(Fake::default());
        let variable = "SOCIAL_DEDUCTION_TEST_UNSET_KEY";
        assert_eq!(keychain.status(variable), KeyStatus::Missing);
        keychain.store(variable, &secret("sk-test")).unwrap();
        assert_eq!(keychain.status(variable), KeyStatus::InKeychain);
        keychain.remove(variable).unwrap();
        keychain.remove(variable).unwrap();
        assert_eq!(keychain.status(variable), KeyStatus::Missing);
        // Cargo sets this variable.
        assert_eq!(keychain.status("CARGO_PKG_NAME"), KeyStatus::InEnvironment);
    }

    #[test]
    fn a_key_s_status_is_where_it_is_and_never_the_key() {
        let in_store = || Ok(Some(secret("sk-test")));
        assert_eq!(status(in_store(), unset), KeyStatus::InKeychain);
        assert_eq!(status(Ok(None), unset), KeyStatus::Missing);
        let environment = || Ok(Some(secret("sk-environment")));
        assert_eq!(status(Ok(None), environment), KeyStatus::InEnvironment);
        // An empty key counts as none.
        assert_eq!(
            status(Ok(Some(secret(""))), || Ok(Some(secret("")))),
            KeyStatus::Missing
        );
        assert_eq!(
            status(Ok(None), || Err(anyhow::anyhow!("not UTF-8"))),
            KeyStatus::Missing
        );
    }

    #[test]
    fn a_store_that_fails_is_unavailable_unless_the_environment_holds_the_key() {
        let keychain = Keychain(Fake::default());
        keychain.0.fail(unavailable());
        let status = keychain.status("SOCIAL_DEDUCTION_TEST_UNSET_KEY");
        assert!(
            matches!(&status, KeyStatus::Unavailable(shown) if shown.contains("OPENAI_API_KEY")),
            "{status:?}"
        );
        assert_eq!(keychain.status("CARGO_PKG_NAME"), KeyStatus::InEnvironment);
        assert_eq!(keychain.store("A", &secret("b")), Err(unavailable()));
        assert_eq!(keychain.remove("A"), Err(unavailable()));
    }

    /// A key no test or message has any other reason to hold.
    const SENTINEL: &str = "sk-test-sentinel";

    /// What every event logged at any level while `act` runs says, as
    /// text.
    fn logged(act: impl FnOnce()) -> String {
        use std::io::Write;
        use std::sync::Arc;
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(bytes)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.lock().unwrap().flush()
            }
        }
        let mut captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, act);
        captured.flush().unwrap();
        let bytes = captured.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn a_key_never_leaks_into_a_log_an_error_or_a_status() {
        let keychain = Keychain(Fake::default());
        let variable = "SOCIAL_DEDUCTION_SENTINEL_KEY";
        let mut shown = Vec::new();
        let log = logged(|| {
            keychain.store(variable, &secret(SENTINEL)).unwrap();
            shown.push(format!("{:?}", keychain.status(variable)));
            shown.push(format!("{:?}", keychain.0.read(variable)));
            shown.push(format!("{:?}", keychain.key(variable)));
            // A store that fails, on the way in and on the way out.
            keychain.0.fail(unavailable());
            let error = keychain.store(variable, &secret(SENTINEL)).unwrap_err();
            shown.push(format!("{error} {error:?}"));
            let error = keychain.0.read(variable).unwrap_err();
            shown.push(format!("{error} {error:?}"));
            shown.push(format!("{:?}", keychain.status(variable)));
            shown.push(format!("{:?}", keychain.key(variable)));
            let found = found(variable, Err(error), || Ok(Some(secret(SENTINEL))));
            shown.push(format!("{found:?}"));
        });
        assert!(log.contains("could not read"), "{log}");
        assert!(!log.contains(SENTINEL), "{log}");
        for shown in shown {
            assert!(!shown.contains(SENTINEL), "{shown}");
        }
    }
}
