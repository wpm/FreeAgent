//! Reaching a model provider: the OpenAI-compatible API every model is
//! behind, the providers known, and the [`Model`] a player asks for one
//! decision. Every Free Agent app whose players are language models
//! reaches them through this crate.
//!
//! Nothing here is particular to any game. An app's `models` command lists
//! what a [`Provider`] serves, and a model-played game checks that its
//! model is in the listing before it begins. Each decision is one
//! [`Request`] to the provider, forcing a tool call, and the [`Response`]
//! it is answered with. [`estimated`] counts what a request takes of the
//! model's context window, [`MAX_TOKENS`] for the reply included. A player
//! depends only on the [`Model`] trait, so a test gives it a fake model and
//! no test reaches a model. The fake and the canned providers a test talks
//! to come with the `test-support` feature.
//!
//! Each app probes a model by asking it to play the app's own games:
//! [`check`] runs the probe the app gives it, and [`catalog`] probes every
//! model a provider lists. API keys are kept in a [`secret`] store.
//!
//! The API key is a [`SecretString`], exposed only where the
//! `Authorization` header is built, so it never appears in an error.

#![warn(missing_docs)]

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod canned;
pub mod catalog;
pub mod check;
mod completion;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod fake;
pub mod secret;
pub mod spec;
pub mod window;

pub use completion::*;

use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::sync::LazyLock;
use std::time::Duration;

/// How long a request may take unless the configuration says otherwise.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The context window, in tokens, of a model nothing else gives one for:
/// a size most models can run in.
pub const CONTEXT_WINDOW: u64 = 4096;

/// The tokens the context window keeps for the reply.
pub const MAX_TOKENS: u64 = 1024;

/// The tokens a chat template adds around each message, by estimate.
pub const MESSAGE_TOKENS: u64 = 8;

/// The tokens a chat template adds around each tool, by estimate.
pub const TOOL_TOKENS: u64 = 32;

/// How many tokens `text` takes, by estimate: one for every 3
/// characters, rounded up. A provider offers no tokenizer, and an estimate
/// over the mark is safer than one under it.
pub fn estimate(text: &str) -> u64 {
    tokens(text.chars().count())
}

/// How many tokens a text of `chars` characters takes, by the estimate
/// [`estimate`] makes.
pub fn tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(3)
}

/// How many tokens `request` takes of the context window, by estimate:
/// each message and its [`MESSAGE_TOKENS`], the tools as JSON and
/// [`TOOL_TOKENS`] for each, and [`MAX_TOKENS`] for the reply.
pub fn estimated(request: &Request) -> u64 {
    let messages: u64 = request
        .messages
        .iter()
        .map(|message| estimate(&message.content) + MESSAGE_TOKENS)
        .sum();
    let tools = serde_json::to_string(&request.tools).unwrap_or_default();
    messages + estimate(&tools) + TOOL_TOKENS * request.tools.len() as u64 + MAX_TOKENS
}

/// How much of what a provider said about a failure an error repeats.
const SAID_AT_MOST: usize = 500;

/// A model asked for one decision at a time.
///
/// A call is one decision, not a turn in a conversation: each [`Request`]
/// carries everything the model is shown. A [`Provider`] is a model over
/// HTTP, and the fake model of the `test-support` feature one for tests.
#[async_trait]
pub trait Model: Send + Sync {
    /// One completion: send `request`, return the model's response.
    async fn complete(&self, request: &Request) -> anyhow::Result<Response>;
}

/// What the client knows, compiled in from `src/models.toml`. Adding to
/// it is an edit there and a rebuild.
const MODELS: &str = include_str!("models.toml");

/// What `src/models.toml` says, parsed once. The file is part of the
/// source, so one that does not parse is a build broken at its first use.
static KNOWLEDGE: LazyLock<Knowledge> =
    LazyLock::new(|| toml::from_str(MODELS).unwrap_or_else(|e| panic!("src/models.toml: {e}")));

/// The shape of `src/models.toml`. An unknown key is an error, so a
/// misspelled one fails loudly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Knowledge {
    /// The providers known, in the order the help lists them.
    #[serde(default)]
    providers: Vec<Known>,
    /// The fields a model's entry in a provider's listing may carry its
    /// context window under, in the order they are read.
    #[serde(default)]
    context_window_fields: Vec<String>,
    /// The ports a local server is looked for on.
    #[serde(default)]
    local_ports: Vec<u16>,
    /// Words in a model's id that say it does not chat.
    #[serde(default)]
    hide: Vec<String>,
    /// The families of models, most capable first.
    #[serde(default)]
    families: Vec<String>,
}

/// A provider `src/models.toml` knows.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Known {
    /// Its name, as the desktop app shows it.
    name: String,
    /// The root of its OpenAI-compatible API, ending in `/v1`.
    base_url: String,
    /// The environment variable conventionally holding its API key, for a
    /// provider that wants one.
    api_key_env: Option<String>,
    /// The headers every request to it must carry.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// The context window, in tokens, of the models it serves, for a
    /// listing that reports none.
    context_window: Option<u64>,
}

/// The provider `src/models.toml` knows at `base_url`: the one whose root
/// the base URL is at or under, if any.
fn known(base_url: &str) -> Option<&'static Known> {
    let base_url = base_url.trim_end_matches('/');
    KNOWLEDGE.providers.iter().find(|known| {
        let root = known.base_url.trim_end_matches('/');
        base_url == root
            || base_url
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The providers `src/models.toml` knows, each as its name, the root of
/// its API and the environment variable conventionally holding its key, if
/// any, in the file's order, for telling the user.
pub fn known_providers() -> Vec<(&'static str, &'static str, Option<&'static str>)> {
    KNOWLEDGE
        .providers
        .iter()
        .map(|known| {
            let (name, base_url) = (known.name.as_str(), known.base_url.as_str());
            (name, base_url, known.api_key_env.as_deref())
        })
        .collect()
}

/// What an app's `models --help` ends with: a table of the providers
/// known, the root of each one's API and the environment variable its key
/// is read from unless `--api-key-env` names another.
pub fn known_providers_help() -> String {
    let providers = known_providers();
    let width = providers
        .iter()
        .map(|(_, root, _)| root.len())
        .max()
        .unwrap_or_default();
    let mut help = String::from(
        "Providers known, with the environment variable their API key is read from:\n",
    );
    for (_, root, variable) in providers {
        writeln!(help, "  {root:width$}  {}", variable.unwrap_or("-")).unwrap();
    }
    help
}

/// The ports `src/models.toml` says a local server is looked for on.
pub fn local_ports() -> &'static [u16] {
    &KNOWLEDGE.local_ports
}

/// Whether the model `id` does not chat, as a word `src/models.toml` hides
/// in it says, whatever its case.
pub fn hidden(id: &str) -> bool {
    let id = id.to_lowercase();
    KNOWLEDGE.hide.iter().any(|word| id.contains(word.as_str()))
}

/// The families of models `src/models.toml` ranks, most capable first,
/// each the start of its models' ids: `claude-opus`, `gpt-4.1`.
pub fn families() -> &'static [String] {
    &KNOWLEDGE.families
}

/// The headers every request to the provider at `base_url` must carry,
/// when `src/models.toml` knows the provider and it needs any.
fn known_headers(base_url: &str) -> Vec<(&'static str, &'static str)> {
    known(base_url)
        .map(|known| {
            known
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// The environment variable conventionally holding the API key for the
/// provider at `base_url`, when `src/models.toml` knows the provider and
/// it wants one.
pub fn known_key_variable(base_url: &str) -> Option<&'static str> {
    known(base_url)?.api_key_env.as_deref()
}

/// The context window, in tokens, of the models the provider at
/// `base_url` serves, when `src/models.toml` knows the provider and gives
/// one.
pub fn known_context_window(base_url: &str) -> Option<u64> {
    known(base_url)?.context_window
}

/// The environment variable holding the API key for the provider at
/// `base_url`: the one `named`, if any, over the one known for the
/// provider, if any. None means no key, which is what a local server
/// expects.
pub fn key_variable<'a>(base_url: &str, named: Option<&'a str>) -> Option<&'a str> {
    named.or_else(|| known_key_variable(base_url))
}

/// A request the provider answered with anything but success: the status,
/// kept so a reader can tell a request the provider would not put to the
/// model from one that never got that far, what the provider said, and
/// the error's words.
#[derive(Debug)]
pub struct Rejection {
    /// The status the provider answered with.
    pub status: StatusCode,
    /// What the provider said about it.
    pub said: Said,
    message: String,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Rejection {}

/// What a provider said about a failure: the start of its words, if it
/// said any, whether they are a JSON error's message, its own account of
/// the failure, and that error's code and type, if it gave them.
#[derive(Debug, Default, PartialEq)]
pub struct Said {
    /// The start of its words, if it said any.
    pub words: Option<String>,
    /// Whether the words are a JSON error's message, its own account.
    pub accounted: bool,
    /// The error's code, if it gave one.
    pub code: Option<String>,
    /// The error's type, if it gave one.
    pub kind: Option<String>,
}

impl Said {
    /// What the provider said in `text`, the body of its answer. A JSON
    /// error in OpenAI's and Anthropic's shape gives its `message`, its
    /// `code` and its `type`; any other text is its words, trimmed, and
    /// nothing is nothing.
    fn read(text: &str) -> Self {
        #[derive(Deserialize)]
        struct Body {
            error: Error,
        }
        #[derive(Deserialize)]
        struct Error {
            message: String,
            code: Option<serde_json::Value>,
            r#type: Option<serde_json::Value>,
        }
        let text = text.trim();
        if text.is_empty() {
            return Self::default();
        }
        let named = |value: Option<serde_json::Value>| value?.as_str().map(String::from);
        match serde_json::from_str::<Body>(text) {
            Ok(Body { error }) => Self {
                words: Some(start(&error.message)),
                accounted: true,
                code: named(error.code),
                kind: named(error.r#type),
            },
            Err(_) => Self {
                words: Some(start(text)),
                ..Self::default()
            },
        }
    }

    /// Whether the provider classified the failure as `name`, by its code
    /// or its type.
    pub fn is(&self, name: &str) -> bool {
        [&self.code, &self.kind]
            .into_iter()
            .any(|given| given.as_deref() == Some(name))
    }
}

/// Whether `error`, from a completion, concerns the model rather than the
/// way to it. A request the provider refused as bad, 400, is one it would
/// not put to the model, as a server that cannot use the tools a request
/// offers answers, and an answer that is not a selection is the model's
/// own. A request that never reached the provider, or that it answered
/// with any other status, says nothing about the model.
pub fn concerns_the_model(error: &anyhow::Error) -> bool {
    let reached = error
        .chain()
        .all(|cause| cause.downcast_ref::<reqwest::Error>().is_none());
    reached && rejected_with(error).is_none_or(|status| status == StatusCode::BAD_REQUEST)
}

/// Whether `error`, from a completion, is the provider refusing the
/// request as bad, 400: refusing its shape, such as a tool choice the
/// model does not accept, whatever the provider's words for it.
pub fn refused(error: &anyhow::Error) -> bool {
    rejected_with(error) == Some(StatusCode::BAD_REQUEST)
}

/// The status the provider answered with, if `error` is the provider
/// answering a request with anything but success.
pub fn rejected_with(error: &anyhow::Error) -> Option<StatusCode> {
    rejection(error).map(|rejection| rejection.status)
}

/// The provider's answer, if `error` is the provider answering a request
/// with anything but success.
pub fn rejection(error: &anyhow::Error) -> Option<&Rejection> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

/// The code a provider gives an account out of credits.
pub const NO_CREDITS: &str = "insufficient_quota";

/// A status that is not success, in words: the status, what it is likely
/// to mean coming from a provider that was sent a key or not, and what
/// the provider `said` about it. Words that are the provider's own account
/// of the failure come first and the meaning of the status after;
/// otherwise the meaning comes first. Only a rate limit, a 429 with no
/// code or one saying so, is to be tried again shortly: OpenAI answers 429
/// to an account out of credits too.
fn explained(status: StatusCode, keyed: bool, said: &Said) -> String {
    let limited =
        said.code.is_none() || said.is("rate_limit_exceeded") || said.is("rate_limit_error");
    let meaning = match status {
        _ if said.is(NO_CREDITS) => {
            Some("the account has no credits left; add credits to continue")
        }
        StatusCode::UNAUTHORIZED if keyed => Some("the API key was not accepted"),
        StatusCode::UNAUTHORIZED => Some("it wants an API key"),
        StatusCode::FORBIDDEN => Some("the API key is not allowed to do that"),
        StatusCode::NOT_FOUND => Some(
            "nothing is served at that path; the base URL is the root of the API, ending in /v1",
        ),
        StatusCode::TOO_MANY_REQUESTS if limited => {
            Some("it is limiting the rate of requests; try again shortly")
        }
        StatusCode::TOO_MANY_REQUESTS => Some("it is turning requests away"),
        status if status.is_server_error() => {
            Some("it is having trouble of its own; try again shortly")
        }
        _ => None,
    };
    let mut explanation = status.to_string();
    match (&said.words, meaning) {
        (Some(words), meaning) if said.accounted => {
            let _ = write!(explanation, ": {words}");
            if let Some(meaning) = meaning {
                let _ = write!(explanation, " ({meaning})");
            }
        }
        (words, meaning) => {
            if let Some(meaning) = meaning {
                let _ = write!(explanation, ": {meaning}");
            }
            if let Some(words) = words {
                let _ = write!(explanation, ". The provider said: {words}");
            }
        }
    }
    explanation
}

/// The start of `text`: all of it when it is short, else its first
/// [`SAID_AT_MOST`] characters and an ellipsis.
fn start(text: &str) -> String {
    match text.char_indices().nth(SAID_AT_MOST) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// A model provider: the root of its OpenAI-compatible API, ending in
/// `/v1`, the key it wants, if any, and the client its requests go through.
#[derive(Debug)]
pub struct Provider {
    base_url: String,
    api_key: Option<SecretString>,
    client: reqwest::Client,
}

/// The listing `GET /v1/models` answers with. Only the ids and the
/// context windows are read, and other fields are ignored, so a provider's
/// additions do not break it.
#[derive(Deserialize)]
struct Listing {
    data: Vec<Listed>,
}

/// One model as a provider lists it: its id, its context window, in
/// tokens, when its entry carries one, and when it was made, when its
/// entry says. The window is the first of the fields `src/models.toml`
/// names that holds a positive whole number. When it was made is OpenAI's
/// `created`, in seconds since 1970, or Anthropic's `created_at`, a date.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "Entry")]
pub struct Listed {
    /// The model's id.
    pub id: String,
    /// The model's context window, in tokens, if its entry says.
    pub context_window: Option<u64>,
    /// When the model was made, in seconds since 1970, if its entry says.
    pub created: Option<u64>,
}

/// A model's entry in a [`Listing`], as written.
#[derive(Deserialize)]
struct Entry {
    id: String,
    #[serde(flatten)]
    fields: serde_json::Map<String, serde_json::Value>,
}

impl From<Entry> for Listed {
    fn from(entry: Entry) -> Self {
        let context_window = KNOWLEDGE.context_window_fields.iter().find_map(|name| {
            entry
                .fields
                .get(name)?
                .as_u64()
                .filter(|&tokens| tokens > 0)
        });
        let created = entry
            .fields
            .get("created")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| {
                let date = entry.fields.get("created_at")?.as_str()?;
                let made = humantime::parse_rfc3339_weak(date).ok()?;
                Some(made.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
            });
        Self {
            id: entry.id,
            context_window,
            created,
        }
    }
}

impl Provider {
    /// The provider at `base_url`, sent `api_key` as a bearer token when
    /// there is one, whose every request may take `request_timeout` and
    /// carries the headers `src/models.toml` says the provider needs.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        request_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let base_url = base_url.into();
        let headers = known_headers(&base_url);
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
                .with_context(|| format!("the header {name} in src/models.toml"))?;
            let value = HeaderValue::from_str(value)
                .with_context(|| format!("the header {name} in src/models.toml"))?;
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

    /// The URL of `path` under the root of the API, followed by the base
    /// URL's query and fragment.
    fn url(&self, path: &str) -> String {
        let end = self
            .base_url
            .find(['?', '#'])
            .unwrap_or(self.base_url.len());
        let (root, query) = self.base_url.split_at(end);
        format!("{}/{path}{query}", root.trim_end_matches('/'))
    }

    /// What the provider answers `request`, described as `sent`, such as
    /// `GET {url}`, carrying the key as a bearer token when there is one.
    /// A request that fails is an error naming what was sent; one answered
    /// with anything but success an error naming what was sent, the
    /// status, and whatever the provider said about it; and an answer that
    /// is not the shape expected an error naming what was sent.
    async fn answered<T: DeserializeOwned>(
        &self,
        mut request: reqwest::RequestBuilder,
        sent: &str,
    ) -> anyhow::Result<T> {
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose_secret());
        }
        let response = request.send().await.with_context(|| sent.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let said = Said::read(&response.text().await.unwrap_or_default());
            return Err(Rejection {
                status,
                message: format!(
                    "{sent} was answered {}",
                    explained(status, self.api_key.is_some(), &said)
                ),
                said,
            }
            .into());
        }
        response
            .json()
            .await
            .with_context(|| format!("reading what {sent} answered"))
    }

    /// The ids of the models the provider serves, as `GET {base_url}/models`
    /// lists them. A request that fails is an error naming the URL, and one
    /// answered with anything but success an error naming the URL, the
    /// status, and whatever the provider said about it.
    pub async fn models(&self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .listing()
            .await?
            .into_iter()
            .map(|model| model.id)
            .collect())
    }

    /// The models the provider serves, as `GET {base_url}/models` lists
    /// them, failing as [`Provider::models`] does.
    pub async fn listing(&self) -> anyhow::Result<Vec<Listed>> {
        let url = self.url("models");
        let listing: Listing = self
            .answered(self.client.get(&url), &format!("GET {url}"))
            .await?;
        Ok(listing.data)
    }

    /// The model `id` as the provider lists it, checking that the provider
    /// serves it. A model it does not list is an error naming the provider
    /// and the model.
    pub async fn listed(&self, id: &str) -> anyhow::Result<Listed> {
        served(&self.base_url, &self.listing().await?, id).cloned()
    }
}

/// The model `id` in `listing`, the models the provider at `base_url`
/// serves. A model not listed is an error naming the provider and the
/// model.
pub fn served<'a>(base_url: &str, listing: &'a [Listed], id: &str) -> anyhow::Result<&'a Listed> {
    listing
        .iter()
        .find(|model| model.id == id)
        .ok_or_else(|| anyhow!("{base_url} does not serve a model with the id {id}"))
}

/// The provider as a model: a completion is `POST {base_url}/chat/completions`.
#[async_trait]
impl Model for Provider {
    /// A request that fails or is answered with anything but success is
    /// an error as [`Provider::models`] gives one, and so is a response
    /// with no choices.
    async fn complete(&self, request: &Request) -> anyhow::Result<Response> {
        let url = self.url("chat/completions");
        let sent = format!("POST {url}");
        let response: Response = self
            .answered(self.client.post(&url).json(request), &sent)
            .await?;
        if response.choices.is_empty() {
            bail!("{sent} was answered with no choices");
        }
        Ok(response)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::canned::{self, answering, bound, completion, listing, reported, unreachable};
    use super::spec::Probed;
    use super::*;
    use serde_json::{Value, json};

    /// Long enough for a request to a listener in this process.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// A request to select one of `candidates`: its one tool, `select`,
    /// takes a `target` from an `enum` of them.
    pub(crate) fn selecting(candidates: &[&str]) -> Request {
        Request::offering(
            "qwen2.5-7b-instruct",
            vec![
                Message::system("You are Player 1."),
                Message::user("Select."),
            ],
            vec![Tool::new(
                "select",
                "Knowing everything you know, select the best one.",
                json!({
                    "type": "object",
                    "properties": {"target": {"type": "string", "enum": candidates}},
                    "required": ["target"],
                }),
            )],
            ToolChoice::Required,
        )
    }

    /// The calls the crate's canned player makes to answer `request`, in
    /// the protocol's JSON: the first tool offered, called with the first
    /// value its first enumerated parameter allows. A request offering no
    /// tool, or one with nothing to choose from, is an error.
    pub(crate) fn first_choice(request: &Value) -> anyhow::Result<Vec<(String, Value)>> {
        let tool = &request["tools"][0]["function"];
        let properties = tool["parameters"]["properties"]
            .as_object()
            .ok_or_else(|| anyhow!("the request offers no tool"))?;
        let (parameter, first) = properties
            .iter()
            .find_map(|(name, schema)| Some((name, schema["enum"].get(0)?)))
            .ok_or_else(|| anyhow!("{} has nothing to choose from", tool["name"]))?;
        let name = tool["name"].as_str().unwrap_or_default().to_string();
        Ok(vec![(name, json!({parameter: first}))])
    }

    /// The crate's canned player's answer to `request`, as a response.
    pub(crate) fn chosen(request: &Request) -> anyhow::Result<Response> {
        Ok(Response::calling(&first_choice(&serde_json::to_value(
            request,
        )?)?))
    }

    /// The tests' probe: one request to select, first made to call a tool,
    /// as a game called Chess would ask it.
    pub(crate) async fn plays(model: &dyn Model, id: &str) -> anyhow::Result<Probed> {
        let ask = async |tool_choice| {
            let request = Request {
                model: id.to_string(),
                tool_choice,
                ..selecting(&["a", "b"])
            };
            model.complete(&request).await.map(drop)
        };
        let (tool_choice, answered) = check::first_answered(ToolChoice::Required, ask).await;
        let failed = answered
            .err()
            .map(|error| ("a selection".to_string(), error));
        check::probed(id, "Chess", tool_choice, failed)
    }

    #[test]
    fn the_canned_player_calls_the_first_tool_with_its_first_choice_or_fails() {
        assert_eq!(
            chosen(&selecting(&["b", "a"])).unwrap(),
            Response::tool_call("select", &json!({"target": "b"}))
        );
        let error = chosen(&selecting(&[])).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "\"select\" has nothing to choose from"
        );
        let error = first_choice(&json!({"tools": []})).unwrap_err();
        assert_eq!(format!("{error:#}"), "the request offers no tool");
    }

    #[test]
    fn a_text_is_estimated_at_a_token_for_every_three_characters_rounded_up() {
        assert_eq!(estimate(""), 0);
        assert_eq!(estimate("a"), 1);
        assert_eq!(estimate("abc"), 1);
        assert_eq!(estimate("abcd"), 2);
        assert_eq!(estimate("abcdef"), 2);
        assert_eq!(estimate("abcdefg"), 3);
        // Characters, not bytes.
        assert_eq!(estimate("éééééé"), 2);
    }

    #[test]
    fn a_request_is_estimated_at_its_messages_and_tools_each_with_an_allowance_and_the_reply() {
        let request = selecting(&["Player 2", "Player 3"]);
        let messages: u64 = request
            .messages
            .iter()
            .map(|message| estimate(&message.content) + MESSAGE_TOKENS)
            .sum();
        let tools = estimate(&serde_json::to_string(&request.tools).unwrap()) + TOOL_TOKENS;
        assert_eq!(estimated(&request), messages + tools + MAX_TOKENS);
        assert_eq!(MAX_TOKENS, 1024);
        // Nothing asked is the reply alone, and an empty list of tools.
        let nothing = Request {
            messages: Vec::new(),
            tools: Vec::new(),
            ..request
        };
        assert_eq!(estimated(&nothing), estimate("[]") + MAX_TOKENS);
    }

    #[test]
    fn a_known_provider_s_default_context_window_is_read_from_the_file() {
        assert_eq!(
            known_context_window("https://api.openai.com/v1"),
            Some(128000)
        );
        assert_eq!(
            known_context_window("https://api.anthropic.com/v1/"),
            Some(200000)
        );
        assert_eq!(known_context_window("http://localhost:1234/v1"), None);
    }

    #[test]
    fn the_known_providers_are_told_in_the_file_s_order() {
        let providers = known_providers();
        assert!(providers.len() >= 2, "{providers:?}");
        assert_eq!(
            providers[0],
            (
                "OpenAI",
                "https://api.openai.com/v1",
                Some("OPENAI_API_KEY")
            )
        );
        assert_eq!(
            providers[1],
            (
                "Anthropic",
                "https://api.anthropic.com/v1",
                Some("ANTHROPIC_API_KEY")
            )
        );
    }

    #[test]
    fn the_known_providers_help_is_a_table_whose_variables_line_up() {
        let table = known_providers_help();
        let rows: Vec<&str> = table.lines().skip(1).collect();
        assert!(
            rows.contains(&"  https://api.openai.com/v1     OPENAI_API_KEY"),
            "{table}"
        );
        assert!(
            rows.contains(&"  https://api.anthropic.com/v1  ANTHROPIC_API_KEY"),
            "{table}"
        );
        // The variables line up, whatever the roots' lengths.
        let columns: std::collections::HashSet<Option<usize>> =
            rows.iter().map(|row| row.rfind("  ")).collect();
        assert_eq!(columns.len(), 1, "{table}");
    }

    #[test]
    fn a_local_server_is_looked_for_on_the_ports_of_the_usual_servers() {
        assert_eq!(local_ports(), [1234, 11434, 8080, 8000]);
    }

    #[test]
    fn a_model_that_does_not_chat_is_hidden_by_a_word_in_its_id_whatever_its_case() {
        for id in [
            "text-embedding-3-small",
            "tts-1",
            "whisper-1",
            "gpt-4o-transcribe",
            "dall-e-3",
            "gpt-image-1",
            "omni-moderation-latest",
            "gpt-4o-realtime-preview",
            "gpt-4o-audio-preview",
            "Text-Embedding-Nomic",
        ] {
            assert!(hidden(id), "{id}");
        }
        for id in ["gpt-4.1-mini", "claude-sonnet-4-5", "qwen2.5-7b-instruct"] {
            assert!(!hidden(id), "{id}");
        }
    }

    #[test]
    fn the_file_ranks_model_families_most_capable_first() {
        let place = |family: &str| families().iter().position(|f| f == family).unwrap();
        assert!(place("claude-fable") < place("claude-opus"));
        assert!(place("claude-opus") < place("claude-sonnet"));
        assert!(place("claude-sonnet") < place("claude-haiku"));
        assert!(place("gpt-4.1") < place("gpt-4.1-mini"));
    }

    #[test]
    fn a_model_s_creation_is_read_from_its_entry_in_seconds_or_as_a_date() {
        let created = |fields: serde_json::Value| {
            let mut model = json!({"id": "m"});
            model
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            entry(model).created
        };
        assert_eq!(
            created(json!({"created": 1_700_000_000})),
            Some(1_700_000_000)
        );
        assert_eq!(
            created(json!({"created_at": "2025-02-19T00:00:00Z"})),
            Some(1_739_923_200)
        );
        assert_eq!(created(json!({"created_at": "yesterday"})), None);
        assert_eq!(created(json!({"created": "soon"})), None);
        assert_eq!(created(json!({})), None);
    }

    #[test]
    fn the_file_parses_and_knows_at_least_one_provider() {
        assert!(!KNOWLEDGE.providers.is_empty());
    }

    #[test]
    fn a_known_provider_s_headers_are_read_from_the_file() {
        assert_eq!(
            known_headers("https://api.anthropic.com/v1"),
            [("anthropic-version", "2023-06-01")]
        );
        assert!(known_headers("https://api.openai.com/v1").is_empty());
        assert!(known_headers("http://localhost:1234/v1").is_empty());
    }

    #[tokio::test]
    async fn the_listing_request_carries_the_provider_s_headers() {
        let (base_url, served) = answering("200 OK", &listing(&[], None));
        let headers = [("anthropic-version", "2023-06-01"), ("x-extra", "yes")];
        let provider = Provider::with_headers(base_url, None, TIMEOUT, &headers).unwrap();
        provider.models().await.unwrap();
        let head = served.join().unwrap().head;
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
            format!("{error:#}").contains("src/models.toml"),
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
    fn a_path_goes_under_the_base_url_s_root_and_before_its_query() {
        let url = |base_url: &str| {
            Provider::new(base_url, None, TIMEOUT)
                .unwrap()
                .url("chat/completions")
        };
        assert_eq!(url("https://host/v1/"), "https://host/v1/chat/completions");
        assert_eq!(
            url("https://host/v1?api-version=2024-06-01"),
            "https://host/v1/chat/completions?api-version=2024-06-01"
        );
        assert_eq!(
            url("https://host/v1/?api-version=2024-06-01#top"),
            "https://host/v1/chat/completions?api-version=2024-06-01#top"
        );
    }

    #[tokio::test]
    async fn the_listing_of_a_base_url_with_a_query_is_asked_with_that_query() {
        let (base_url, served) = answering("200 OK", &listing(&["a"], None));
        let provider = Provider::new(format!("{base_url}?api-version=1"), None, TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["a"]);
        let head = served.join().unwrap().head;
        assert_eq!(head[0], "get /v1/models?api-version=1 http/1.1");
    }

    #[tokio::test]
    async fn the_listing_is_asked_of_the_models_path_without_a_key_when_none_is_configured() {
        let (base_url, served) = answering("200 OK", &listing(&["b", "a"], None));
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        let models = provider.models().await.unwrap();
        assert_eq!(models, ["b", "a"]);
        let head = served.join().unwrap().head;
        assert_eq!(head[0], "get /v1/models http/1.1");
        assert!(
            !head.iter().any(|line| line.starts_with("authorization:")),
            "{head:?}"
        );
    }

    #[tokio::test]
    async fn the_listing_request_carries_the_key_as_a_bearer_token() {
        let (base_url, served) = answering("200 OK", &listing(&["a"], None));
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url, Some(key), TIMEOUT).unwrap();
        provider.models().await.unwrap();
        let head = served.join().unwrap().head;
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
                "GET {base_url}/models was answered 401 Unauthorized: Incorrect API key provided \
                 (the API key was not accepted)"
            )
        );
        assert!(!shown.contains("sk-secret"), "{shown}");
    }

    /// What `status` and `said` are explained as, from a provider sent a
    /// key or not.
    fn explanation(status: &str, keyed: bool, said: &str) -> String {
        let status = StatusCode::from_bytes(status.split(' ').next().unwrap().as_bytes()).unwrap();
        explained(status, keyed, &Said::read(said))
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
            "403 Forbidden: the API key is not allowed to do that"
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
        // OpenAI's and Anthropic's shape, the provider's own account.
        assert_eq!(
            explanation(
                "418 I'm a teapot",
                false,
                r#"{"type": "error", "error": {"type": "api_error", "message": "Brewing"}}"#
            ),
            "418 I'm a teapot: Brewing"
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

    #[test]
    fn an_account_out_of_credits_is_explained_by_the_provider_s_words_first_and_not_as_a_rate_limit()
     {
        assert_eq!(
            explanation("429 Too Many Requests", true, canned::NO_CREDITS),
            "429 Too Many Requests: You have no credits remaining. Add credits to continue using \
             the API at https://platform.openai.com/settings/organization/billing/. \
             (the account has no credits left; add credits to continue)"
        );
    }

    #[test]
    fn only_a_rate_limit_is_to_be_tried_again_shortly() {
        let limited = r#"{"error": {"message": "Rate limit reached", "type": "requests", "param": null, "code": "rate_limit_exceeded"}}"#;
        assert_eq!(
            explanation("429 Too Many Requests", true, limited),
            "429 Too Many Requests: Rate limit reached \
             (it is limiting the rate of requests; try again shortly)"
        );
        // Anthropic's, which carries a type and no code.
        let limited =
            r#"{"type": "error", "error": {"type": "rate_limit_error", "message": "Slow down"}}"#;
        assert_eq!(
            explanation("429 Too Many Requests", true, limited),
            "429 Too Many Requests: Slow down (it is limiting the rate of requests; try again shortly)"
        );
        // OpenAI's with no code, which carries a type.
        let limited = r#"{"error": {"message": "Slow down", "type": "requests", "code": null}}"#;
        assert_eq!(
            explanation("429 Too Many Requests", true, limited),
            "429 Too Many Requests: Slow down (it is limiting the rate of requests; try again shortly)"
        );
        // A 429 with a code of another kind is not.
        let other = r#"{"error": {"message": "Not now", "code": "busy"}}"#;
        assert_eq!(
            explanation("429 Too Many Requests", true, other),
            "429 Too Many Requests: Not now (it is turning requests away)"
        );
    }

    #[test]
    fn what_the_provider_said_in_a_json_error_comes_before_the_meaning_of_the_status() {
        let missing = r#"{"error": {"message": "The model `gpt-9` does not exist", "type": "invalid_request_error", "code": "model_not_found"}}"#;
        assert_eq!(
            explanation("404 Not Found", true, missing),
            "404 Not Found: The model `gpt-9` does not exist (nothing is served at that path; \
             the base URL is the root of the API, ending in /v1)"
        );
        // A status with no meaning is followed by the words alone.
        let brewing = r#"{"error": {"message": "Brewing", "code": "teapot"}}"#;
        assert_eq!(
            explanation("418 I'm a teapot", false, brewing),
            "418 I'm a teapot: Brewing"
        );
    }

    #[test]
    fn what_the_provider_said_at_length_is_repeated_only_at_its_start() {
        let long = "x".repeat(SAID_AT_MOST + 1);
        let shown = explanation("418 I'm a teapot", false, &long);
        assert!(shown.ends_with("…"), "{shown}");
        assert_eq!(shown.chars().filter(|c| *c == 'x').count(), SAID_AT_MOST);
        let exact = "y".repeat(SAID_AT_MOST);
        assert!(explanation("418 I'm a teapot", false, &exact).ends_with(&exact));
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
        let (base_url, _served) =
            answering("200 OK", &listing(&["a", "qwen2.5-7b-instruct"], None));
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        assert_eq!(
            provider.listed("qwen2.5-7b-instruct").await.unwrap(),
            Listed {
                id: "qwen2.5-7b-instruct".to_string(),
                context_window: None,
                created: None,
            }
        );
        let (base_url, _served) =
            answering("200 OK", &listing(&["a", "qwen2.5-7b-instruct"], None));
        let provider = Provider::new(base_url.clone(), None, TIMEOUT).unwrap();
        let error = provider.listed("qwen").await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains(&base_url), "{shown}");
        assert!(shown.contains("qwen"), "{shown}");
    }

    /// The model a listing's `entry` describes.
    fn entry(entry: serde_json::Value) -> Listed {
        serde_json::from_value(entry).unwrap()
    }

    #[test]
    fn the_file_names_the_fields_a_context_window_is_read_from() {
        assert_eq!(
            KNOWLEDGE.context_window_fields,
            ["max_model_len", "context_length"]
        );
    }

    #[test]
    fn a_model_s_context_window_is_the_first_named_field_holding_a_positive_whole_number() {
        let window = |fields: serde_json::Value| {
            let mut model = json!({"id": "m", "object": "model"});
            model
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            entry(model).context_window
        };
        assert_eq!(window(json!({"max_model_len": 32768})), Some(32768));
        assert_eq!(window(json!({"context_length": 8192})), Some(8192));
        assert_eq!(
            window(json!({"context_length": 8192, "max_model_len": 32768})),
            Some(32768)
        );
        assert_eq!(
            window(json!({"max_model_len": 0, "context_length": 8192})),
            Some(8192)
        );
        assert_eq!(window(json!({})), None);
        assert_eq!(window(json!({"context_window": 8192})), None);
        for unusable in [json!(0), json!(-1), json!(1.5), json!("8192"), json!(null)] {
            assert_eq!(window(json!({"max_model_len": unusable})), None);
        }
    }

    #[tokio::test]
    async fn a_listed_model_carries_the_context_window_its_entry_reports() {
        let said = json!({"data": [
            {"id": "a", "context_length": 8192},
            {"id": "b", "max_model_len": 32768},
        ]});
        let (base_url, _served) = answering("200 OK", &said.to_string());
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        assert_eq!(
            provider.listed("b").await.unwrap(),
            Listed {
                id: "b".to_string(),
                context_window: Some(32768),
                created: None,
            }
        );
    }

    #[tokio::test]
    async fn a_completion_is_posted_to_the_chat_completions_path_as_json_without_a_key_when_none_is_configured()
     {
        let (base_url, served) = answering(
            "200 OK",
            &completion("select", &json!({"target": "Player 2"})),
        );
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        let request = selecting(&["Player 2", "Player 3"]);
        let response = provider.complete(&request).await.unwrap();
        assert_eq!(
            response,
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
        let served = served.join().unwrap();
        let head = &served.head;
        assert_eq!(head[0], "post /v1/chat/completions http/1.1");
        assert!(
            head.contains(&"content-type: application/json".to_string()),
            "{head:?}"
        );
        assert!(
            !head.iter().any(|line| line.starts_with("authorization:")),
            "{head:?}"
        );
        assert_eq!(served.json(), serde_json::to_value(&request).unwrap());
    }

    #[tokio::test]
    async fn a_completion_request_carries_the_key_as_a_bearer_token() {
        let (base_url, served) = answering(
            "200 OK",
            &completion("select", &json!({"target": "Player 2"})),
        );
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url, Some(key), TIMEOUT).unwrap();
        provider.complete(&selecting(&["Player 2"])).await.unwrap();
        let head = served.join().unwrap().head;
        assert!(
            head.contains(&"authorization: bearer sk-secret".to_string()),
            "{head:?}"
        );
    }

    #[tokio::test]
    async fn a_completion_answered_with_anything_but_success_is_an_error_naming_the_url_and_the_status_not_the_key()
     {
        let (base_url, _served) = answering("429 Too Many Requests", "Rate limit reached");
        let key = SecretString::from("sk-secret");
        let provider = Provider::new(base_url.clone(), Some(key), TIMEOUT).unwrap();
        let error = provider
            .complete(&selecting(&["Player 2"]))
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert_eq!(
            shown,
            format!(
                "POST {base_url}/chat/completions was answered 429 Too Many Requests: \
                 it is limiting the rate of requests; try again shortly. \
                 The provider said: Rate limit reached"
            )
        );
        assert!(!shown.contains("sk-secret"), "{shown}");
    }

    #[tokio::test]
    async fn an_error_concerns_the_model_unless_the_provider_was_not_reached_or_answered_anything_but_400()
     {
        // An answer that is no selection is the model's own.
        assert!(concerns_the_model(&anyhow!("the model made no tool call")));
        // A request refused as bad is one the provider would not put to
        // the model; any other status says nothing about it.
        for (status, answered, expected) in [
            (StatusCode::BAD_REQUEST, "400 Bad Request", true),
            (StatusCode::UNAUTHORIZED, "401 Unauthorized", false),
            (
                StatusCode::TOO_MANY_REQUESTS,
                "429 Too Many Requests",
                false,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "500 Internal Server Error",
                false,
            ),
        ] {
            let (base_url, _served) = answering(answered, "{}");
            let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
            let error = provider
                .complete(&selecting(&["Player 2"]))
                .await
                .unwrap_err();
            assert_eq!(error.downcast_ref::<Rejection>().unwrap().status, status);
            assert_eq!(
                concerns_the_model(&error.context("asked")),
                expected,
                "{answered}"
            );
        }
        // Nor does a provider that was not reached.
        let provider = Provider::new(unreachable(), None, TIMEOUT).unwrap();
        let error = provider
            .complete(&selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(!concerns_the_model(&error), "{error:#}");
    }

    #[tokio::test]
    async fn a_completion_takes_no_longer_than_the_timeout() {
        // A provider that accepts and never answers.
        let (listener, base_url) = bound();
        let _held = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::park();
            drop(stream);
        });
        let provider = Provider::new(base_url, None, Duration::from_millis(100)).unwrap();
        let started = std::time::Instant::now();
        let error = provider
            .complete(&selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(started.elapsed() < TIMEOUT, "{error:#}");
    }

    #[tokio::test]
    async fn a_completion_with_no_choices_is_an_error_naming_the_url() {
        let (base_url, _served) = answering("200 OK", r#"{"choices": []}"#);
        let provider = Provider::new(base_url.clone(), None, TIMEOUT).unwrap();
        let error = provider
            .complete(&selecting(&["Player 2"]))
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(
            shown.contains(&format!("{base_url}/chat/completions")),
            "{shown}"
        );
        assert!(shown.contains("no choices"), "{shown}");
    }

    #[tokio::test]
    async fn a_completion_that_is_not_the_shape_expected_is_an_error() {
        let (base_url, _served) = answering("200 OK", r#"{"data": []}"#);
        let provider = Provider::new(base_url, None, TIMEOUT).unwrap();
        assert!(provider.complete(&selecting(&["Player 2"])).await.is_err());
    }
}
