//! The desktop app's backend, reached through `window.__TAURI__`, which
//! Tauri sets on the page (`withGlobalTauri`). Commands go through
//! `core.invoke`, events come to the window's listeners, and what a
//! command streams comes back down a [`Channel`], as JSON.

use std::marker::PhantomData;

use leptos::task::spawn_local;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

/// Tauri's own objects on the page.
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"])]
        pub async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

        /// Tauri's channel: what the backend sends down it reaches its
        /// `onmessage`.
        #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"])]
        pub type Channel;

        #[wasm_bindgen(constructor, js_namespace = ["window", "__TAURI__", "core"])]
        pub fn new() -> Channel;

        #[wasm_bindgen(method, setter)]
        pub fn set_onmessage(this: &Channel, handler: &js_sys::Function);

        /// The window the page is in.
        #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "webviewWindow"])]
        pub type WebviewWindow;

        #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "webviewWindow"], js_name = getCurrentWebviewWindow)]
        pub fn current_window() -> WebviewWindow;

        /// `handler` hears each `event` sent to this window or to every
        /// window.
        #[wasm_bindgen(catch, method, js_name = listen)]
        pub async fn listen_to(
            this: &WebviewWindow,
            event: &str,
            handler: &js_sys::Function,
        ) -> Result<JsValue, JsValue>;
    }
}

/// Whether the page is inside the desktop app.
pub fn in_tauri() -> bool {
    web_sys::window()
        .map(|window| {
            js_sys::Reflect::has(&window, &JsValue::from_str("__TAURI__")).unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Where what a command streams arrives: a channel, and what is done with
/// each `T` as it comes. It is kept for as long as the command streams.
pub struct Channel<T> {
    channel: js::Channel,
    _handler: Closure<dyn FnMut(JsValue)>,
    _sent: PhantomData<fn(T)>,
}

impl<T: DeserializeOwned + 'static> Channel<T> {
    /// A channel whose messages go to `take`, one at a time, in order.
    pub fn new(mut take: impl FnMut(T) + 'static) -> Self {
        let handler =
            Closure::new(
                move |message: JsValue| match serde_wasm_bindgen::from_value(message) {
                    Ok(message) => take(message),
                    Err(failure) => {
                        web_sys::console::error_1(&format!("a channel's message: {failure}").into())
                    }
                },
            );
        let channel = js::Channel::new();
        channel.set_onmessage(handler.as_ref().unchecked_ref());
        Self {
            channel,
            _handler: handler,
            _sent: PhantomData,
        }
    }
}

impl<T> AsRef<JsValue> for Channel<T> {
    /// The channel, as a command's argument.
    fn as_ref(&self) -> &JsValue {
        self.channel.as_ref()
    }
}

/// `args` as an object for a command.
pub fn object(args: &[(&str, &JsValue)]) -> JsValue {
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

/// The command `cmd`'s answer to `args`, an object such as [`object`]
/// makes, as JavaScript.
///
/// # Errors
///
/// The command's error.
pub async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, String> {
    js::invoke(cmd, args).await.map_err(error)
}

/// The command `cmd`'s answer to `args`, a JSON object.
///
/// # Errors
///
/// The command's error, or why its answer could not be read.
pub async fn call<T: DeserializeOwned>(cmd: &str, args: serde_json::Value) -> Result<T, String> {
    let args = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|error| error.to_string())?;
    let answer = invoke(cmd, args).await?;
    serde_wasm_bindgen::from_value(answer).map_err(|error| error.to_string())
}

/// Run the command `cmd` with the JSON `args`, in the background, and log
/// its error if it fails.
pub fn background(cmd: &'static str, args: serde_json::Value) {
    match args.serialize(&serde_wasm_bindgen::Serializer::json_compatible()) {
        Ok(args) => command(cmd, args),
        Err(failure) => web_sys::console::error_1(&format!("{cmd}: {failure}").into()),
    }
}

/// Run the command `cmd` with `args`, in the background, and log its error
/// if it fails.
pub fn command(cmd: &'static str, args: JsValue) {
    spawn_local(async move {
        if let Err(failure) = invoke(cmd, args).await {
            web_sys::console::error_1(&format!("{cmd}: {failure}").into());
        }
    });
}

/// Open `url` in the system browser.
pub fn open_url(url: String) {
    command("plugin:opener|open_url", object(&[("url", &url.into())]));
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
        if let Err(failure) = js::current_window().listen_to(event, &handler).await {
            web_sys::console::error_1(&format!("listening to {event}: {}", error(failure)).into());
        }
    });
}
