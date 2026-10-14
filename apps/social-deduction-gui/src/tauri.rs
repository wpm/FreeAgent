//! The desktop app's backend, reached through `window.__TAURI__`, which
//! Tauri sets on the page (`withGlobalTauri`). Commands go through
//! `core.invoke`, and a game's cues come back down a `core.Channel`, as
//! JSON.

use leptos::task::spawn_local;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use social_deduction_stage::Cue;
use social_deduction_stage::form::{Check, FormView, KeyStatus, Value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    /// Tauri's channel: what the backend sends down it reaches its
    /// `onmessage`.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"])]
    type Channel;

    #[wasm_bindgen(constructor, js_namespace = ["window", "__TAURI__", "core"])]
    fn new() -> Channel;

    #[wasm_bindgen(method, setter)]
    fn set_onmessage(this: &Channel, handler: &js_sys::Function);
}

/// Whether the page is inside the desktop app.
pub fn in_tauri() -> bool {
    web_sys::window()
        .map(|window| {
            js_sys::Reflect::has(&window, &JsValue::from_str("__TAURI__")).unwrap_or(false)
        })
        .unwrap_or(false)
}

/// An example configuration to play.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Preset {
    /// Where it is among the examples: `werewolf/random`.
    pub name: String,
    /// The configuration file's text.
    pub text: String,
}

/// Where a game's cues arrive: a channel, and what is done with each cue
/// as it comes. It is kept for as long as the game plays.
pub struct Cues {
    channel: Channel,
    _handler: Closure<dyn FnMut(JsValue)>,
}

impl Cues {
    /// A channel whose cues go to `show`, one at a time, in order.
    pub fn new(mut show: impl FnMut(Cue) + 'static) -> Self {
        let handler = Closure::new(
            move |cue: JsValue| match serde_wasm_bindgen::from_value(cue) {
                Ok(cue) => show(cue),
                Err(error) => web_sys::console::error_1(&format!("a cue: {error}").into()),
            },
        );
        let channel = Channel::new();
        channel.set_onmessage(handler.as_ref().unchecked_ref());
        Self {
            channel,
            _handler: handler,
        }
    }
}

/// `args` as an object for a command.
fn object(args: &[(&str, &JsValue)]) -> JsValue {
    let object = js_sys::Object::new();
    for (name, value) in args {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str(name), value);
    }
    object.into()
}

/// The error a command failed with, as the backend words it.
fn error(error: JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

/// The example configurations.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn presets() -> Result<Vec<Preset>, String> {
    let presets = invoke("presets", object(&[])).await.map_err(error)?;
    serde_wasm_bindgen::from_value(presets).map_err(|error| error.to_string())
}

/// The command `cmd`'s answer to `args`, a JSON object.
///
/// # Errors
///
/// The command's error, or why its answer could not be read.
async fn call<T: DeserializeOwned>(cmd: &str, args: serde_json::Value) -> Result<T, String> {
    let args = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|error| error.to_string())?;
    let answer = invoke(cmd, args).await.map_err(error)?;
    serde_wasm_bindgen::from_value(answer).map_err(|error| error.to_string())
}

/// The configuration being edited, as the form shows it.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn document_view() -> Result<FormView, String> {
    call("document_view", json!({})).await
}

/// Set the key at `path` to `value`, and the form after.
///
/// # Errors
///
/// When the edit names no key, or passes through a value.
pub async fn document_set(path: Vec<String>, value: Value) -> Result<FormView, String> {
    call("document_set", json!({ "path": path, "value": value })).await
}

/// The configuration's text.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn document_text() -> Result<String, String> {
    call("document_text", json!({})).await
}

/// Replace the configuration with `text`, and the form after.
///
/// # Errors
///
/// When `text` is not TOML, in the parser's words.
pub async fn document_replace(text: String) -> Result<FormView, String> {
    call("document_replace", json!({ "text": text })).await
}

/// Open the preset `name`, and the form after.
///
/// # Errors
///
/// When there is no such preset.
pub async fn load_preset(name: String) -> Result<FormView, String> {
    call("load_preset", json!({ "name": name })).await
}

/// The filters the file dialogs show: configuration files.
fn toml_filters() -> serde_json::Value {
    json!([{ "name": "Configuration", "extensions": ["toml"] }])
}

/// Open a configuration file the user picks, and the form after, or
/// `None` when they pick none.
///
/// # Errors
///
/// When the file cannot be read or is not TOML.
pub async fn open_file() -> Result<Option<FormView>, String> {
    let options = json!({ "options": { "filters": toml_filters() } });
    let Some(path) = call::<Option<String>>("plugin:dialog|open", options).await? else {
        return Ok(None);
    };
    call("open", json!({ "path": path })).await.map(Some)
}

/// Save the configuration to a file the user picks. Whether they picked
/// one.
///
/// # Errors
///
/// When the file cannot be written.
pub async fn save_file() -> Result<bool, String> {
    let options = json!({ "options": { "filters": toml_filters() } });
    let Some(path) = call::<Option<String>>("plugin:dialog|save", options).await? else {
        return Ok(false);
    };
    call::<()>("save", json!({ "path": path })).await?;
    Ok(true)
}

/// Ask the model `name` to play.
///
/// # Errors
///
/// When it could not be asked.
pub async fn model_check(name: String) -> Result<Check, String> {
    call("model_check", json!({ "name": name })).await
}

/// Store `key` in the keychain for `variable`.
///
/// # Errors
///
/// When the keychain cannot store it.
pub async fn key_store(variable: String, key: String) -> Result<(), String> {
    call("key_store", json!({ "variable": variable, "key": key })).await
}

/// Remove the key stored for `variable`.
///
/// # Errors
///
/// When the keychain cannot remove it.
pub async fn key_remove(variable: String) -> Result<(), String> {
    call("key_remove", json!({ "variable": variable })).await
}

/// Where the key for `variable` is.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn key_status(variable: String) -> Result<KeyStatus, String> {
    call("key_status", json!({ "variable": variable })).await
}

/// Play the game the configuration `text` describes, its cues sent to
/// `cues`. It returns once the game has started.
///
/// # Errors
///
/// Why the game could not start, in full.
pub async fn play(text: &str, cues: &Cues) -> Result<(), String> {
    let args = object(&[("text", &text.into()), ("cues", &cues.channel)]);
    invoke("play", args).await.map(drop).map_err(error)
}

/// Stop the game being prepared or playing.
pub fn stop() {
    command("stop", object(&[]));
}

/// Reveal the last game's log in the system's file manager.
pub fn show_log() {
    command("show_log", object(&[]));
}

/// Open `url` in the system browser.
pub fn open_url(url: String) {
    command("plugin:opener|open_url", object(&[("url", &url.into())]));
}

/// Run the command `cmd` with `args`, in the background, and log its error
/// if it fails.
fn command(cmd: &'static str, args: JsValue) {
    spawn_local(async move {
        if let Err(failure) = invoke(cmd, args).await {
            web_sys::console::error_1(&format!("{cmd}: {}", error(failure)).into());
        }
    });
}
