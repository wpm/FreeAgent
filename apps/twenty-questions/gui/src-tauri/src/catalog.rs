//! The commands behind the Models tab, as [`free_agent_desktop::catalog`]
//! answers them, and the [`Prober`] it probes Twenty Questions' way with:
//! asking a model to play as a player and as the host, holding a local
//! server's models back while a game plays.

use crate::secret::SERVICE;
use crate::settings::Store;
use free_agent_desktop::catalog::{self, Prober};
use free_agent_model::Model;
use free_agent_model::spec::Probed;
use futures_util::future::LocalBoxFuture;
use tauri::{AppHandle, Manager};
use twenty_questions::live::Session;
use twenty_questions::model::plays;
use twenty_questions::settings::{Listing, Progress, Provider, Settings};

/// How Twenty Questions' models are probed.
pub fn prober() -> Prober {
    Prober {
        version: env!("CARGO_PKG_VERSION"),
        service: SERVICE,
        plays: probe,
        busy: |app| app.state::<Session>().busy(),
        shown: |app, found| app.state::<Store>().settings().providers_shown(found),
    }
}

/// What asking `model` to play as the model `id` finds, as [`plays`] says.
fn probe<'a>(model: &'a dyn Model, id: &'a str) -> LocalBoxFuture<'a, anyhow::Result<Probed>> {
    Box::pin(plays(model, id))
}

/// The providers: those known, then a local server on each port that
/// answers.
#[tauri::command]
pub async fn providers() -> Vec<Provider> {
    catalog::providers().await
}

/// How far probing each provider has got, for a window that opens while
/// it runs.
#[tauri::command]
pub fn checking(app: AppHandle) -> Vec<Progress> {
    catalog::checking(&app)
}

/// The models the provider at `base_url` lists now, with what probing
/// found of each, its probes forgotten first when `forget` says.
#[tauri::command]
pub async fn listing(
    app: AppHandle,
    base_url: String,
    api_key_env: Option<String>,
    forget: bool,
) -> Result<Listing, String> {
    Ok(catalog::listing(&app, base_url, api_key_env, forget).await)
}
