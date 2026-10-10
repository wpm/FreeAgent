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

use anyhow::{Context, bail};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::Duration;

/// The whitelist, compiled in: one model id per line, exactly as the
/// provider lists it, with blank lines and lines starting with `#` ignored.
/// Adding a model is an edit to the file and a rebuild.
const TOOL_MODELS: &str = include_str!("tool_models.txt");

/// The ids of the models known to make tool calls, as `src/tool_models.txt`
/// lists them.
pub fn tool_models() -> BTreeSet<&'static str> {
    TOOL_MODELS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// Whether the model `id` is known to make tool calls: whether it is on
/// the whitelist, matched exactly, whatever its provider.
pub fn makes_tool_calls(id: &str) -> bool {
    tool_models().contains(id)
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

/// A model provider: the root of its OpenAI-compatible API, ending in
/// `/v1`, the key it wants, if any, and how long a request may take.
#[derive(Debug)]
pub struct Provider {
    base_url: String,
    api_key: Option<SecretString>,
    request_timeout: Duration,
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
    /// there is one, whose requests may take `request_timeout`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        request_timeout: Duration,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
            request_timeout,
        }
    }

    /// The root of the provider's API.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// How long a request may take.
    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    /// The ids of the models the provider serves, as `GET {base_url}/models`
    /// lists them. A request that fails or is answered with anything but
    /// success is an error naming the URL and the status.
    pub async fn models(&self) -> anyhow::Result<Vec<String>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .timeout(self.request_timeout)
            .build()?;
        let mut request = client.get(&url);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose_secret());
        }
        let response = request.send().await.with_context(|| format!("GET {url}"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("GET {url} was answered {status}");
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
        self.check_listed(&self.models().await?, id)
    }

    /// Check that `id` is in `listing`, what the provider serves.
    fn check_listed(&self, listing: &[String], id: &str) -> anyhow::Result<()> {
        if listing.iter().any(|served| served == id) {
            Ok(())
        } else {
            bail!("{} does not serve a model with the id {id}", self.base_url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::llm::Config;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    /// A provider that answers one request with `status` and `body`, in a
    /// task that gives back the head of the request it was sent.
    async fn canned(status: &str, body: &str) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let served = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut buffer = [0; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0, "the request ended before its head did");
                head.extend_from_slice(&buffer[..n]);
            }
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
            String::from_utf8(head).unwrap()
        });
        (base_url, served)
    }

    /// A listing of `ids` in OpenAI's shape, with the fields a provider
    /// adds that are not read.
    fn listing(ids: &[&str]) -> String {
        let data: Vec<_> = ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "object": "model", "owned_by": "test"}))
            .collect();
        serde_json::json!({"object": "list", "data": data}).to_string()
    }

    /// The lines of a request head, lowercased, so a header is found
    /// whatever its case.
    fn lines(head: &str) -> Vec<String> {
        head.lines().map(str::to_lowercase).collect()
    }

    #[test]
    fn the_whitelist_parses_is_not_empty_and_ignores_comments_and_blank_lines() {
        let models = tool_models();
        assert!(!models.is_empty());
        assert!(
            models
                .iter()
                .all(|id| !id.is_empty() && !id.starts_with('#'))
        );
        assert!(models.iter().all(|id| id.trim() == *id));
        assert!(makes_tool_calls("qwen2.5-7b-instruct"));
        assert!(!makes_tool_calls("# Models known to make tool calls."));
        assert!(!makes_tool_calls(""));
        assert!(!makes_tool_calls("QWEN2.5-7B-INSTRUCT"));
    }

    #[test]
    fn the_whitelist_covers_every_example_s_model() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/werewolf/llm");
        let mut examples = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                let id = Config::load(&path).unwrap().model.id;
                assert!(makes_tool_calls(&id), "{}: {id}", path.display());
                examples += 1;
            }
        }
        assert!(examples > 0, "no examples in {dir}");
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
        let (base_url, served) = canned("200 OK", &listing(&["b", "a"])).await;
        let provider = Provider::new(base_url, None, Duration::from_secs(5));
        let models = provider.models().await.unwrap();
        assert_eq!(models, ["b", "a"]);
        let head = lines(&served.await.unwrap());
        assert_eq!(head[0], "get /v1/models http/1.1");
        assert!(
            !head.iter().any(|line| line.starts_with("authorization:")),
            "{head:?}"
        );
    }

    #[tokio::test]
    async fn the_listing_request_carries_the_key_as_a_bearer_token() {
        let (base_url, served) = canned("200 OK", &listing(&["a"])).await;
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url, Some(key), Duration::from_secs(5));
        provider.models().await.unwrap();
        let head = lines(&served.await.unwrap());
        assert!(
            head.contains(&"authorization: bearer sk-secret".to_string()),
            "{head:?}"
        );
        assert!(!format!("{provider:?}").contains("sk-secret"));
    }

    #[tokio::test]
    async fn a_status_that_is_not_success_is_an_error_naming_the_url_and_the_status_not_the_key() {
        let (base_url, _served) = canned("401 Unauthorized", "{}").await;
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url.clone(), Some(key), Duration::from_secs(5));
        let error = provider.models().await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains(&format!("{base_url}/models")), "{shown}");
        assert!(shown.contains("401"), "{shown}");
        assert!(!shown.contains("sk-secret"), "{shown}");
    }

    #[tokio::test]
    async fn a_request_that_fails_is_an_error_naming_the_url() {
        // A port that was listening and is no more.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let provider = Provider::new(base_url.clone(), None, Duration::from_secs(5));
        let error = provider.models().await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains(&format!("{base_url}/models")), "{shown}");
    }

    #[tokio::test]
    async fn a_request_takes_no_longer_than_the_timeout() {
        // A provider that accepts and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let _held = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });
        let provider = Provider::new(base_url, None, Duration::from_millis(100));
        let started = std::time::Instant::now();
        let error = provider.models().await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "{error:#}");
    }

    #[tokio::test]
    async fn a_listing_that_is_not_the_shape_expected_is_an_error() {
        let (base_url, _served) = canned("200 OK", r#"{"models": []}"#).await;
        let provider = Provider::new(base_url, None, Duration::from_secs(5));
        assert!(provider.models().await.is_err());
    }

    #[test]
    fn a_model_the_provider_lists_is_served_and_one_it_does_not_is_an_error_naming_both() {
        let provider = Provider::new("http://localhost:1234/v1", None, Duration::from_secs(5));
        let listing: Vec<String> = ["a", "qwen2.5-7b-instruct"].map(String::from).to_vec();
        provider
            .check_listed(&listing, "qwen2.5-7b-instruct")
            .unwrap();
        let error = provider.check_listed(&listing, "qwen").unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("http://localhost:1234/v1"), "{shown}");
        assert!(shown.contains("qwen"), "{shown}");
        assert!(provider.check_listed(&[], "a").is_err());
    }

    #[tokio::test]
    async fn the_check_that_a_model_is_served_asks_the_provider_once() {
        let (base_url, served) = canned("200 OK", &listing(&["a"])).await;
        let provider = Provider::new(base_url, None, Duration::from_secs(5));
        provider.check_serves("a").await.unwrap();
        served.await.unwrap();
        let (base_url, _served) = canned("200 OK", &listing(&["a"])).await;
        let provider = Provider::new(base_url, None, Duration::from_secs(5));
        assert!(provider.check_serves("b").await.is_err());
    }
}
