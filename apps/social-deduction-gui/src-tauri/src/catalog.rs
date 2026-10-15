//! The commands behind the Models tab: the providers, local servers
//! among them, and each provider's listing, as
//! [`social_deduction::catalog`] gives each, and checking its models.
//!
//! Models are checked in the background, never at the user's asking: each
//! listing starts probing the models it holds that have not been probed,
//! as many at once as [`catalog::at_once`] says, and a local server's only
//! while no game is playing. The app lists every provider when it starts,
//! so a first launch, or the first after an update, checks every model.
//! Each probe sends a `probe` event as it ends and a `progress` event with
//! it. Probes are kept in `probes.json` in the app's data directory.
//!
//! A model that could not be asked is left to probe again, and the
//! progress carries why, for its provider's card. When the account is the
//! reason, as with a key not accepted or no credits left, the provider's
//! other models wait too, until its key changes or Settings opens again.

use crate::secret::keychain;
use crate::settings::Store;
use secrecy::SecretString;
use social_deduction::catalog::{self, KNOCK, Probes};
use social_deduction::live::Session;
use social_deduction::model::spec::Keys;
use social_deduction::model::{key_variable, local_ports};
use social_deduction::settings::{Check, Listing, Probe, Progress, Provider, Unchecked, is_remote};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, async_runtime};

/// How often probing a local server's models asks whether a game still
/// plays.
const WHILE_PLAYING: Duration = Duration::from_secs(2);

/// The probes kept, the providers being probed, and how far probing each
/// has got.
pub struct Catalog {
    path: PathBuf,
    probes: Mutex<Probes>,
    probing: Mutex<HashSet<String>>,
    progress: Mutex<HashMap<String, Progress>>,
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
            probing: Mutex::default(),
            progress: Mutex::default(),
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

    /// Keep `progress`, and send it to the Settings window.
    fn progressed(&self, app: &AppHandle, progress: &Progress) {
        let mut all = self.progress.lock().unwrap();
        all.insert(progress.base_url.clone(), progress.clone());
        announce(app, "progress", progress);
    }
}

/// The variable holding the key for the provider at `base_url`, the one
/// `api_key_env` names or the one it is known by, if it wants one, and
/// the key, if it is set.
fn key(base_url: &str, api_key_env: Option<&str>) -> Option<(String, Option<SecretString>)> {
    let variable = key_variable(base_url, api_key_env)?;
    Some((
        variable.to_string(),
        keychain().key(variable).ok().flatten(),
    ))
}

/// The providers: those known, then a local server on each port that
/// answers.
#[tauri::command]
pub async fn providers() -> Vec<Provider> {
    catalog::providers(&catalog::discover(local_ports(), KNOCK).await)
}

/// How far probing each provider has got, for a window that opens while
/// it runs.
#[tauri::command]
pub fn checking(catalog: State<'_, Catalog>) -> Vec<Progress> {
    catalog.progress.lock().unwrap().values().cloned().collect()
}

/// The models the provider at `base_url` lists now, with what probing
/// found of each, its probes forgotten first when `forget` says, as when
/// its key changes. Its models not yet probed are probed in the
/// background. A provider whose key is not set is not asked.
#[tauri::command]
pub async fn listing(
    app: AppHandle,
    base_url: String,
    api_key_env: Option<String>,
    forget: bool,
) -> Result<Listing, String> {
    Ok(list(&app, base_url, api_key_env, forget).await)
}

/// What [`listing`] gives.
async fn list(
    app: &AppHandle,
    base_url: String,
    api_key_env: Option<String>,
    forget: bool,
) -> Listing {
    let catalog = app.state::<Catalog>();
    if forget {
        catalog.change(|probes| probes.clear(&base_url));
    }
    let key = match key(&base_url, api_key_env.as_deref()) {
        Some((variable, None)) => {
            return Listing {
                error: Some(format!("Its models are listed once {variable} is set.")),
                ..Listing::default()
            };
        }
        Some((_, key)) => key,
        None => None,
    };
    let mut listing = catalog::list(&base_url, key.clone()).await;
    if listing.error.is_none() {
        start(app, &listing, base_url.clone(), key);
    }
    listing.checks = catalog.probes.lock().unwrap().of(&base_url);
    listing
}

/// List every provider the Settings window shows, in the background, so
/// that each one's models not yet probed are probed: every model on a
/// first launch and after an update.
pub fn check_in_background(app: &AppHandle) {
    let app = app.clone();
    async_runtime::spawn(async move {
        let found = providers().await;
        let shown = app.state::<Store>().settings().providers_shown(&found);
        for provider in shown {
            let app = app.clone();
            async_runtime::spawn(async move {
                list(&app, provider.base_url, provider.api_key_env, false).await;
            });
        }
    });
}

/// Probe each model of the provider at `base_url`, asked with `key`, that
/// its `listing` holds and that has not been probed, as many at once as
/// [`catalog::at_once`] says, on a thread of its own, unless it is being
/// probed already. A local server's models wait while a game plays. The
/// probes are kept as they end, and stored once all have.
fn start(app: &AppHandle, listing: &Listing, base_url: String, key: Option<SecretString>) {
    let catalog = app.state::<Catalog>();
    let ids = catalog.probes.lock().unwrap().unprobed(&base_url, listing);
    if ids.is_empty() || !catalog.probing.lock().unwrap().insert(base_url.clone()) {
        return;
    }
    let app = app.clone();
    async_runtime::spawn_blocking(move || {
        let catalog = app.state::<Catalog>();
        let mut progress = Progress {
            base_url: base_url.clone(),
            checked: 0,
            total: ids.len(),
            unchecked: None,
        };
        catalog.progressed(&app, &progress);
        let local = !is_remote(&base_url);
        let probe = |id: String| {
            let (url, key, app) = (base_url.clone(), key.clone(), app.clone());
            async move {
                if local {
                    let busy = || app.state::<Session>().busy();
                    catalog::idle(busy, WHILE_PLAYING).await;
                }
                catalog::probe(&url, &id, key).await
            }
        };
        let record = |id: &str, probed: Result<Check, Unchecked>| {
            let check = match probed {
                Ok(check) => {
                    let mut probes = catalog.probes.lock().unwrap();
                    probes.record(&base_url, id, check.clone());
                    Some(check)
                }
                Err(unchecked) => {
                    progress.unchecked = Some(unchecked);
                    None
                }
            };
            let probe = Probe {
                base_url: base_url.clone(),
                id: id.to_string(),
                check,
            };
            announce(&app, "probe", &probe);
            progress.checked += 1;
            catalog.progressed(&app, &progress);
        };
        let at_once = catalog::at_once(&base_url);
        async_runtime::block_on(catalog::probe_each(ids, at_once, probe, record));
        // Those not probed once the account failed are left for next time.
        if progress.checked < progress.total {
            progress.total = progress.checked;
            catalog.progressed(&app, &progress);
        }
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
