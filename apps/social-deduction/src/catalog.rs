//! What the desktop app's Models tab offers: the providers a model is
//! reached through, those `src/models.toml` knows and the local servers
//! [`discover`] finds; each provider's [`list`]ing; and what probing each
//! listed model found, kept in [`Probes`] so that no model is probed twice.
//!
//! A remote provider, one reached over `https`, has its models probed
//! [`AT_ONCE`] at a time. A local server loads each model it is asked
//! about, so its models are probed one at a time. [`probe_each`] runs a
//! provider's probes either way.

use crate::model::check;
use crate::model::{self, Provider as Client, REQUEST_TIMEOUT};
use crate::settings::{Check, Listed, Listing, Provider};
use futures_util::StreamExt;
use reqwest::StatusCode;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
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
    known.chain(ports.iter().map(|port| local(*port))).collect()
}

/// The local server on `port`: `Local (localhost:1234)`, wanting no key.
pub fn local(port: u16) -> Provider {
    Provider {
        name: format!("Local (localhost:{port})"),
        base_url: format!("http://localhost:{port}/v1"),
        api_key_env: None,
    }
}

/// Whether the provider at `base_url` is remote: reached over `https`.
pub fn is_remote(base_url: &str) -> bool {
    base_url.trim_start().starts_with("https://")
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
        let client = Client::new(local(*port).base_url, None, timeout).ok()?;
        client.listing().await.ok().map(|_| *port)
    });
    futures_util::future::join_all(knocks)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// The models the provider at `base_url` lists, asked with `key`, each
/// marked hidden when its id says it does not chat; or, when it cannot
/// list them, why, in a line for under the provider.
pub async fn list(base_url: &str, key: Option<SecretString>) -> Listing {
    let listed = async { Client::new(base_url, key, REQUEST_TIMEOUT)?.listing().await };
    match listed.await {
        Ok(models) => Listing {
            models: models
                .into_iter()
                .map(|model| Listed {
                    hidden: model::hidden(&model.id),
                    id: model.id,
                    context_window: model.context_window,
                })
                .collect(),
            ..Listing::default()
        },
        Err(error) => Listing {
            error: Some(unlisted(base_url, &error)),
            ..Listing::default()
        },
    }
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

/// Probe each of `ids` with `probe`, `at_once` at a time, passing each id
/// to `record` with what its probe found, or `None` when the probe could
/// not be run, as each probe ends.
pub async fn probe_each<F>(
    ids: Vec<String>,
    at_once: usize,
    probe: impl Fn(String) -> F,
    mut record: impl FnMut(&str, Option<Check>),
) where
    F: Future<Output = anyhow::Result<Check>>,
{
    let probes = ids.into_iter().map(|id| {
        let probed = probe(id.clone());
        async move { (id, probed.await.ok()) }
    });
    let mut ended = futures_util::stream::iter(probes).buffer_unordered(at_once.max(1));
    while let Some((id, check)) = ended.next().await {
        record(&id, check);
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
        assert!(is_remote("https://api.openai.com/v1"));
        assert!(!is_remote("http://localhost:1234/v1"));
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
    async fn a_listing_marks_the_ids_hidden_and_reports_each_context_window() {
        let base_url = canned::playing_within(&["gpt-4.1", "text-embedding-3-small"], 32768);
        let listing = list(&base_url, None).await;
        assert_eq!(
            listing.models,
            [
                Listed {
                    id: "gpt-4.1".to_string(),
                    context_window: Some(32768),
                    hidden: false,
                },
                Listed {
                    id: "text-embedding-3-small".to_string(),
                    context_window: Some(32768),
                    hidden: true,
                },
            ]
        );
        assert_eq!(listing.error, None);
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
        probe_each(ids, at_once, probe, |id, check| {
            recorded.lock().unwrap().push((id.to_string(), check));
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

    /// A listing of `ids`, of which those starting `text-embedding` are
    /// hidden.
    fn listing(ids: &[&str]) -> Listing {
        Listing {
            models: ids
                .iter()
                .map(|id| Listed {
                    id: id.to_string(),
                    context_window: None,
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
