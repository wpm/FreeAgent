//! The commands behind the Models tab: the providers, local servers
//! among them, each provider's listing, and probing its models, as
//! [`social_deduction::catalog`] does each.
//!
//! Nothing here keeps the window waiting on the network: each command is
//! async, and probing runs in the background, sending a `probe` event as
//! each model's probe ends and a `progress` event with it. Listings are
//! kept for the app's session, and probes in `probes.json` in the app's
//! data directory.

use crate::secret::keychain;
use secrecy::SecretString;
use social_deduction::catalog::{self, KNOCK, Probes};
use social_deduction::model::spec::Keys;
use social_deduction::model::{key_variable, local_ports};
use social_deduction::settings::{Check, Listing, Probe, Progress, Provider, is_remote};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State, async_runtime};

/// The listings fetched this session and the probes kept.
pub struct Catalog {
    path: PathBuf,
    probes: Mutex<Probes>,
    listings: Mutex<HashMap<String, Listing>>,
    probing: Mutex<HashSet<String>>,
}

impl Catalog {
    /// The catalog of `app`, its probes read from its data directory.
    ///
    /// # Errors
    ///
    /// When the app has no data directory.
    pub fn open(app: &AppHandle) -> tauri::Result<Self> {
        let path = app.path().app_data_dir()?.join("probes.json");
        Ok(Self {
            probes: Mutex::new(Probes::load(&path)),
            path,
            listings: Mutex::default(),
            probing: Mutex::default(),
        })
    }

    /// Change the probes with `change`, and keep them.
    fn change(&self, change: impl FnOnce(&mut Probes)) {
        let mut probes = self.probes.lock().unwrap();
        change(&mut probes);
        if let Err(error) = probes.save(&self.path) {
            tracing::warn!("the probes could not be saved: {error}");
        }
    }
}

/// The key for the provider at `base_url`, read from the variable
/// `api_key_env` names or the one it is known by: whether one is needed,
/// and the key, if it is set.
fn key(base_url: &str, api_key_env: Option<&str>) -> (bool, Option<SecretString>) {
    match key_variable(base_url, api_key_env) {
        Some(variable) => (true, keychain().key(variable).ok().flatten()),
        None => (false, None),
    }
}

/// The providers: those known, then a local server on each port that
/// answers.
#[tauri::command]
pub async fn providers() -> Vec<Provider> {
    catalog::providers(&catalog::discover(local_ports(), KNOCK).await)
}

/// The models the provider at `base_url` lists, with what probing found
/// of each, fetched once a session, or again when `refresh` says, which
/// forgets its probes too. A remote provider whose key is set has its
/// models probed in the background.
#[tauri::command]
pub async fn listing(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    base_url: String,
    api_key_env: Option<String>,
    refresh: bool,
) -> Result<Listing, String> {
    if refresh {
        catalog.listings.lock().unwrap().remove(&base_url);
        catalog.change(|probes| probes.clear(&base_url));
    }
    let kept = catalog.listings.lock().unwrap().get(&base_url).cloned();
    let (needed, key) = key(&base_url, api_key_env.as_deref());
    let mut listing = match kept {
        Some(listing) => listing,
        None => {
            let listing = catalog::list(&base_url, key.clone()).await;
            if listing.error.is_none() {
                let mut listings = catalog.listings.lock().unwrap();
                listings.insert(base_url.clone(), listing.clone());
            }
            listing
        }
    };
    listing.checks = catalog.probes.lock().unwrap().of(&base_url);
    if listing.error.is_none() && is_remote(&base_url) && (!needed || key.is_some()) {
        start(&app, base_url, key);
    }
    Ok(listing)
}

/// Probe every model of the provider at `base_url` not probed yet, in the
/// background, as **Check all** does.
#[tauri::command]
pub fn check_all(app: AppHandle, base_url: String, api_key_env: Option<String>) {
    let (_, key) = key(&base_url, api_key_env.as_deref());
    start(&app, base_url, key);
}

/// Probe the model `id` of the provider at `base_url`, as its **Check**
/// does, and give back what it found, or `None` when the probe could not
/// be run.
#[tauri::command]
pub async fn check_model(
    app: AppHandle,
    catalog: State<'_, Catalog>,
    base_url: String,
    api_key_env: Option<String>,
    id: String,
) -> Result<Option<Check>, String> {
    let (_, key) = key(&base_url, api_key_env.as_deref());
    let (url, model) = (base_url.clone(), id.clone());
    // Probing goes through futures the compiler cannot prove Send, as
    // preparing a game does.
    let check = async_runtime::spawn_blocking(move || {
        async_runtime::block_on(async move { catalog::probe(&url, &model, key).await.ok() })
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Some(check) = &check {
        catalog.change(|probes| probes.record(&base_url, &id, check.clone()));
    }
    let probe = Probe {
        base_url,
        id,
        check: check.clone(),
    };
    announce(&app, "probe", &probe);
    Ok(check)
}

/// Probe each model of the provider at `base_url`, asked with `key`, that
/// its listing holds and that has not been probed, as many at once as
/// [`catalog::at_once`] says, on a thread of its own, unless it is being
/// probed already. The probes are kept as they end, and stored once all
/// have.
fn start(app: &AppHandle, base_url: String, key: Option<SecretString>) {
    let catalog = app.state::<Catalog>();
    let Some(listing) = catalog.listings.lock().unwrap().get(&base_url).cloned() else {
        return;
    };
    if !catalog.probing.lock().unwrap().insert(base_url.clone()) {
        return;
    }
    let ids = catalog.probes.lock().unwrap().unprobed(&base_url, &listing);
    let app = app.clone();
    async_runtime::spawn_blocking(move || {
        let catalog = app.state::<Catalog>();
        let mut progress = Progress {
            base_url: base_url.clone(),
            checked: 0,
            total: ids.len(),
        };
        announce(&app, "progress", &progress);
        let probe = |id: String| {
            let (url, key) = (base_url.clone(), key.clone());
            async move { catalog::probe(&url, &id, key).await }
        };
        let record = |id: &str, check: Option<Check>| {
            if let Some(check) = &check {
                let mut probes = catalog.probes.lock().unwrap();
                probes.record(&base_url, id, check.clone());
            }
            let probe = Probe {
                base_url: base_url.clone(),
                id: id.to_string(),
                check,
            };
            announce(&app, "probe", &probe);
            progress.checked += 1;
            announce(&app, "progress", &progress);
        };
        let at_once = catalog::at_once(&base_url);
        async_runtime::block_on(catalog::probe_each(ids, at_once, probe, record));
        catalog.change(|_| {});
        catalog.probing.lock().unwrap().remove(&base_url);
    });
}

/// Send `payload` to the Settings window as `event`.
fn announce(app: &AppHandle, event: &str, payload: impl serde::Serialize + Clone) {
    if let Err(error) = app.emit_to(crate::settings::WINDOW, event, payload) {
        tracing::warn!("probing could not be announced: {error}");
    }
}
