//! Reaching a model provider: the OpenAI-compatible API every model is
//! behind, and the whitelist of models known to make tool calls.
//!
//! Nothing here is particular to Werewolf. The `models` command lists what
//! a [`Provider`] serves, and a model-played game checks its model against
//! the whitelist and the listing before it begins, since a model player
//! makes its selection with a tool call, and a model that cannot make one
//! would silently never select.
//!
//! The API key is a [`SecretString`], exposed only where the
//! `Authorization` header is built, so it never appears in an error.

#[doc(hidden)]
pub mod canned;

use anyhow::{Context, anyhow, bail};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::ffi::OsString;
use std::time::Duration;

/// How long a request may take unless the configuration says otherwise.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The whitelist, compiled in: one model id per line, exactly as the
/// provider lists it, with blank lines and lines starting with `#` ignored.
/// Adding a model is an edit to the file and a rebuild.
const TOOL_MODELS: &str = include_str!("tool_models.txt");

/// The ids of the models known to make tool calls, as `src/tool_models.txt`
/// lists them.
fn tool_models() -> impl Iterator<Item = &'static str> {
    TOOL_MODELS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Whether the model `id` is known to make tool calls: whether it is on
/// the whitelist, matched exactly, whatever its provider.
pub fn makes_tool_calls(id: &str) -> bool {
    tool_models().any(|model| model == id)
}

/// Check that the model `id` is known to make tool calls, which needs no
/// network. The error names the whitelist to add it to.
pub fn check_makes_tool_calls(id: &str) -> anyhow::Result<()> {
    if makes_tool_calls(id) {
        Ok(())
    } else {
        bail!("{id} is not known to make tool calls: it is not listed in src/tool_models.txt")
    }
}

/// The API key the environment variable `name` holds. A variable that is
/// not set, or is set to something that is not UTF-8, is an error naming
/// it.
pub fn api_key(name: &str) -> anyhow::Result<SecretString> {
    key(name, std::env::var_os(name))
}

/// The key the environment variable `name` holds as `value`.
fn key(name: &str, value: Option<OsString>) -> anyhow::Result<SecretString> {
    let value = value.ok_or_else(|| anyhow!("api_key_env names {name}, which is not set"))?;
    let value = value
        .into_string()
        .map_err(|_| anyhow!("api_key_env names {name}, whose value is not UTF-8"))?;
    Ok(SecretString::from(value))
}

/// A model provider: the root of its OpenAI-compatible API, ending in
/// `/v1`, the key it wants, if any, and the client its requests go through.
#[derive(Debug)]
pub struct Provider {
    base_url: String,
    api_key: Option<SecretString>,
    client: reqwest::Client,
}

/// The listing `GET /v1/models` answers with. Only the ids are read, and
/// unknown fields are ignored, so a provider's additions do not break it.
#[derive(Deserialize)]
struct Listing {
    data: Vec<Listed>,
}

/// One model in a [`Listing`].
#[derive(Deserialize)]
struct Listed {
    id: String,
}

impl Provider {
    /// The provider at `base_url`, sent `api_key` as a bearer token when
    /// there is one, whose every request may take `request_timeout`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        request_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()?;
        Ok(Self {
            base_url: base_url.into(),
            api_key,
            client,
        })
    }

    /// The ids of the models the provider serves, as `GET {base_url}/models`
    /// lists them. A request that fails is an error naming the URL, and one
    /// answered with anything but success an error naming the URL, the
    /// status, and whatever the provider said about it.
    pub async fn models(&self) -> anyhow::Result<Vec<String>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let mut request = self.client.get(&url);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose_secret());
        }
        let response = request.send().await.with_context(|| format!("GET {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let said = response.text().await.unwrap_or_default();
            match said.trim() {
                "" => bail!("GET {url} was answered {status}"),
                said => bail!("GET {url} was answered {status}: {said}"),
            }
        }
        let listing: Listing = response
            .json()
            .await
            .with_context(|| format!("reading the listing GET {url} answered"))?;
        Ok(listing.data.into_iter().map(|model| model.id).collect())
    }

    /// Check that the provider serves the model `id`: that it is in the
    /// provider's listing. The error names the provider and the model.
    pub async fn check_serves(&self, id: &str) -> anyhow::Result<()> {
        if self.models().await?.iter().any(|served| served == id) {
            Ok(())
        } else {
            bail!("{} does not serve a model with the id {id}", self.base_url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::canned::{answering, bound, listing, unreachable};
    use super::*;

    /// Long enough for a request to a listener in this process.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// The lines of a request head, lowercased, so a header is found
    /// whatever its case.
    fn lines(head: &str) -> Vec<String> {
        head.lines().map(str::to_lowercase).collect()
    }

    #[test]
    fn the_whitelist_is_not_empty_and_matches_an_id_exactly_as_written() {
        assert!(tool_models().next().is_some());
        assert!(makes_tool_calls("qwen2.5-7b-instruct"));
        assert!(makes_tool_calls("claude-opus-5-5"));
        assert!(makes_tool_calls("gpt-5.5"));
        // Not a comment, a blank line, or another case.
        assert!(!makes_tool_calls("# Models known to make tool calls."));
        assert!(!makes_tool_calls(""));
        assert!(!makes_tool_calls("QWEN2.5-7B-INSTRUCT"));
    }

    #[test]
    fn a_key_variable_that_is_not_set_is_an_error_naming_it() {
        let error = api_key("SOCIAL_DEDUCTION_NO_SUCH_KEY").unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("SOCIAL_DEDUCTION_NO_SUCH_KEY"), "{shown}");
        assert!(shown.contains("not set"), "{shown}");
    }

    #[test]
    fn the_key_is_read_from_the_named_variable_and_never_shown() {
        // A variable Cargo sets for every test, since setting one is unsafe.
        let name = "CARGO_MANIFEST_DIR";
        let value = std::env::var(name).unwrap();
        let key = api_key(name).unwrap();
        assert_eq!(key.expose_secret(), value);
        assert!(!format!("{key:?}").contains(&value));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_variable_whose_value_is_not_utf_8_is_an_error_naming_it() {
        use std::os::unix::ffi::OsStringExt;
        let error = key("SOCIAL_DEDUCTION_KEY", Some(OsString::from_vec(vec![0xff]))).unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("SOCIAL_DEDUCTION_KEY"), "{shown}");
        assert!(!shown.contains("not set"), "{shown}");
    }

    #[test]
    fn a_model_off_the_whitelist_is_an_error_naming_it_and_the_file() {
        check_makes_tool_calls("qwen2.5-7b-instruct").unwrap();
        let error = check_makes_tool_calls("gpt-0").unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("gpt-0"), "{shown}");
        assert!(shown.contains("src/tool_models.txt"), "{shown}");
    }

    #[tokio::test]
    async fn the_listing_is_asked_of_the_models_path_without_a_key_when_none_is_configured() {
        let (base_url, served) = answering("200 OK", &listing(&["b", "a"]));
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        let models = provider.models().await.unwrap();
        assert_eq!(models, ["b", "a"]);
        let head = lines(&served.join().unwrap());
        assert_eq!(head[0], "get /v1/models http/1.1");
        assert!(
            !head.iter().any(|line| line.starts_with("authorization:")),
            "{head:?}"
        );
    }

    #[tokio::test]
    async fn the_listing_request_carries_the_key_as_a_bearer_token() {
        let (base_url, served) = answering("200 OK", &listing(&["a"]));
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url, Some(key), TIMEOUT).unwrap();
        provider.models().await.unwrap();
        let head = lines(&served.join().unwrap());
        assert!(
            head.contains(&"authorization: bearer sk-secret".to_string()),
            "{head:?}"
        );
        assert!(!format!("{provider:?}").contains("sk-secret"));
    }

    #[tokio::test]
    async fn a_status_that_is_not_success_is_an_error_naming_the_url_the_status_and_what_was_said_not_the_key()
     {
        let said = r#"{"error": {"message": "Incorrect API key provided"}}"#;
        let (base_url, _served) = answering("401 Unauthorized", said);
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url.clone(), Some(key), TIMEOUT).unwrap();
        let error = provider.models().await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains(&format!("{base_url}/models")), "{shown}");
        assert!(shown.contains("401"), "{shown}");
        assert!(shown.contains("Incorrect API key provided"), "{shown}");
        assert!(!shown.contains("sk-secret"), "{shown}");
        // A provider that says nothing is not quoted.
        let (base_url, _served) = answering("503 Service Unavailable", "");
        let provider = Provider::new(base_url.clone(), None, TIMEOUT).unwrap();
        let shown = format!("{:#}", provider.models().await.unwrap_err());
        assert!(shown.ends_with("503 Service Unavailable"), "{shown}");
    }

    #[tokio::test]
    async fn a_request_that_fails_is_an_error_naming_the_url() {
        let base_url = unreachable();
        let provider = Provider::new(base_url.clone(), None, TIMEOUT).unwrap();
        let error = provider.models().await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains(&format!("{base_url}/models")), "{shown}");
    }

    #[tokio::test]
    async fn a_request_takes_no_longer_than_the_timeout() {
        // A provider that accepts and never answers.
        let (listener, base_url) = bound();
        let _held = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::park();
            drop(stream);
        });
        let provider = Provider::new(base_url, None, Duration::from_millis(100)).unwrap();
        let started = std::time::Instant::now();
        let error = provider.models().await.unwrap_err();
        assert!(started.elapsed() < TIMEOUT, "{error:#}");
    }

    #[tokio::test]
    async fn a_listing_that_is_not_the_shape_expected_is_an_error() {
        let (base_url, _served) = answering("200 OK", r#"{"models": []}"#);
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        assert!(provider.models().await.is_err());
    }

    #[tokio::test]
    async fn a_model_the_provider_lists_is_served_and_one_it_does_not_is_an_error_naming_both() {
        let (base_url, _served) = answering("200 OK", &listing(&["a", "qwen2.5-7b-instruct"]));
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        provider.check_serves("qwen2.5-7b-instruct").await.unwrap();
        let (base_url, _served) = answering("200 OK", &listing(&["a", "qwen2.5-7b-instruct"]));
        let provider = Provider::new(base_url.clone(), None, TIMEOUT).unwrap();
        let error = provider.check_serves("qwen").await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains(&base_url), "{shown}");
        assert!(shown.contains("qwen"), "{shown}");
    }
}
