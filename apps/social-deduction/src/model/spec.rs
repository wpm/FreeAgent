//! What every game shares in setting up its models: the [`Spec`] a
//! `[models.<name>]` table describes, and [`set_up`], which turns each
//! model in play into the [`ModelInPlay`] the log records and the handle
//! its players ask it through.
//!
//! The API key never appears in the file. The file names the environment
//! variable that holds it, and [`Spec::api_key`] reads that variable into
//! a [`SecretString`], which is redacted wherever it is shown.

use super::window::{self, ContextWindow};
use super::{Model, Provider, REQUEST_TIMEOUT, ToolChoice, api_key, key_variable};
use anyhow::Context as _;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// A `[models.<name>]` table: one model from one provider, and how to
/// reach it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// The root of an OpenAI-compatible API, ending in `/v1`.
    pub base_url: String,
    /// The model's id, exactly as the provider lists it.
    pub id: String,
    /// The environment variable holding the API key, over the one known
    /// for the provider.
    pub api_key_env: Option<String>,
    /// How long a request may take.
    #[serde(default = "request_timeout", with = "humantime_serde")]
    pub request_timeout: Duration,
    /// The model's context window, in tokens, over what the provider
    /// reports.
    pub context_window: Option<u64>,
}

impl Spec {
    /// The environment variable holding the API key: `api_key_env` when
    /// the file names one, else the one known for the provider, if any.
    pub fn key_variable(&self) -> Option<&str> {
        key_variable(&self.base_url, self.api_key_env.as_deref())
    }

    /// The API key, read from the variable [`Spec::key_variable`] names.
    /// A variable that is named but not set is an error. When none is
    /// named there is no key, which is what a local server expects.
    pub fn api_key(&self) -> anyhow::Result<Option<SecretString>> {
        self.key_variable().map(api_key).transpose()
    }

    /// The provider the model is reached through, with its key read from
    /// the environment as [`Spec::api_key`] reads it.
    pub fn provider(&self) -> anyhow::Result<Provider> {
        Provider::new(&self.base_url, self.api_key()?, self.request_timeout)
    }
}

/// The default `request_timeout`, for `serde`.
fn request_timeout() -> Duration {
    REQUEST_TIMEOUT
}

/// A model in play, as it was set up. The API key is never recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInPlay {
    /// The root of the provider's OpenAI-compatible API.
    pub base_url: String,
    /// The model's id, as the provider lists it.
    pub id: String,
    /// How long each request could take.
    #[serde(with = "humantime_serde")]
    pub request_timeout: Duration,
    /// The name of the environment variable the key came from, if any.
    pub api_key_env: Option<String>,
    /// Whether each request made the model call a tool, as the probe
    /// found the model would.
    pub tool_choice: ToolChoice,
    /// The model's context window and where it came from.
    pub context_window: ContextWindow,
}

/// The models in play as the log records them, by name.
pub type Models = BTreeMap<String, ModelInPlay>;

/// The handle each model in play is asked through, by name.
pub type Handles = BTreeMap<String, Arc<dyn Model>>;

/// What a game's probe found when it asked a model to play.
#[derive(Debug)]
pub enum Probed {
    /// Asked as a player is, the model answered every request as a player
    /// must, under the tool choice it is to be asked under.
    Played(ToolChoice),
    /// One of the requests the model did not answer as a player must: the
    /// provider refused it as bad, or the answer was not well formed. The
    /// error names the model, the game and the request, and carries the cause
    /// beneath it.
    DidNotPlay(anyhow::Error),
}

/// Set up each model `in_play`, by name, one at a time, since a local
/// server loads each model it is asked for: read its key, find it in its
/// provider's listing, settle its context window against `needed`, the
/// tokens the game's largest request to it takes before what the player
/// remembers, and
/// `probe` it, which settles the tool choice. Each, by name, as the log
/// records it, and the handle its players ask it through. A model that
/// cannot be set up is an error naming it.
pub async fn set_up(
    in_play: BTreeMap<&str, &Spec>,
    needed: impl Fn(&str) -> anyhow::Result<u64>,
    probe: impl AsyncFn(&Provider, &str) -> anyhow::Result<Probed>,
) -> anyhow::Result<(Models, Handles)> {
    let mut models = Models::new();
    let mut handles = Handles::new();
    for (name, spec) in in_play {
        let set_up = async {
            let provider = spec.provider()?;
            let listed = provider.listed(&spec.id).await?;
            let context_window = window::settle(name, spec, listed.context_window, needed(name)?)?;
            let tool_choice = match probe(&provider, &spec.id).await? {
                Probed::Played(tool_choice) => tool_choice,
                Probed::DidNotPlay(error) => return Err(error),
            };
            let played = ModelInPlay {
                base_url: spec.base_url.clone(),
                id: spec.id.clone(),
                request_timeout: spec.request_timeout,
                api_key_env: spec.key_variable().map(String::from),
                tool_choice,
                context_window,
            };
            anyhow::Ok((played, provider))
        };
        let (played, provider) = set_up
            .await
            .with_context(|| format!("setting up the model {name}"))?;
        models.insert(name.to_string(), played);
        handles.insert(name.to_string(), Arc::new(provider));
    }
    Ok((models, handles))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::CONTEXT_WINDOW;
    use crate::model::canned::{playing, playing_within, serving};
    use crate::model::window::Source;
    use anyhow::anyhow;
    use secrecy::ExposeSecret;
    use std::sync::Mutex;

    /// The `[models.qwen]` table at `base_url`, with `rest` after its id.
    pub(crate) fn qwen(base_url: &str, rest: &str) -> Spec {
        let table = format!("base_url = \"{base_url}\"\nid = \"qwen2.5-7b-instruct\"\n{rest}");
        toml::from_str(&table).unwrap()
    }

    /// A local server's root.
    pub(crate) const LOCAL: &str = "http://localhost:1234/v1";

    /// A provider `src/models.toml` knows.
    pub(crate) const OPENAI: &str = "https://api.openai.com/v1";

    #[test]
    fn the_request_timeout_comes_from_the_table_or_the_code() {
        assert_eq!(qwen(LOCAL, "").request_timeout, REQUEST_TIMEOUT);
        assert_eq!(
            qwen(LOCAL, "request_timeout = \"10s\"").request_timeout,
            Duration::from_secs(10)
        );
    }

    #[test]
    fn an_unknown_key_or_a_duration_the_table_cannot_parse_is_an_error() {
        for rest in [
            "temperature = 0.5",
            "request_timeout = \"soon\"",
            "request_timeout = 10",
        ] {
            let table = format!("base_url = \"{LOCAL}\"\nid = \"qwen\"\n{rest}");
            assert!(toml::from_str::<Spec>(&table).is_err(), "{rest}");
        }
    }

    #[test]
    fn an_omitted_key_variable_means_no_key() {
        assert!(qwen(LOCAL, "").api_key().unwrap().is_none());
    }

    #[test]
    fn the_key_is_read_from_the_named_variable_and_never_shown() {
        // A variable Cargo sets for every test, since setting one is unsafe.
        let name = "CARGO_MANIFEST_DIR";
        let value = std::env::var(name).unwrap();
        let spec = qwen(LOCAL, &format!("api_key_env = \"{name}\""));
        let key = spec.api_key().unwrap().unwrap();
        assert_eq!(key.expose_secret(), value);
        let shown = format!("{key:?}\n{spec:?}");
        assert!(shown.contains(name), "{shown}");
        assert!(!shown.contains(&value), "{shown}");
    }

    #[test]
    fn a_known_provider_s_key_variable_is_filled_in_unless_the_table_names_one() {
        assert_eq!(qwen(OPENAI, "").key_variable(), Some("OPENAI_API_KEY"));
        let named = qwen(OPENAI, "api_key_env = \"MY_KEY\"");
        assert_eq!(named.key_variable(), Some("MY_KEY"));
    }

    #[tokio::test]
    async fn each_model_in_play_is_set_up_in_name_order_with_what_it_needs_and_the_probe_finds() {
        let qwen_url = playing(&["qwen2.5-7b-instruct"]);
        let claude_url = playing(&["claude-sonnet-4-5"]);
        let claude: Spec = toml::from_str(&format!(
            "base_url = \"{claude_url}\"\nid = \"claude-sonnet-4-5\"\ncontext_window = 8192"
        ))
        .unwrap();
        let qwen = qwen(&qwen_url, "");
        let in_play = BTreeMap::from([("qwen", &qwen), ("claude", &claude)]);
        let asked = Mutex::new(Vec::new());
        let needed = |name: &str| {
            asked.lock().unwrap().push(format!("needed {name}"));
            Ok(100)
        };
        let probe = async |_: &Provider, id: &str| {
            asked.lock().unwrap().push(format!("probed {id}"));
            let tool_choice = match id {
                "claude-sonnet-4-5" => ToolChoice::Auto,
                _ => ToolChoice::Required,
            };
            Ok(Probed::Played(tool_choice))
        };
        let (models, handles) = set_up(in_play, needed, probe).await.unwrap();
        assert_eq!(
            *asked.lock().unwrap(),
            [
                "needed claude",
                "probed claude-sonnet-4-5",
                "needed qwen",
                "probed qwen2.5-7b-instruct"
            ]
        );
        assert_eq!(
            models,
            Models::from([
                (
                    "claude".to_string(),
                    ModelInPlay {
                        base_url: claude_url,
                        id: "claude-sonnet-4-5".to_string(),
                        request_timeout: REQUEST_TIMEOUT,
                        api_key_env: None,
                        tool_choice: ToolChoice::Auto,
                        context_window: ContextWindow {
                            tokens: 8192,
                            source: Source::File,
                        },
                    }
                ),
                (
                    "qwen".to_string(),
                    ModelInPlay {
                        base_url: qwen_url,
                        id: "qwen2.5-7b-instruct".to_string(),
                        request_timeout: REQUEST_TIMEOUT,
                        api_key_env: None,
                        tool_choice: ToolChoice::Required,
                        context_window: ContextWindow {
                            tokens: CONTEXT_WINDOW,
                            source: Source::Default,
                        },
                    }
                ),
            ])
        );
        assert_eq!(handles.keys().collect::<Vec<_>>(), ["claude", "qwen"]);
        // No model in play sets up nothing.
        let (models, handles) = set_up(BTreeMap::new(), needed, probe).await.unwrap();
        assert!(models.is_empty() && handles.is_empty());
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_set_up_is_an_error_naming_it() {
        // `needed` of `None` is a game that cannot say what the model needs.
        // A model that is probed does not play.
        let shown = async |spec: &Spec, needed: Option<u64>| {
            let probe =
                async |_: &Provider, _: &str| Ok(Probed::DidNotPlay(anyhow!("it did not play")));
            let in_play = BTreeMap::from([("qwen", spec)]);
            let needed = |_: &str| needed.ok_or_else(|| anyhow!("no prompt"));
            let error = set_up(in_play, needed, probe).await.err().unwrap();
            format!("{error:#}")
        };
        // Not served.
        let spec = qwen(&serving(&["llama-3.1-8b-instruct"]), "");
        let error = shown(&spec, Some(0)).await;
        assert!(error.starts_with("setting up the model qwen: "), "{error}");
        // Its key not set.
        let spec = qwen(LOCAL, "api_key_env = \"SOCIAL_DEDUCTION_NO_SUCH_KEY\"");
        let error = shown(&spec, Some(0)).await;
        assert!(error.starts_with("setting up the model qwen: "), "{error}");
        assert!(error.contains("SOCIAL_DEDUCTION_NO_SUCH_KEY"), "{error}");
        // Not playing.
        let spec = qwen(&playing(&["qwen2.5-7b-instruct"]), "");
        let error = shown(&spec, Some(0)).await;
        assert_eq!(error, "setting up the model qwen: it did not play");
        // Its window too small.
        let spec = qwen(&playing_within(&["qwen2.5-7b-instruct"], 100), "");
        let error = shown(&spec, Some(101)).await;
        assert!(
            error.starts_with(
                "setting up the model qwen: the context window the provider reports for qwen is 100 tokens"
            ),
            "{error}"
        );
        // What it needs not found.
        let spec = qwen(&playing(&["qwen2.5-7b-instruct"]), "");
        let error = shown(&spec, None).await;
        assert_eq!(error, "setting up the model qwen: no prompt");
    }
}
