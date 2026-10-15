//! What the desktop app's Models tab offers: the providers a model is
//! reached through, those `src/models.toml` knows and the local servers
//! [`discover`] finds; each provider's [`list`]ing, most capable first, as
//! [`rank`] ranks it; and what probing each listed model found, kept in
//! [`Probes`] so that no model is probed twice.
//!
//! A remote provider, one reached over `https`, has its models probed
//! [`AT_ONCE`] at a time. A local server loads each model it is asked
//! about, so its models are probed one at a time, and [`idle`] holds them
//! back while a game plays. [`probe_each`] runs a provider's probes either
//! way.

use crate::model::check;
use crate::model::{self, Provider as Client, REQUEST_TIMEOUT};
use crate::settings::{Check, Listed, Listing, Provider, Trouble, Unchecked, is_remote};
use futures_util::StreamExt;
use reqwest::StatusCode;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

/// How many of a remote provider's models are probed at once.
pub const AT_ONCE: usize = 4;

/// How long a port is given to answer before no local server is taken to
/// be on it.
pub const KNOCK: Duration = Duration::from_millis(300);

/// The providers `src/models.toml` knows, then a local server on each of
/// `ports`, in order.
pub fn providers(ports: &[u16]) -> Vec<Provider> {
    let known = model::known_providers()
        .into_iter()
        .map(|(name, base_url, api_key_env)| Provider {
            name: name.to_string(),
            base_url: base_url.to_string(),
            api_key_env: api_key_env.map(String::from),
        });
    known
        .chain(ports.iter().map(|port| Provider::local(*port)))
        .collect()
}

/// How many of the models of the provider at `base_url` are probed at
/// once: [`AT_ONCE`] for a remote provider, one for a local one.
pub fn at_once(base_url: &str) -> usize {
    if is_remote(base_url) { AT_ONCE } else { 1 }
}

/// Those of `ports` on which a local server lists its models, each asked
/// at once and given `timeout` to answer, in order.
pub async fn discover(ports: &[u16], timeout: Duration) -> Vec<u16> {
    let knocks = ports.iter().map(|port| async move {
        let client = Client::new(Provider::local(*port).base_url, None, timeout).ok()?;
        client.listing().await.ok().map(|_| *port)
    });
    futures_util::future::join_all(knocks)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// The models the provider at `base_url` lists, asked with `key`, most
/// capable first, as [`rank`] ranks them, each marked hidden when its id
/// says it does not chat and with the context window a game gives it, as
/// [`window`] says; or, when it cannot list them, why, in a line for under
/// the provider.
pub async fn list(base_url: &str, key: Option<SecretString>) -> Listing {
    let listed = async { Client::new(base_url, key, REQUEST_TIMEOUT)?.listing().await };
    match listed.await {
        Ok(mut models) => {
            rank(&mut models);
            Listing {
                models: models
                    .into_iter()
                    .map(|model| Listed {
                        hidden: model::hidden(&model.id),
                        context_window: window(base_url, model.context_window),
                        id: model.id,
                    })
                    .collect(),
                ..Listing::default()
            }
        }
        Err(error) => Listing {
            error: Some(unlisted(base_url, &error)),
            ..Listing::default()
        },
    }
}

/// The context window, in tokens, a game gives a model of the provider at
/// `base_url` whose configuration sets none, as [`model::window::unset`] gives
/// it, from `reported`, the one its listing reports.
pub fn window(base_url: &str, reported: Option<u64>) -> u64 {
    model::window::unset(base_url, reported).tokens
}

/// Where a model stands among its provider's models, which sort most
/// capable first.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Standing {
    /// Of the family at this place in `src/models.toml`'s list, newest
    /// first, and an undated one by its id, a higher version first.
    Family(usize, Reverse<Option<u64>>, Reverse<String>),
    /// Of so many tenths of a billion parameters, the most first.
    Sized(Reverse<u64>, String),
    /// Of neither, newest first.
    Other(Reverse<Option<u64>>, String),
}

/// Rank a provider's `models` most capable first: those of a family
/// `src/models.toml` lists, in its order and newest first within each;
/// then those whose id gives a parameter count, the most first; then the
/// rest, newest first.
pub fn rank(models: &mut [model::Listed]) {
    models.sort_by_cached_key(|model| {
        let (id, made) = (model.id.clone(), Reverse(model.created));
        match (family(&model.id), parameters(&model.id)) {
            (Some(place), _) => Standing::Family(place, made, Reverse(id)),
            (None, Some(count)) => Standing::Sized(Reverse(count), id),
            (None, None) => Standing::Other(made, id),
        }
    });
}

/// The place in `src/models.toml`'s list of the family of the model `id`:
/// one whose name the id is, or starts with before a hyphen and a digit,
/// whatever its case.
fn family(id: &str) -> Option<usize> {
    let id = id.to_lowercase();
    model::families().iter().position(|family| {
        id.strip_prefix(family.as_str()).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .strip_prefix('-')
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
    })
}

/// How many tenths of a billion parameters the model `id` has, as a part
/// of its id of its own says, such as `7b` or `0.5b`: the largest, if
/// several do.
fn parameters(id: &str) -> Option<u64> {
    id.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .filter_map(|part| {
            let number = part.strip_suffix('b')?;
            let (whole, tenths) = number.split_once('.').unwrap_or((number, "0"));
            let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
            if !digits(whole) || !digits(tenths) || tenths.len() > 1 {
                return None;
            }
            Some(whole.parse::<u64>().ok()? * 10 + tenths.parse::<u64>().ok()?)
        })
        .max()
}

/// Why the provider at `base_url` could not list its models, from `error`.
fn unlisted(base_url: &str, error: &anyhow::Error) -> String {
    let unreached = error
        .chain()
        .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some());
    match model::rejected_with(error) {
        Some(StatusCode::UNAUTHORIZED) => {
            "Couldn't list models: 401, check the API key".to_string()
        }
        _ if unreached && !is_remote(base_url) => "No local server answered".to_string(),
        _ => format!("Couldn't list models: {error:#}"),
    }
}

/// What probing the model `id` of the provider at `base_url`, asked with
/// `key`, finds, as `social-deduction models --check` probes it.
///
/// # Errors
///
/// When the model could not be asked, which says nothing about it.
pub async fn probe(base_url: &str, id: &str, key: Option<SecretString>) -> anyhow::Result<Check> {
    let client = Client::new(base_url, key, REQUEST_TIMEOUT)?;
    check::probe(&client, id).await
}

/// Why a model could not be asked to play, from `error`, its probe's:
/// with what the provider said, or the error when it said nothing.
pub fn unchecked(error: &anyhow::Error) -> Unchecked {
    let rejection = model::rejection(error);
    let trouble = match rejection {
        Some(rejection) if rejection.said.is(model::NO_CREDITS) => Trouble::NoCredits,
        Some(rejection) if rejection.status == StatusCode::UNAUTHORIZED => Trouble::KeyRefused,
        Some(rejection) if rejection.status == StatusCode::FORBIDDEN => Trouble::KeyForbidden,
        _ => Trouble::Failed,
    };
    let said = rejection
        .and_then(|rejection| rejection.said.words.clone())
        .unwrap_or_else(|| format!("{error:#}"));
    Unchecked { trouble, said }
}

/// Probe each of `ids` with `probe`, `at_once` at a time, passing each id
/// to `record` with what its probe found, or why it could not be run, as
/// each probe ends. A probe that could not be run is logged, and one the
/// account could not run, as [`Unchecked::account_wide`] says, stops the
/// rest from starting, since each would fail the same way.
pub async fn probe_each<F>(
    ids: Vec<String>,
    at_once: usize,
    probe: impl Fn(String) -> F,
    mut record: impl FnMut(&str, Result<Check, Unchecked>),
) where
    F: Future<Output = anyhow::Result<Check>>,
{
    let stopped = Cell::new(false);
    let mut ended = futures_util::stream::iter(ids)
        .take_while(|_| std::future::ready(!stopped.get()))
        .map(|id| {
            let probed = probe(id.clone());
            async move { (id, probed.await) }
        })
        .buffer_unordered(at_once.max(1));
    while let Some((id, probed)) = ended.next().await {
        let probed = probed.map_err(|error| {
            tracing::warn!("{id} could not be checked: {error:#}");
            unchecked(&error)
        });
        if probed.as_ref().is_err_and(Unchecked::account_wide) {
            stopped.set(true);
        }
        record(&id, probed);
    }
}

/// Wait until `busy` says no, asking it every `every`: a local server's
/// models are not probed while a game plays.
pub async fn idle(busy: impl Fn() -> bool, every: Duration) {
    while busy() {
        tokio::time::sleep(every).await;
    }
}

/// What probing has found of each model, by its provider's base URL and
/// its id, as one version of the app found it: those of another version
/// are probed again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probes {
    version: String,
    probes: BTreeMap<String, BTreeMap<String, Check>>,
}

impl Default for Probes {
    /// None, found by this version.
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            probes: BTreeMap::new(),
        }
    }
}

impl Probes {
    /// The probes kept at `path`, if this version kept them there, or
    /// none.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Probes>(&text).ok())
            .filter(|probes| probes.version == Probes::default().version)
            .unwrap_or_default()
    }

    /// Keep the probes at `path`, making its directory first.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// What probing found of each model of the provider at `base_url`, by
    /// id.
    pub fn of(&self, base_url: &str) -> BTreeMap<String, Check> {
        self.probes.get(base_url).cloned().unwrap_or_default()
    }

    /// Keep what probing the model `id` of the provider at `base_url`
    /// found.
    pub fn record(&mut self, base_url: &str, id: &str, check: Check) {
        let probes = self.probes.entry(base_url.to_string()).or_default();
        probes.insert(id.to_string(), check);
    }

    /// Forget every probe of the provider at `base_url`.
    pub fn clear(&mut self, base_url: &str) {
        self.probes.remove(base_url);
    }

    /// The ids of the models in `listing`, of the provider at `base_url`,
    /// still to probe: those not hidden and not probed already.
    pub fn unprobed(&self, base_url: &str, listing: &Listing) -> Vec<String> {
        let probed = self.probes.get(base_url);
        listing
            .models
            .iter()
            .filter(|model| !model.hidden)
            .filter(|model| probed.is_none_or(|probed| !probed.contains_key(&model.id)))
            .map(|model| model.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::canned;
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A file of its own for the test `name`, not yet made.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("catalog-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("probes.json")
    }

    /// The port of the local root `base_url`, `http://127.0.0.1:<port>`.
    fn port(base_url: &str) -> u16 {
        let rest = base_url.rsplit(':').next().unwrap();
        rest.trim_end_matches("/v1").parse().unwrap()
    }

    #[test]
    fn the_providers_are_the_known_ones_then_each_local_server() {
        let providers = providers(&[1234]);
        let names: Vec<&str> = providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["OpenAI", "Anthropic", "Local (localhost:1234)"]);
        assert_eq!(providers[0].api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(providers[2].base_url, "http://localhost:1234/v1");
        assert_eq!(providers[2].api_key_env, None);
    }

    #[test]
    fn a_remote_provider_is_probed_four_at_a_time_and_a_local_one_one_at_a_time() {
        assert_eq!(at_once("https://api.anthropic.com/v1"), 4);
        assert_eq!(at_once("http://localhost:11434/v1"), 1);
    }

    #[tokio::test]
    async fn both_local_servers_are_found_and_a_closed_port_is_skipped_within_the_knock() {
        let first = port(&canned::serving(&["a"]));
        let second = port(&canned::serving(&["b"]));
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let started = std::time::Instant::now();
        let found = discover(&[first, closed, second], KNOCK).await;
        assert_eq!(found, [first, second]);
        assert!(started.elapsed() < Duration::from_secs(2));
        // One that never answers is given up on.
        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = silent.local_addr().unwrap().port();
        let started = std::time::Instant::now();
        assert!(discover(&[port], KNOCK).await.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_listing_marks_the_ids_hidden_and_ranks_them_with_each_context_window() {
        let base_url = canned::playing_within(&["text-embedding-3-small", "gpt-4.1"], 32768);
        let listing = list(&base_url, None).await;
        assert_eq!(
            listing.models,
            [
                Listed {
                    id: "gpt-4.1".to_string(),
                    context_window: 32768,
                    hidden: false,
                },
                Listed {
                    id: "text-embedding-3-small".to_string(),
                    context_window: 32768,
                    hidden: true,
                },
            ]
        );
        assert_eq!(listing.error, None);
        // A listing that reports no window gives the code's default.
        let listing = list(&canned::playing(&["llama"]), None).await;
        assert_eq!(listing.models[0].context_window, model::CONTEXT_WINDOW);
    }

    #[test]
    fn a_window_the_listing_does_not_report_is_the_provider_s_default() {
        assert_eq!(window("https://api.anthropic.com/v1", None), 200_000);
        assert_eq!(window("https://api.anthropic.com/v1", Some(8192)), 8192);
        assert_eq!(
            window("http://localhost:1234/v1", None),
            model::CONTEXT_WINDOW
        );
    }

    /// A model the provider at lists as `id`, made at `created`.
    fn made(id: &str, created: Option<u64>) -> model::Listed {
        model::Listed {
            id: id.to_string(),
            context_window: None,
            created,
        }
    }

    #[test]
    fn models_rank_by_family_then_by_size_then_newest_first() {
        let mut models = vec![
            made("mystery-old", Some(1)),
            made("claude-haiku-5-5", Some(50)),
            made("gemma-3-12b-it", None),
            made("claude-opus-4-1-20250805", Some(10)),
            made("mystery-new", Some(9)),
            made("claude-opus-5-5", Some(40)),
            made("llama-3.2-3b-instruct", None),
            made("gpt-4.1-mini", None),
            made("gpt-4.1-2025-04-14", None),
            made("qwen2.5-7b-instruct-1m", None),
            made("undated", None),
            made("claude-fable-5-1", Some(30)),
            made("gpt-4.1", None),
        ];
        rank(&mut models);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-fable-5-1",
                "claude-opus-5-5",
                "claude-opus-4-1-20250805",
                "gpt-4.1-2025-04-14",
                "gpt-4.1",
                "claude-haiku-5-5",
                "gpt-4.1-mini",
                "gemma-3-12b-it",
                "qwen2.5-7b-instruct-1m",
                "llama-3.2-3b-instruct",
                "mystery-new",
                "mystery-old",
                "undated",
            ]
        );
    }

    #[test]
    fn a_family_is_the_whole_id_or_its_start_before_a_version_or_date() {
        let family = |id| family(id).map(|place| model::families()[place].as_str());
        assert_eq!(family("claude-opus-5-5"), Some("claude-opus"));
        assert_eq!(family("Claude-Opus-5-5"), Some("claude-opus"));
        assert_eq!(family("gpt-4.1"), Some("gpt-4.1"));
        assert_eq!(family("gpt-4.1-mini-2025-04-14"), Some("gpt-4.1-mini"));
        assert_eq!(family("o3-mini"), None);
        assert_eq!(family("claude-opusx"), None);
    }

    #[test]
    fn a_parameter_count_is_read_from_a_part_of_the_id_of_its_own() {
        assert_eq!(parameters("qwen2.5-7b-instruct-1m"), Some(70));
        assert_eq!(parameters("llama3.2:3b"), Some(30));
        assert_eq!(parameters("Meta-Llama-3.1-8B-Instruct"), Some(80));
        assert_eq!(parameters("qwen2.5-0.5b"), Some(5));
        assert_eq!(parameters("gemma-2b-or-27b"), Some(270));
        for none in ["phi-3.5-mini", "mixtral-8x7b", "b", "1.2.3b", "gpt-4.1"] {
            assert_eq!(parameters(none), None, "{none}");
        }
    }

    #[tokio::test]
    async fn a_listing_that_fails_says_why_in_a_line() {
        let (base_url, _) = canned::answering("401 Unauthorized", "{}");
        let listing = list(&base_url, None).await;
        assert!(listing.models.is_empty());
        assert_eq!(
            listing.error.as_deref(),
            Some("Couldn't list models: 401, check the API key")
        );
        let listing = list(&canned::unreachable(), None).await;
        assert_eq!(listing.error.as_deref(), Some("No local server answered"));
        let (base_url, _) = canned::answering("500 Internal Server Error", "{}");
        let error = list(&base_url, None).await.error.unwrap();
        assert!(error.starts_with("Couldn't list models: GET"), "{error}");
        assert!(error.contains("500"), "{error}");
        let error = list("https://", None).await.error.unwrap();
        assert!(error.starts_with("Couldn't list models: "), "{error}");
    }

    #[tokio::test]
    async fn a_probe_says_whether_the_model_plays_or_could_not_be_run() {
        let base_url = canned::playing(&["llama"]);
        assert_eq!(probe(&base_url, "llama", None).await.unwrap(), Check::Plays);
        let base_url = canned::refusing_required(&["llama"]);
        assert_eq!(
            probe(&base_url, "llama", None).await.unwrap(),
            Check::PlaysUnforced
        );
        assert!(probe(&canned::unreachable(), "llama", None).await.is_err());
    }

    /// Probe `ids` `at_once` at a time with probes that each take a while,
    /// and give back the most that ran at once and what was recorded.
    async fn probed(ids: &[&str], at_once: usize) -> (usize, Vec<(String, Option<Check>)>) {
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let recorded = Mutex::new(Vec::new());
        let probe = |id: String| {
            let (running, most) = (&running, &most);
            async move {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(1)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                match id.as_str() {
                    "down" => Err(anyhow::anyhow!("no key")),
                    "mute" => Ok(Check::DidNotPlay("no tools".to_string())),
                    _ => Ok(Check::Plays),
                }
            }
        };
        let ids = ids.iter().map(ToString::to_string).collect();
        probe_each(ids, at_once, probe, |id, probed| {
            recorded.lock().unwrap().push((id.to_string(), probed.ok()));
        })
        .await;
        let mut recorded = recorded.into_inner().unwrap();
        recorded.sort_by(|a, b| a.0.cmp(&b.0));
        (most.into_inner(), recorded)
    }

    #[tokio::test(start_paused = true)]
    async fn probing_runs_at_most_so_many_at_a_time_and_a_probe_that_cannot_run_is_unchecked() {
        let ids = ["a", "b", "c", "d", "e", "f", "down", "mute"];
        let (most, recorded) = probed(&ids, AT_ONCE).await;
        assert_eq!(most, 4);
        assert_eq!(recorded.len(), 8);
        assert!(recorded.contains(&("down".to_string(), None)));
        assert!(recorded.contains(&(
            "mute".to_string(),
            Some(Check::DidNotPlay("no tools".to_string()))
        )));
        assert!(recorded.contains(&("a".to_string(), Some(Check::Plays))));
        let (most, recorded) = probed(&ids, 1).await;
        assert_eq!(most, 1);
        assert_eq!(recorded.len(), 8);
    }

    #[tokio::test]
    async fn a_model_that_could_not_be_asked_is_unchecked_for_what_the_provider_said() {
        let answered = async |status: &str, body: &str| {
            let (base_url, _served) = canned::answering(status, body);
            unchecked(&probe(&base_url, "gpt", None).await.unwrap_err())
        };
        let trouble = |trouble, said: &str| Unchecked {
            trouble,
            said: said.to_string(),
        };
        let said = r#"{"error": {"message": "Not you", "code": "nope"}}"#;
        assert_eq!(
            answered("429 Too Many Requests", canned::NO_CREDITS).await,
            trouble(
                Trouble::NoCredits,
                "You have no credits remaining. Add credits to continue using the API at \
                 https://platform.openai.com/settings/organization/billing/."
            )
        );
        assert_eq!(
            answered("401 Unauthorized", said).await,
            trouble(Trouble::KeyRefused, "Not you")
        );
        assert_eq!(
            answered("403 Forbidden", said).await,
            trouble(Trouble::KeyForbidden, "Not you")
        );
        assert_eq!(
            answered("500 Internal Server Error", said).await,
            trouble(Trouble::Failed, "Not you")
        );
        // A provider that said nothing, or was not reached, is unchecked
        // for the error.
        let error = answered("503 Service Unavailable", "").await;
        assert_eq!(error.trouble, Trouble::Failed);
        assert!(error.said.contains("503"), "{error:?}");
        let base_url = canned::unreachable();
        let error = unchecked(&probe(&base_url, "gpt", None).await.unwrap_err());
        assert_eq!(error.trouble, Trouble::Failed);
        assert!(error.said.contains(&base_url), "{error:?}");
    }

    #[tokio::test]
    async fn a_model_the_account_could_not_ask_stops_the_provider_s_probing_and_any_other_does_not()
    {
        let (broke, _served) = canned::answering("429 Too Many Requests", canned::NO_CREDITS);
        let down = canned::unreachable();
        let playing = canned::playing(&["a", "b"]);
        let asked = |id: String| {
            let base_url = match id.as_str() {
                "broke" => broke.clone(),
                "down" => down.clone(),
                _ => playing.clone(),
            };
            async move { probe(&base_url, &id, None).await }
        };
        let mut recorded = Vec::new();
        let ids = ["down", "a", "broke", "b"].map(String::from).to_vec();
        probe_each(ids, 1, asked, |id, probed| {
            recorded.push((id.to_string(), probed));
        })
        .await;
        let ids: Vec<&str> = recorded.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["down", "a", "broke"]);
        let trouble = |probed: &Result<Check, Unchecked>| probed.as_ref().unwrap_err().trouble;
        assert_eq!(trouble(&recorded[0].1), Trouble::Failed);
        assert_eq!(recorded[1].1, Ok(Check::Plays));
        assert_eq!(trouble(&recorded[2].1), Trouble::NoCredits);
    }

    #[tokio::test(start_paused = true)]
    async fn waiting_for_idle_asks_until_nothing_is_busy() {
        let asked = AtomicUsize::new(0);
        let started = tokio::time::Instant::now();
        idle(
            || asked.fetch_add(1, Ordering::SeqCst) < 2,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(asked.into_inner(), 3);
        assert_eq!(started.elapsed(), Duration::from_secs(10));
    }

    /// A listing of `ids`, of which those starting `text-embedding` are
    /// hidden.
    fn listing(ids: &[&str]) -> Listing {
        Listing {
            models: ids
                .iter()
                .map(|id| Listed {
                    id: id.to_string(),
                    context_window: model::CONTEXT_WINDOW,
                    hidden: id.starts_with("text-embedding"),
                })
                .collect(),
            ..Listing::default()
        }
    }

    #[test]
    fn probes_persist_and_are_not_run_again_until_cleared() {
        let path = scratch("persist");
        let openai = "https://api.openai.com/v1";
        let listed = listing(&["gpt-4.1", "gpt-4.1-mini", "text-embedding-3-small"]);
        let mut probes = Probes::load(&path);
        assert_eq!(probes, Probes::default());
        assert_eq!(
            probes.unprobed(openai, &listed),
            ["gpt-4.1", "gpt-4.1-mini"]
        );
        probes.record(openai, "gpt-4.1", Check::Plays);
        probes.save(&path).unwrap();
        let mut probes = Probes::load(&path);
        assert_eq!(probes.unprobed(openai, &listed), ["gpt-4.1-mini"]);
        assert_eq!(
            probes.of(openai),
            BTreeMap::from([("gpt-4.1".to_string(), Check::Plays)])
        );
        assert!(probes.of("http://localhost:1234/v1").is_empty());
        probes.clear(openai);
        assert_eq!(
            probes.unprobed(openai, &listed),
            ["gpt-4.1", "gpt-4.1-mini"]
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn probes_another_version_kept_are_run_again() {
        let path = scratch("version");
        let mut probes = Probes {
            version: "0.0.0".to_string(),
            ..Probes::default()
        };
        probes.record("https://api.openai.com/v1", "gpt-4.1", Check::Plays);
        probes.save(&path).unwrap();
        assert_eq!(Probes::load(&path), Probes::default());
        assert!(Probes::default().save(Path::new("")).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
