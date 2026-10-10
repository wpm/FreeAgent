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
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
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

/// The providers the client knows, compiled in: on each line the root of
/// a provider's API, ending in /v1, the environment variable conventionally holding its
/// key, and the headers its requests must carry, with blank lines and
/// lines starting with `#` ignored. Adding a provider is an edit to the
/// file and a rebuild.
const MODEL_URLS: &str = include_str!("model_urls.txt");

/// A provider `src/model_urls.txt` knows.
#[derive(Debug, PartialEq)]
struct Known {
    /// The root of its API.
    root: &'static str,
    /// The environment variable conventionally holding its API key, for a
    /// provider that wants one.
    key_variable: Option<&'static str>,
    /// The headers every request to it must carry.
    headers: Vec<(&'static str, &'static str)>,
}

/// Every provider `src/model_urls.txt` knows, in the file's order.
fn knowns() -> impl Iterator<Item = Known> {
    MODEL_URLS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut columns = line.split_whitespace();
            let root = columns.next().unwrap_or_default().trim_end_matches('/');
            let key_variable = columns.next().filter(|variable| *variable != "-");
            let headers = columns
                .filter_map(|header| header.split_once(':'))
                .collect();
            Known {
                root,
                key_variable,
                headers,
            }
        })
}

/// The provider `src/model_urls.txt` knows at `base_url`: the one whose
/// root the base URL is at or under, if any.
fn known(base_url: &str) -> Option<Known> {
    let base_url = base_url.trim_end_matches('/');
    knowns().find(|known| {
        base_url == known.root
            || base_url
                .strip_prefix(known.root)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The providers `src/model_urls.txt` knows, each as the root of its API
/// and the environment variable conventionally holding its key, if any,
/// in the file's order, for telling the user.
pub fn known_providers() -> Vec<(&'static str, Option<&'static str>)> {
    knowns()
        .map(|known| (known.root, known.key_variable))
        .collect()
}

/// The environment variable conventionally holding the API key for the
/// provider at `base_url`, when `src/model_urls.txt` knows the provider and
/// it wants one.
pub fn known_key_variable(base_url: &str) -> Option<&'static str> {
    known(base_url)?.key_variable
}

/// The environment variable holding the API key for the provider at
/// `base_url`: the one `named`, if any, over the one known for the
/// provider, if any. None means no key, which is what a local server
/// expects.
pub fn key_variable<'a>(base_url: &str, named: Option<&'a str>) -> Option<&'a str> {
    named.or_else(|| known_key_variable(base_url))
}

/// The API key the environment variable `name` holds. A variable that is
/// not set, or is set to something that is not UTF-8, is an error naming
/// it.
pub fn api_key(name: &str) -> anyhow::Result<SecretString> {
    key(name, std::env::var_os(name))
}

/// The key the environment variable `name` holds as `value`.
fn key(name: &str, value: Option<OsString>) -> anyhow::Result<SecretString> {
    let value = value.ok_or_else(|| anyhow!("{name} is not set"))?;
    let value = value
        .into_string()
        .map_err(|_| anyhow!("{name} is set to something that is not UTF-8"))?;
    Ok(SecretString::from(value))
}

/// A status that is not success, in words: the status, what it is likely
/// to mean coming from a provider that was sent a key or not, and
/// whatever the provider `said` about it. A provider's message comes as
/// `error.message` in JSON, as OpenAI's and Anthropic's do; any other
/// text is quoted as it is.
fn explained(status: StatusCode, keyed: bool, said: &str) -> String {
    let meaning = match status {
        StatusCode::UNAUTHORIZED if keyed => Some("the API key was not accepted"),
        StatusCode::UNAUTHORIZED => Some("it wants an API key"),
        StatusCode::FORBIDDEN => Some("the API key is not allowed to list models"),
        StatusCode::NOT_FOUND => Some(
            "nothing is served at that path; the base URL is the root of the API, ending in /v1",
        ),
        StatusCode::TOO_MANY_REQUESTS => {
            Some("it is limiting the rate of requests; try again shortly")
        }
        status if status.is_server_error() => {
            Some("it is having trouble of its own; try again shortly")
        }
        _ => None,
    };
    let mut explanation = status.to_string();
    if let Some(meaning) = meaning {
        explanation.push_str(": ");
        explanation.push_str(meaning);
    }
    if let Some(said) = message(said) {
        explanation.push_str(". The provider said: ");
        explanation.push_str(&said);
    }
    explanation
}

/// What a provider said about a failure: the message of a JSON error
/// in OpenAI's and Anthropic's shape, or else its text, trimmed, or
/// nothing when it said nothing.
fn message(said: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Said {
        error: Error,
    }
    #[derive(Deserialize)]
    struct Error {
        message: String,
    }
    let said = said.trim();
    if said.is_empty() {
        return None;
    }
    Some(match serde_json::from_str::<Said>(said) {
        Ok(json) => json.error.message,
        Err(_) => said.to_string(),
    })
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
    /// there is one, whose every request may take `request_timeout` and
    /// carries the headers `src/model_urls.txt` says the provider needs.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        request_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let base_url = base_url.into();
        let headers = known(&base_url)
            .map(|known| known.headers)
            .unwrap_or_default();
        Self::with_headers(base_url, api_key, request_timeout, &headers)
    }

    /// [`Provider::new`], with `headers` on every request. A header the
    /// file misspells is an error naming the file.
    fn with_headers(
        base_url: String,
        api_key: Option<SecretString>,
        request_timeout: Duration,
        headers: &[(&str, &str)],
    ) -> anyhow::Result<Self> {
        let mut sent = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("the header {name} in src/model_urls.txt"))?;
            let value = HeaderValue::from_str(value)
                .with_context(|| format!("the header {name} in src/model_urls.txt"))?;
            sent.insert(name, value);
        }
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .default_headers(sent)
            .build()?;
        Ok(Self {
            base_url,
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
            bail!(
                "GET {url} was answered {}",
                explained(status, self.api_key.is_some(), &said)
            );
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
    fn the_known_providers_are_told_in_the_file_s_order() {
        let providers = known_providers();
        assert!(providers.len() >= 2, "{providers:?}");
        assert_eq!(
            providers[0],
            ("https://api.openai.com/v1", Some("OPENAI_API_KEY"))
        );
        assert_eq!(
            providers[1],
            ("https://api.anthropic.com/v1", Some("ANTHROPIC_API_KEY"))
        );
    }

    #[test]
    fn a_known_provider_s_headers_are_read_from_the_file() {
        assert_eq!(
            known("https://api.anthropic.com/v1").unwrap().headers,
            [("anthropic-version", "2023-06-01")]
        );
        assert!(
            known("https://api.openai.com/v1")
                .unwrap()
                .headers
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_listing_request_carries_the_provider_s_headers() {
        let (base_url, served) = answering("200 OK", &listing(&[]));
        let headers = [("anthropic-version", "2023-06-01"), ("x-extra", "yes")];
        let provider = Provider::with_headers(base_url, None, TIMEOUT, &headers).unwrap();
        provider.models().await.unwrap();
        let head = lines(&served.join().unwrap());
        assert!(
            head.contains(&"anthropic-version: 2023-06-01".to_string()),
            "{head:?}"
        );
        assert!(head.contains(&"x-extra: yes".to_string()), "{head:?}");
    }

    #[test]
    fn a_header_the_file_misspells_is_an_error_naming_the_file() {
        let bad = [("no spaces allowed", "x")];
        let error =
            Provider::with_headers("http://localhost/v1".into(), None, TIMEOUT, &bad).unwrap_err();
        assert!(
            format!("{error:#}").contains("src/model_urls.txt"),
            "{error:#}"
        );
    }

    #[test]
    fn a_known_provider_s_key_variable_is_found_from_a_base_url_at_or_under_its_root() {
        assert_eq!(
            known_key_variable("https://api.openai.com/v1"),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            known_key_variable("https://api.openai.com/v1/"),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            known_key_variable("https://api.anthropic.com/v1"),
            Some("ANTHROPIC_API_KEY")
        );
        assert_eq!(known_key_variable("http://localhost:1234/v1"), None);
        // Under the root, not merely starting with it, nor above it.
        assert_eq!(
            known_key_variable("https://api.openai.com.example/v1"),
            None
        );
        assert_eq!(known_key_variable("https://api.openai.com"), None);
    }

    #[test]
    fn a_named_key_variable_is_used_over_the_known_one() {
        let openai = "https://api.openai.com/v1";
        assert_eq!(key_variable(openai, None), Some("OPENAI_API_KEY"));
        assert_eq!(key_variable(openai, Some("MY_KEY")), Some("MY_KEY"));
        assert_eq!(key_variable("http://localhost:1234/v1", None), None);
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
        let said = r#"{"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}}"#;
        let (base_url, _served) = answering("401 Unauthorized", said);
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url.clone(), Some(key), TIMEOUT).unwrap();
        let error = provider.models().await.unwrap_err();
        let shown = format!("{error:#}");
        assert_eq!(
            shown,
            format!(
                "GET {base_url}/models was answered 401 Unauthorized: the API key was not accepted. \
                 The provider said: Incorrect API key provided"
            )
        );
        assert!(!shown.contains("sk-secret"), "{shown}");
    }

    /// What `status` and `said` are explained as, from a provider sent a
    /// key or not.
    fn explanation(status: &str, keyed: bool, said: &str) -> String {
        let status = StatusCode::from_bytes(status.split(' ').next().unwrap().as_bytes()).unwrap();
        explained(status, keyed, said)
    }

    #[test]
    fn a_status_is_explained_in_words() {
        assert_eq!(
            explanation("401 Unauthorized", false, ""),
            "401 Unauthorized: it wants an API key"
        );
        assert_eq!(
            explanation("401 Unauthorized", true, ""),
            "401 Unauthorized: the API key was not accepted"
        );
        assert_eq!(
            explanation("403 Forbidden", true, ""),
            "403 Forbidden: the API key is not allowed to list models"
        );
        assert_eq!(
            explanation("404 Not Found", false, ""),
            "404 Not Found: nothing is served at that path; the base URL is the root of the API, ending in /v1"
        );
        assert_eq!(
            explanation("429 Too Many Requests", true, ""),
            "429 Too Many Requests: it is limiting the rate of requests; try again shortly"
        );
        assert_eq!(
            explanation("503 Service Unavailable", false, ""),
            "503 Service Unavailable: it is having trouble of its own; try again shortly"
        );
        // A status with no explanation is given as it is.
        assert_eq!(
            explanation("418 I'm a teapot", false, ""),
            "418 I'm a teapot"
        );
    }

    #[test]
    fn what_the_provider_said_is_its_message_when_it_is_json_and_its_text_when_not() {
        // OpenAI's and Anthropic's shape.
        assert_eq!(
            explanation(
                "418 I'm a teapot",
                false,
                r#"{"type": "error", "error": {"type": "api_error", "message": "Brewing"}}"#
            ),
            "418 I'm a teapot. The provider said: Brewing"
        );
        // Anything else is quoted as it is, trimmed; nothing is not quoted.
        assert_eq!(
            explanation("418 I'm a teapot", false, "  <html>Brewing</html>\n"),
            "418 I'm a teapot. The provider said: <html>Brewing</html>"
        );
        assert_eq!(
            explanation("418 I'm a teapot", false, r#"{"message": "Brewing"}"#),
            r#"418 I'm a teapot. The provider said: {"message": "Brewing"}"#
        );
        assert_eq!(
            explanation("418 I'm a teapot", false, " \n"),
            "418 I'm a teapot"
        );
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
