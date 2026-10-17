//! The desktop app's backend, reached through `window.__TAURI__`, which
//! Tauri sets on the page (`withGlobalTauri`). Commands go through
//! `core.invoke`, and a game's cues come back down a `core.Channel`, as
//! JSON.

use leptos::task::spawn_local;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use social_deduction_stage::settings::{
    AppSettings, FieldError, GameSettings, KeyStatus, Listing, Progress, Provider, Snapshot,
};
use social_deduction_stage::{Cue, GameName};
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

    /// The window the page is in.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "webviewWindow"])]
    type WebviewWindow;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "webviewWindow"], js_name = getCurrentWebviewWindow)]
    fn current_window() -> WebviewWindow;

    /// `handler` hears each `event` sent to this window or to every
    /// window.
    #[wasm_bindgen(catch, method, js_name = listen)]
    async fn listen_to(
        this: &WebviewWindow,
        event: &str,
        handler: &js_sys::Function,
    ) -> Result<JsValue, JsValue>;
}

/// Whether the page is inside the desktop app.
pub fn in_tauri() -> bool {
    web_sys::window()
        .map(|window| {
            js_sys::Reflect::has(&window, &JsValue::from_str("__TAURI__")).unwrap_or(false)
        })
        .unwrap_or(false)
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

/// The settings, what is wrong with them, and where logs go.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn settings_get() -> Result<Snapshot, String> {
    call("settings_get", json!({})).await
}

/// Make the settings `settings`, and what is wrong with them.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn settings_set(settings: &AppSettings) -> Result<Vec<FieldError>, String> {
    call("settings_set", json!({ "settings": settings })).await
}

/// Open the Settings window at the field `path`, or as it was.
pub fn settings_open(path: Option<Vec<String>>) {
    background("settings_open", json!({ "path": path }));
}

/// The settings `game` starts from.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn game_defaults(game: GameName) -> Result<GameSettings, String> {
    call("game_defaults", json!({ "game": game })).await
}

/// The providers, local servers that answer among them.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn providers() -> Result<Vec<Provider>, String> {
    call("providers", json!({})).await
}

/// The models the provider at `base_url` lists now, with what probing
/// found of each, its probes forgotten first when `forget` says. Its
/// models not yet probed are probed in the background.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn listing(
    base_url: String,
    api_key_env: Option<String>,
    forget: bool,
) -> Result<Listing, String> {
    let args = json!({ "baseUrl": base_url, "apiKeyEnv": api_key_env, "forget": forget });
    call("listing", args).await
}

/// How far probing each provider has got.
///
/// # Errors
///
/// When the backend cannot be reached.
pub async fn checking() -> Result<Vec<Progress>, String> {
    call("checking", json!({})).await
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

/// Play a game of `game` under the settings, its cues sent to `cues`. It
/// returns once the game has started.
///
/// # Errors
///
/// Why the game could not start, in full.
pub async fn play(game: GameName, cues: &Cues) -> Result<(), String> {
    let game = serde_wasm_bindgen::to_value(&game).map_err(|error| error.to_string())?;
    let args = object(&[("game", &game), ("cues", &cues.channel)]);
    invoke("play", args).await.map(drop).map_err(error)
}

/// Stop the game being prepared or playing.
pub fn stop() {
    command("stop", object(&[]));
}

/// Open `url` in the system browser.
pub fn open_url(url: String) {
    command("plugin:opener|open_url", object(&[("url", &url.into())]));
}

/// Run the command `cmd` with the JSON `args`, in the background, and log
/// its error if it fails.
fn background(cmd: &'static str, args: serde_json::Value) {
    match args.serialize(&serde_wasm_bindgen::Serializer::json_compatible()) {
        Ok(args) => command(cmd, args),
        Err(failure) => web_sys::console::error_1(&format!("{cmd}: {failure}").into()),
    }
}

/// Have `handle` hear each `event` the backend sends to this window or to
/// every window, its payload as JSON, for as long as the page is open.
/// What it sends another window, such as the main window's `settings`,
/// goes by.
pub fn listen<T: DeserializeOwned>(event: &'static str, mut handle: impl FnMut(T) + 'static) {
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |received: JsValue| {
        let payload =
            js_sys::Reflect::get(&received, &JsValue::from_str("payload")).unwrap_or(JsValue::NULL);
        match serde_wasm_bindgen::from_value(payload) {
            Ok(payload) => handle(payload),
            Err(failure) => web_sys::console::error_1(&format!("{event}: {failure}").into()),
        }
    });
    // Kept for as long as the page is open.
    let handler: js_sys::Function = handler.into_js_value().unchecked_into();
    spawn_local(async move {
        if let Err(failure) = current_window().listen_to(event, &handler).await {
            web_sys::console::error_1(&format!("listening to {event}: {}", error(failure)).into());
        }
    });
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
