//! Providers for tests to talk to: each a listener on a port of the OS's
//! choosing in this process, so that no test reaches the network. Nothing
//! but tests uses this module.
//!
//! A provider that plays answers each completion request as the app's
//! canned [`Player`] does, so each app brings its own.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A canned player: the calls it makes to answer a completion request, in
/// the protocol's JSON, each as a tool's name and its arguments, or why it
/// cannot answer.
pub type Player = fn(&serde_json::Value) -> anyhow::Result<Vec<(String, serde_json::Value)>>;

/// A listener on a port of the OS's choosing, and the root of the API it
/// would serve.
pub fn bound() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    (listener, base_url)
}

/// A request as a canned provider received it.
#[derive(Debug)]
pub struct Served {
    /// The lines of its head, the request line first, lowercased so a
    /// header is found whatever its case.
    pub head: Vec<String>,
    /// Its body, empty when it had none.
    pub body: String,
}

impl Served {
    /// The body, parsed as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap()
    }

    /// The method and the path of the request line, without its query,
    /// such as `get` and `/v1/models`.
    fn asked(&self) -> (&str, &str) {
        let mut words = self.head[0].split_whitespace();
        let method = words.next().unwrap();
        let target = words.next().unwrap();
        (method, target.split('?').next().unwrap())
    }
}

/// An HTTP response of `status` carrying the JSON `body` on a connection
/// that closes after it.
fn response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A provider that answers one request with `status` and `body`, in a
/// thread that gives back the request it was sent.
pub fn answering(status: &str, body: &str) -> (String, JoinHandle<Served>) {
    let (listener, base_url) = bound();
    let response = response(status, body);
    let served = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let served = receive(&mut stream).expect("the request ended before it was whole");
        stream.write_all(response.as_bytes()).unwrap();
        served
    });
    (base_url, served)
}

/// The request arriving on `stream`: its head, and as much body as its
/// `Content-Length` says follows. None when the connection closes before
/// the request is whole.
fn receive(stream: &mut TcpStream) -> Option<Served> {
    let mut received = Vec::new();
    let mut buffer = [0; 1024];
    let end = loop {
        if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        let n = stream.read(&mut buffer).ok().filter(|&n| n > 0)?;
        received.extend_from_slice(&buffer[..n]);
    };
    let head: Vec<String> = String::from_utf8_lossy(&received[..end])
        .lines()
        .map(str::to_lowercase)
        .collect();
    let length = head
        .iter()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map_or(0, |value| value.trim().parse().unwrap());
    let mut body = received.split_off(end + 4);
    while body.len() < length {
        let n = stream.read(&mut buffer).ok().filter(|&n| n > 0)?;
        body.extend_from_slice(&buffer[..n]);
    }
    Some(Served {
        head,
        body: String::from_utf8(body).unwrap(),
    })
}

/// A listing of `ids` in OpenAI's shape, with the fields a provider adds
/// that are not read, each model's entry reporting a context window of
/// `window` tokens, as OpenRouter's does, when there is one.
pub fn listing(ids: &[&str], window: Option<u64>) -> String {
    let data: Vec<_> = ids
        .iter()
        .map(|id| {
            let mut model = serde_json::json!({"id": id, "object": "model", "owned_by": "test"});
            if let Some(tokens) = window {
                model["context_length"] = tokens.into();
            }
            model
        })
        .collect();
    serde_json::json!({"object": "list", "data": data}).to_string()
}

/// A provider that plays as [`playing`] does, its listing reporting a
/// context window of `tokens` for each of `ids`.
pub fn playing_within(player: Player, ids: &[&str], tokens: u64) -> String {
    serving_and(listing(ids, Some(tokens)), move |request| {
        answered(player(request))
    })
}

/// A provider that serves `ids`: the root of an API that answers one
/// request with their listing.
pub fn serving(ids: &[&str]) -> String {
    answering("200 OK", &listing(ids, None)).0
}

/// The root of an API nobody serves: a listener that hangs up on every
/// connection and is kept for the rest of the process, so that no other
/// test can be handed its port.
pub fn unreachable() -> String {
    let (listener, base_url) = bound();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    base_url
}

/// A provider that plays, kept for the rest of the process: it answers
/// every listing request with `ids`, and every completion request with the
/// calls `player` makes, as [`answered`] answers them. Anything else asked
/// of it is not found.
pub fn playing(player: Player, ids: &[&str]) -> String {
    playing_only(player, ids, ids)
}

/// A provider that plays as [`playing`] does and keeps every completion
/// request it is sent, as JSON, in order, for a test to read.
pub fn recording(player: Player, ids: &[&str]) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&requests);
    let base_url = serving_and(listing(ids, None), move |request| {
        kept.lock().unwrap().push(request.clone());
        answered(player(request))
    });
    (base_url, requests)
}

/// A provider that serves `ids` but plays only `selecting` of them, kept
/// for the rest of the process: it answers every listing request with
/// `ids`; every completion request for a model in `selecting` as
/// [`playing`] does; every other completion request 400 Bad Request,
/// saying what [`refusal`] says, as a server that cannot use the tools a
/// request offers does; and a completion request naming no model 400 Bad
/// Request too, saying so. Anything else asked of it is not found.
pub fn playing_only(player: Player, ids: &[&str], selecting: &[&str]) -> String {
    let selecting: Vec<String> = selecting.iter().map(|id| id.to_string()).collect();
    serving_and(listing(ids, None), move |request| {
        let Some(model) = request["model"].as_str() else {
            return response("400 Bad Request", &error("the request names no model"));
        };
        if selecting.iter().any(|id| id == model) {
            answered(player(request))
        } else {
            response("400 Bad Request", &error(&refusal(model)))
        }
    })
}

/// A provider that serves `ids` and plays each only when not made to call
/// a tool, kept for the rest of the process: a completion request whose
/// `tool_choice` is `"required"` is answered 400 Bad Request, saying what
/// [`required_refusal`] says, as a model that refuses to be made to call a
/// tool is; any other is answered as [`playing`] answers it.
pub fn refusing_required(player: Player, ids: &[&str]) -> String {
    refusing_when(
        player,
        ids,
        |request| request["tool_choice"] == "required",
        required_refusal,
    )
}

/// Whether the completion `request` offers the tool named `tool`.
fn offers(request: &serde_json::Value, tool: &str) -> bool {
    request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|offered| offered["function"]["name"] == tool)
}

/// A provider that serves `ids` and plays each as [`playing`] does, except
/// that a completion request offering the tool `tool` is answered 400 Bad
/// Request, saying what [`refusal`] says, kept for the rest of the process.
pub fn refusing_offers_of(player: Player, tool: &'static str, ids: &[&str]) -> String {
    refusing_when(player, ids, move |request| offers(request, tool), refusal)
}

/// A provider that serves `ids` and plays each as [`playing`] does, except
/// that a completion request offering the tool `tool` whose `tool_choice` is
/// `"required"` is refused as [`refusing_required`] refuses it, kept for
/// the rest of the process.
pub fn refusing_required_offering(player: Player, tool: &'static str, ids: &[&str]) -> String {
    refusing_when(
        player,
        ids,
        move |request| offers(request, tool) && request["tool_choice"] == "required",
        required_refusal,
    )
}

/// A provider that serves `ids` and plays each as [`playing`] does, except
/// that a completion request `refused` picks out is answered 400 Bad
/// Request, saying what `said` says of the model asked, kept for the rest
/// of the process.
pub fn refusing_when(
    player: Player,
    ids: &[&str],
    refused: impl Fn(&serde_json::Value) -> bool + Send + 'static,
    said: fn(&str) -> String,
) -> String {
    serving_and(listing(ids, None), move |request| {
        if refused(request) {
            let model = request["model"].as_str().unwrap_or_default();
            response("400 Bad Request", &error(&said(model)))
        } else {
            answered(player(request))
        }
    })
}

/// A provider that serves `ids`, refuses every completion request whose
/// `tool_choice` is `"required"` as [`refusing_required`] does, and answers
/// any other 429 Too Many Requests, as a provider out of patience does,
/// kept for the rest of the process.
pub fn refusing_required_then_busy(ids: &[&str]) -> String {
    serving_and(listing(ids, None), |request| {
        if request["tool_choice"] == "required" {
            let model = request["model"].as_str().unwrap_or_default();
            response("400 Bad Request", &error(&required_refusal(model)))
        } else {
            response("429 Too Many Requests", &error("slow down"))
        }
    })
}

/// What a canned provider says when `model` is asked for a completion that
/// requires a tool call, which it does not accept.
pub fn required_refusal(model: &str) -> String {
    format!("tool_choice \"required\" is not supported for {model}")
}

/// `calls` as a provider answers them: in a completion, or 400 Bad Request
/// saying why the player could not answer, as a server that cannot use the
/// tools a request offers does.
pub fn answered(calls: anyhow::Result<Vec<(String, serde_json::Value)>>) -> String {
    match calls {
        Ok(calls) => response("200 OK", &completion_of(&calls)),
        Err(refused) => response("400 Bad Request", &error(&format!("{refused:#}"))),
    }
}

/// A provider that serves `ids` but answers every completion request 401
/// Unauthorized, as one that wants an API key does, kept for the rest of
/// the process.
pub fn wanting_a_key(ids: &[&str]) -> String {
    serving_and(listing(ids, None), |_| {
        response("401 Unauthorized", &error("an API key is required"))
    })
}

/// A provider kept for the rest of the process that answers every listing
/// request with `listing` and every completion request, parsed as JSON,
/// with what `completing` makes of it. Anything else asked of it is not
/// found.
pub fn serving_and(
    listing: String,
    completing: impl Fn(&serde_json::Value) -> String + Send + 'static,
) -> String {
    let (listener, base_url) = bound();
    serve(listener, listing, completing);
    base_url
}

/// Answer every request to `listener`, for the rest of the process, as
/// [`serving_and`] answers it, the paths it serves found at the end of what
/// is asked, so that a base URL with a query still reaches them.
fn serve(
    listener: TcpListener,
    listing: String,
    completing: impl Fn(&serde_json::Value) -> String + Send + 'static,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(served) = receive(&mut stream) else {
                continue;
            };
            let answer = match served.asked() {
                ("get", path) if path.ends_with("/models") => response("200 OK", &listing),
                ("post", path) if path.ends_with("/chat/completions") => completing(&served.json()),
                _ => response("404 Not Found", "{}"),
            };
            let _ = stream.write_all(answer.as_bytes());
        }
    });
}

/// A key for a [`leaking`] provider, which no test has any other reason to
/// hold.
pub const CANARY: &str = "sk-canary-0123456789";

/// The variable a [`leaking`] provider's key is read from.
pub const CANARY_VARIABLE: &str = "FREE_AGENT_CANARY_KEY";

/// The userinfo in the base URL of a [`leaking`] provider.
pub const LEAKED_USERINFO: &str = "canary-user:canary-pass";

/// A provider that serves `ids` and plays each as [`playing`] does, but
/// gives away everything it is asked with, kept for the rest of the
/// process: its base URL carries [`LEAKED_USERINFO`] and `key` in its
/// query, every usage it reports holds `key` and the base URL, and the
/// first completion a player asks for, whose story opens `You are `, is
/// answered 400 Bad Request, saying both.
pub fn leaking(player: Player, ids: &[&str], key: &str) -> String {
    let (listener, root) = bound();
    let base_url = root.replacen("http://", &format!("http://{LEAKED_USERINFO}@"), 1);
    let base_url = format!("{base_url}?key={key}");
    let said = format!("{key} cannot be used at {base_url}");
    let usage = serde_json::json!({"total_tokens": 2, "key": key, "base_url": base_url});
    let failed = AtomicBool::new(false);
    serve(listener, listing(ids, None), move |request| {
        let asked = request["messages"]
            .as_array()
            .and_then(|messages| messages.last()?["content"].as_str())
            .unwrap_or_default();
        if asked.starts_with("You are ") && !failed.swap(true, atomic::Ordering::Relaxed) {
            return response("400 Bad Request", &error(&said));
        }
        let calls = player(request).expect("the canned player answers what a game asks");
        let mut completion: serde_json::Value =
            serde_json::from_str(&completion_of(&calls)).unwrap();
        completion["usage"] = usage.clone();
        response("200 OK", &completion.to_string())
    });
    base_url
}

/// Assert that `log`, written of a game played against a [`leaking`]
/// provider whose key is `key`, holds neither the key, nor either half of
/// the userinfo, nor the query, and that what the provider gave away is
/// there, redacted.
pub fn assert_redacted(log: &str, key: &str) {
    let (user, password) = LEAKED_USERINFO.split_once(':').unwrap();
    for leaked in [key, user, password, "?key="] {
        assert!(!log.contains(leaked), "{leaked} in {log}");
    }
    for redacted in [
        r#""key":"[redacted]""#,
        "[redacted] cannot be used at http://127.0.0.1:",
    ] {
        assert!(log.contains(redacted), "no {redacted} in {log}");
    }
}

/// A provider that serves `ids` and plays none of them, kept for the rest
/// of the process: every completion request is answered 400 Bad Request,
/// saying what [`refusal`] says, as [`playing_only`] answers one for a
/// model it does not play.
pub fn refusing(ids: &[&str]) -> String {
    serving_and(listing(ids, None), |request| {
        let model = request["model"].as_str().unwrap_or_default();
        response("400 Bad Request", &error(&refusal(model)))
    })
}

/// What a canned provider says when asked for a completion from `model`,
/// which it does not play.
pub fn refusal(model: &str) -> String {
    format!("{model} does not support tools")
}

/// What OpenAI answers a request with, 429 Too Many Requests, when the
/// account is out of credits.
pub const NO_CREDITS: &str = r#"{
    "error": {
        "message": "You have no credits remaining. Add credits to continue using the API at https://platform.openai.com/settings/organization/billing/.",
        "type": "insufficient_quota",
        "param": null,
        "code": "insufficient_quota"
    }
}"#;

/// An error in OpenAI's shape saying `message`.
fn error(message: &str) -> String {
    serde_json::json!({"error": {"message": message, "type": "invalid_request_error"}}).to_string()
}

/// The usage every canned completion reports.
pub fn reported() -> serde_json::Value {
    serde_json::json!({"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2})
}

/// A completion in OpenAI's shape whose one choice calls the tool `name`
/// with `arguments`, with the fields a provider adds that are not read.
pub fn completion(name: &str, arguments: &serde_json::Value) -> String {
    completion_of(&[(name.to_string(), arguments.clone())])
}

/// A completion in OpenAI's shape whose one choice makes each of `calls`, a
/// tool's name and its arguments, in order, with the fields a provider adds
/// that are not read.
pub fn completion_of(calls: &[(String, serde_json::Value)]) -> String {
    let tool_calls: Vec<serde_json::Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (name, arguments))| {
            serde_json::json!({
                "id": format!("call_{}", i + 1),
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            })
        })
        .collect();
    serde_json::json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": "test",
        "choices": [{
            "index": 0,
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": tool_calls,
            },
        }],
        "usage": reported(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{first_choice, selecting};
    use crate::{Model, Provider, REQUEST_TIMEOUT, Request, Response, ToolChoice};
    use serde_json::json;

    /// The provider at `base_url`, asked with no key.
    fn provider(base_url: &str) -> Provider {
        Provider::new(base_url, None, REQUEST_TIMEOUT).unwrap()
    }

    /// What the provider at `base_url` answers `request`, as the error's
    /// words when it fails.
    async fn asked(base_url: &str, request: &Request) -> Result<Response, String> {
        let answer = provider(base_url).complete(request).await;
        answer.map_err(|error| format!("{error:#}"))
    }

    /// The canned player's selection of `target`.
    fn selected(target: &str) -> Response {
        Response::tool_call("select", &json!({"target": target})).reporting(reported())
    }

    #[tokio::test]
    async fn a_request_longer_than_one_read_is_received_whole() {
        let answer = completion("select", &json!({"target": "a"}));
        let (base_url, served) = answering("200 OK", &answer);
        let mut request = selecting(&["a"]);
        request.messages[1] = crate::Message::user("x".repeat(4096));
        provider(&base_url).complete(&request).await.unwrap();
        let received = served.join().unwrap().json();
        assert_eq!(received, serde_json::to_value(&request).unwrap());
    }

    #[tokio::test]
    async fn the_playing_provider_lists_its_models_and_plays_as_its_player() {
        let base_url = playing(first_choice, &["qwen2.5-7b-instruct"]);
        let provider = provider(&base_url);
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
        provider.listed("qwen2.5-7b-instruct").await.unwrap();
        for candidates in [&["Player 3", "Player 2"][..], &["Player 1"]] {
            assert_eq!(
                provider.complete(&selecting(candidates)).await.unwrap(),
                selected(candidates[0])
            );
        }
        // A request its player cannot answer is refused as bad, as a
        // server would.
        let error = asked(&base_url, &selecting(&[])).await.unwrap_err();
        assert!(error.contains("400 Bad Request"), "{error}");
        assert!(error.contains("nothing to choose from"), "{error}");
    }

    #[tokio::test]
    async fn the_provider_playing_within_a_window_reports_it_for_each_model_and_plays() {
        let base_url = playing_within(first_choice, &["qwen2.5-7b-instruct"], 32768);
        let listed = provider(&base_url).listed("qwen2.5-7b-instruct").await;
        assert_eq!(listed.unwrap().context_window, Some(32768));
        let answer = asked(&base_url, &selecting(&["Player 2"])).await;
        assert_eq!(answer.unwrap(), selected("Player 2"));
    }

    #[tokio::test]
    async fn the_recording_provider_plays_and_keeps_every_request() {
        let (base_url, requests) = recording(first_choice, &["qwen2.5-7b-instruct"]);
        let request = selecting(&["Player 2"]);
        assert_eq!(
            asked(&base_url, &request).await.unwrap(),
            selected("Player 2")
        );
        let kept = requests.lock().unwrap().clone();
        assert_eq!(kept, [serde_json::to_value(&request).unwrap()]);
    }

    #[tokio::test]
    async fn the_provider_playing_only_some_models_refuses_a_completion_from_the_others() {
        let ids = ["qwen2.5-7b-instruct", "gpt-0"];
        let base_url = playing_only(first_choice, &ids, &["qwen2.5-7b-instruct"]);
        assert_eq!(provider(&base_url).models().await.unwrap(), ids);
        let answer = asked(&base_url, &selecting(&["Player 2"])).await;
        assert_eq!(answer.unwrap(), selected("Player 2"));
        let mut request = selecting(&["Player 2"]);
        request.model = "gpt-0".to_string();
        let error = asked(&base_url, &request).await.unwrap_err();
        assert!(error.contains("400 Bad Request"), "{error}");
        assert!(error.contains(&refusal("gpt-0")), "{error}");
        // The refusing provider plays nothing.
        let base_url = refusing(&["qwen2.5-7b-instruct"]);
        provider(&base_url)
            .listed("qwen2.5-7b-instruct")
            .await
            .unwrap();
        let error = asked(&base_url, &selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(error.contains(&refusal("qwen2.5-7b-instruct")), "{error}");
    }

    #[tokio::test]
    async fn the_refusing_providers_refuse_only_the_requests_they_pick_out() {
        let id = "qwen2.5-7b-instruct";
        let auto = Request {
            tool_choice: ToolChoice::Auto,
            ..selecting(&["Player 2"])
        };
        // Made to call a tool.
        let base_url = refusing_required(first_choice, &[id]);
        let error = asked(&base_url, &selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(error.contains(&required_refusal(id)), "{error}");
        assert_eq!(asked(&base_url, &auto).await.unwrap(), selected("Player 2"));
        // Offered a tool.
        let base_url = refusing_offers_of(first_choice, "select", &[id]);
        let error = asked(&base_url, &auto).await.unwrap_err();
        assert!(error.contains(&refusal(id)), "{error}");
        let base_url = refusing_offers_of(first_choice, "vote", &[id]);
        assert_eq!(asked(&base_url, &auto).await.unwrap(), selected("Player 2"));
        // Made to call a tool, offered it.
        let base_url = refusing_required_offering(first_choice, "select", &[id]);
        let error = asked(&base_url, &selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(error.contains(&required_refusal(id)), "{error}");
        assert_eq!(asked(&base_url, &auto).await.unwrap(), selected("Player 2"));
        // Made to call a tool, then out of patience.
        let base_url = refusing_required_then_busy(&[id]);
        let error = asked(&base_url, &selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(error.contains(&required_refusal(id)), "{error}");
        let error = asked(&base_url, &auto).await.unwrap_err();
        assert!(error.contains("429 Too Many Requests"), "{error}");
    }

    #[tokio::test]
    async fn the_leaking_provider_gives_away_its_key_and_base_url() {
        let base_url = leaking(first_choice, &["qwen2.5-7b-instruct"], CANARY);
        assert!(base_url.contains(LEAKED_USERINFO), "{base_url}");
        assert!(base_url.ends_with(&format!("?key={CANARY}")), "{base_url}");
        let mut request = selecting(&["Player 2"]);
        request.messages[1] = crate::Message::user("You are Player 1.");
        let error = asked(&base_url, &request).await.unwrap_err();
        assert!(
            error.contains(&format!("{CANARY} cannot be used at")),
            "{error}"
        );
        // Only the first time; after that it plays, its usage telling all.
        let answer = asked(&base_url, &request).await.unwrap();
        let usage = answer.usage.unwrap();
        assert_eq!(usage["key"], CANARY);
        assert_eq!(usage["base_url"], base_url);
        assert_redacted(
            r#"{"key":"[redacted]"} [redacted] cannot be used at http://127.0.0.1:"#,
            CANARY,
        );
    }

    #[tokio::test]
    async fn the_playing_provider_refuses_a_completion_naming_no_model_and_goes_on_serving() {
        let base_url = playing(first_choice, &["qwen2.5-7b-instruct"]);
        let client = reqwest::Client::new();
        let answer = client
            .post(format!("{base_url}/chat/completions"))
            .json(&json!({"messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 400);
        assert!(
            answer.text().await.unwrap().contains("names no model"),
            "{base_url}"
        );
        assert_eq!(
            provider(&base_url).models().await.unwrap(),
            ["qwen2.5-7b-instruct"]
        );
    }

    #[tokio::test]
    async fn the_provider_wanting_a_key_lists_its_models_and_answers_every_completion_401() {
        let base_url = wanting_a_key(&["qwen2.5-7b-instruct"]);
        assert_eq!(
            provider(&base_url).models().await.unwrap(),
            ["qwen2.5-7b-instruct"]
        );
        let error = asked(&base_url, &selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(error.contains("401 Unauthorized"), "{error}");
        assert!(error.contains("an API key is required"), "{error}");
    }

    #[tokio::test]
    async fn the_playing_provider_outlives_a_hang_up_and_finds_nothing_at_any_other_path() {
        let base_url = playing(first_choice, &["qwen2.5-7b-instruct"]);
        let address = base_url
            .trim_start_matches("http://")
            .trim_end_matches("/v1");
        drop(TcpStream::connect(address).unwrap());
        let mut asking = TcpStream::connect(address).unwrap();
        asking
            .write_all(b"GET /v1/elsewhere HTTP/1.1\r\nHost: test\r\n\r\n")
            .unwrap();
        let mut answer = String::new();
        asking.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 404 Not Found\r\n"), "{answer}");
        assert_eq!(
            provider(&base_url).models().await.unwrap(),
            ["qwen2.5-7b-instruct"]
        );
    }
}
