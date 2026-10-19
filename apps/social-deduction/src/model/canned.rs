//! Providers for tests to talk to: each a listener on a port of the OS's
//! choosing in this process, so that no test reaches the network. Nothing
//! but tests uses this module.

use super::fake::canned_calls;
use crate::report::seat_order;
use std::cmp::Ordering;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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
pub fn playing_within(ids: &[&str], tokens: u64) -> String {
    serving_and(listing(ids, Some(tokens)), played)
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
/// every listing request with `ids`, and every completion request as the
/// canned player does, with the calls [`canned_calls`] makes: offered one
/// tool, it calls it with the first value its enumerated parameter allows,
/// as a player's `select` tool asks for a `target` from an `enum` of
/// candidates; offered several, it speaks and acts once a phase and waits
/// after that. Anything else asked of it is not found.
pub fn playing(ids: &[&str]) -> String {
    playing_only(ids, ids)
}

/// A provider that plays as [`playing`] does and keeps every completion
/// request it is sent, as JSON, in order, for a test to read.
pub fn recording(ids: &[&str]) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&requests);
    let base_url = serving_and(listing(ids, None), move |request| {
        kept.lock().unwrap().push(request.clone());
        played(request)
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
pub fn playing_only(ids: &[&str], selecting: &[&str]) -> String {
    let selecting: Vec<String> = selecting.iter().map(|id| id.to_string()).collect();
    serving_and(listing(ids, None), move |request| {
        let Some(model) = request["model"].as_str() else {
            return response("400 Bad Request", &error("the request names no model"));
        };
        if selecting.iter().any(|id| id == model) {
            played(request)
        } else {
            response("400 Bad Request", &error(&refusal(model)))
        }
    })
}

/// A provider that serves `ids` and plays each by night alone, kept for
/// the rest of the process: a completion request offering one tool is
/// answered as [`playing`] answers it, and one offering several, as a
/// day's does, 400 Bad Request saying what [`day_refusal`] says, as a
/// server that takes one tool at a time does.
pub fn refusing_days(ids: &[&str]) -> String {
    refusing_when(
        ids,
        |request| request["tools"].as_array().map_or(0, Vec::len) > 1,
        day_refusal,
    )
}

/// A provider that serves `ids` and plays each only when not made to call
/// a tool, kept for the rest of the process: a completion request whose
/// `tool_choice` is `"required"` is answered 400 Bad Request, saying what
/// [`required_refusal`] says, as a model that refuses to be made to call a
/// tool is; any other is answered as [`playing`] answers it.
pub fn refusing_required(ids: &[&str]) -> String {
    refusing_when(
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
pub fn refusing_offers_of(tool: &'static str, ids: &[&str]) -> String {
    refusing_when(ids, move |request| offers(request, tool), refusal)
}

/// A provider that serves `ids` and plays each as [`playing`] does, except
/// that a completion request offering the tool `tool` whose `tool_choice` is
/// `"required"` is refused as [`refusing_required`] refuses it, kept for
/// the rest of the process.
pub fn refusing_required_offering(tool: &'static str, ids: &[&str]) -> String {
    refusing_when(
        ids,
        move |request| offers(request, tool) && request["tool_choice"] == "required",
        required_refusal,
    )
}

/// A provider that serves `ids` and plays each as [`playing`] does, except
/// that a completion request `refused` picks out is answered 400 Bad
/// Request, saying what `said` says of the model asked, kept for the rest
/// of the process.
fn refusing_when(
    ids: &[&str],
    refused: impl Fn(&serde_json::Value) -> bool + Send + 'static,
    said: fn(&str) -> String,
) -> String {
    serving_and(listing(ids, None), move |request| {
        if refused(request) {
            let model = request["model"].as_str().unwrap_or_default();
            response("400 Bad Request", &error(&said(model)))
        } else {
            played(request)
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

/// What a canned provider says when asked for a completion from `model`
/// with several tools, which it does not take.
pub fn day_refusal(model: &str) -> String {
    format!("{model} takes one tool at a time")
}

/// The canned player's answer to the completion `request`, in the
/// protocol's JSON: the calls [`canned_calls`] makes, as [`answered`]
/// answers them.
fn played(request: &serde_json::Value) -> String {
    answered(canned_calls(request))
}

/// `calls` as a provider answers them: in a completion, or 400 Bad Request
/// saying why the player could not answer, as a server that cannot use the
/// tools a request offers does.
fn answered(calls: anyhow::Result<Vec<(String, serde_json::Value)>>) -> String {
    match calls {
        Ok(calls) => response("200 OK", &completion_of(&calls)),
        Err(refused) => response("400 Bad Request", &error(&format!("{refused:#}"))),
    }
}

/// A provider that plays as [`playing`] does, kept for the rest of the
/// process, except that the first seat alive retracts its vote by day:
/// whenever the canned player would speak and vote, it calls `retract`
/// after.
pub fn retracting(ids: &[&str]) -> String {
    serving_and(listing(ids, None), |request| {
        answered(canned_calls(request).map(|mut calls| {
            if first_seat_voting(request, &calls) {
                calls.push(("retract".to_string(), serde_json::json!({})));
            }
            calls
        }))
    })
}

/// Whether `calls`, the canned player's answer to `request`, are `speak`
/// then `select`, from a voter who comes before every candidate `select`
/// offers in seat order. The voter is named by the story the request tells,
/// which opens with who the player is, and by day the candidates are every
/// other player alive, so this is the first seat alive, voting by day.
fn first_seat_voting(request: &serde_json::Value, calls: &[(String, serde_json::Value)]) -> bool {
    let [(speak, _), (select, _)] = calls else {
        return false;
    };
    let asked = request["messages"]
        .as_array()
        .and_then(|messages| messages.last()?["content"].as_str())
        .unwrap_or_default();
    let me = asked
        .strip_prefix("You are ")
        .and_then(|rest| rest.split([',', '.']).next())
        .unwrap_or_default()
        .to_string();
    let mut candidates = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| &tool["function"])
        .filter(|function| function["name"] == "select")
        .filter_map(|function| function["parameters"]["properties"]["target"]["enum"].as_array())
        .flatten()
        .filter_map(serde_json::Value::as_str);
    (speak.as_str(), select.as_str()) == ("speak", "select")
        && !me.is_empty()
        && candidates.all(|candidate| seat_order(&me, &candidate.to_string()) == Ordering::Less)
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
fn serving_and(
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
pub const CANARY_VARIABLE: &str = "SOCIAL_DEDUCTION_CANARY_KEY";

/// A quick Werewolf configuration file whose players are the model served at
/// `base_url`, a [`leaking`] provider's, asked with the key in
/// [`CANARY_VARIABLE`], and random.
pub fn leaking_werewolf(base_url: &str) -> String {
    format!(
        "game = \"werewolf\"\n[phases]\nnight_limit = \"1s\"\nday_limit = \"2s\"\nratify_limit = \"1s\"\n\
         [models.qwen]\nbase_url = \"{base_url}\"\nid = \"qwen2.5-7b-instruct\"\n\
         api_key_env = \"{CANARY_VARIABLE}\"\n\
         [strategies.plain]\npolicy = \"prompted\"\nsystem = \"Play.\"\n\
         [roles.werewolf]\nstrategy = \"random\"\n\
         [roles.villager]\nstrategy = \"plain\"\n\
         [roles.doctor]\nstrategy = \"plain\"\n\
         [roles.seer]\nstrategy = \"plain\"\n"
    )
}

/// The userinfo in the base URL of a [`leaking`] provider.
pub const LEAKED_USERINFO: &str = "canary-user:canary-pass";

/// A provider that serves `ids` and plays each as [`playing`] does, but
/// gives away everything it is asked with, kept for the rest of the
/// process: its base URL carries [`LEAKED_USERINFO`] and `key` in its
/// query, every usage it reports holds `key` and the base URL, and the
/// first completion a player asks for is answered 400 Bad Request, saying
/// both.
pub fn leaking(ids: &[&str], key: &str) -> String {
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
        let calls = canned_calls(request).expect("the canned player answers what a game asks");
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

/// A provider that serves `ids` and plays none of them: every completion
/// request is answered 400 Bad Request, as [`playing_only`] answers one for
/// a model it does not play.
pub fn refusing(ids: &[&str]) -> String {
    playing_only(ids, &[])
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
    use crate::model::fake::{SAID, by_day, selecting};
    use crate::model::{Model, Provider, REQUEST_TIMEOUT, Response};
    use serde_json::json;

    #[tokio::test]
    async fn the_playing_provider_lists_its_models_and_selects_the_first_candidate_every_time() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
        provider.listed("qwen2.5-7b-instruct").await.unwrap();
        for candidates in [&["Player 3", "Player 2"][..], &["Player 1"]] {
            assert_eq!(
                provider.complete(&selecting(candidates)).await.unwrap(),
                Response::tool_call("select", &json!({"target": candidates[0]}))
                    .reporting(reported())
            );
        }
    }

    #[tokio::test]
    async fn the_playing_provider_speaks_and_votes_by_day_and_refuses_a_request_it_cannot_play() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let answer = provider
            .complete(&by_day(&["Player 2", "Player 3"], 60, "Day 1."))
            .await
            .unwrap();
        assert_eq!(
            answer,
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "Player 2"})),
            ])
            .reporting(reported())
        );
        // A request offering a tool it cannot use is refused as bad, as a
        // server would.
        let error = provider.complete(&selecting(&[])).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("400 Bad Request"), "{shown}");
        assert!(shown.contains("select"), "{shown}");
    }

    #[tokio::test]
    async fn the_retracting_provider_has_the_first_seat_alive_retract_its_vote_after_voting() {
        let base_url = retracting(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let first = provider
            .complete(&by_day(
                &["Player 2", "Player 3"],
                60,
                "You are Player 1, a villager.\n\nDay 1.",
            ))
            .await
            .unwrap();
        assert_eq!(
            first,
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "Player 2"})),
                ("retract".to_string(), json!({})),
            ])
            .reporting(reported())
        );
        // Any other seat plays as the playing provider does.
        let other = provider
            .complete(&by_day(
                &["Player 1", "Player 2"],
                60,
                "You are Player 3, the seer.\n\nDay 1.",
            ))
            .await
            .unwrap();
        assert_eq!(
            other,
            Response::calling(&[
                ("speak".to_string(), json!({"text": SAID})),
                ("select".to_string(), json!({"target": "Player 1"})),
            ])
            .reporting(reported())
        );
        // Seats are in the order of their numbers, and the first is the one
        // that sorts before every candidate, not only the one it votes
        // against.
        let seated = |me: &str, candidates: &[&str]| {
            by_day(
                candidates,
                60,
                &format!("You are {me}, a villager.\n\nDay 1."),
            )
        };
        let tenth = provider
            .complete(&seated("Player 2", &["Player 10", "Player 3"]))
            .await
            .unwrap();
        assert_eq!(tenth.tool_calls().len(), 3);
        for (me, candidates) in [
            ("Player 10", &["Player 2", "Player 11"][..]),
            ("Player 3", &["Player 4", "Player 1"]),
        ] {
            let other = provider.complete(&seated(me, candidates)).await.unwrap();
            assert_eq!(other.tool_calls().len(), 2, "{me}");
        }
        // So does the first seat once it has voted, and by night.
        let again = provider
            .complete(&by_day(
                &["Player 2", "Player 3"],
                60,
                "You are Player 1, a villager.\n\nDay 1.\nYou vote against Player 2.",
            ))
            .await
            .unwrap();
        assert_eq!(
            again,
            Response::tool_call("wait", &json!({"seconds": 60})).reporting(reported())
        );
        assert_eq!(
            provider.complete(&selecting(&["Player 2"])).await.unwrap(),
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
    }

    #[tokio::test]
    async fn the_provider_playing_within_a_window_reports_it_for_each_model_and_plays() {
        let base_url = playing_within(&["qwen2.5-7b-instruct"], 32768);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let listed = provider.listed("qwen2.5-7b-instruct").await.unwrap();
        assert_eq!(listed.context_window, Some(32768));
        assert_eq!(
            provider.complete(&selecting(&["Player 2"])).await.unwrap(),
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
    }

    #[tokio::test]
    async fn the_provider_playing_only_some_models_refuses_a_completion_from_the_others() {
        let base_url = playing_only(&["qwen2.5-7b-instruct", "gpt-0"], &["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(
            provider.models().await.unwrap(),
            ["qwen2.5-7b-instruct", "gpt-0"]
        );
        assert_eq!(
            provider.complete(&selecting(&["Player 2"])).await.unwrap(),
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
        let mut request = selecting(&["Player 2"]);
        request.model = "gpt-0".to_string();
        let error = provider.complete(&request).await.unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("400 Bad Request"), "{shown}");
        assert!(shown.contains(&refusal("gpt-0")), "{shown}");
        // The refusing provider plays nothing.
        let base_url = refusing(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        provider.listed("qwen2.5-7b-instruct").await.unwrap();
        let error = provider
            .complete(&selecting(&["Player 2"]))
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains(&refusal("qwen2.5-7b-instruct")),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn the_provider_refusing_days_selects_by_night_and_refuses_a_request_offering_several_tools()
     {
        let base_url = refusing_days(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
        assert_eq!(
            provider.complete(&selecting(&["Player 2"])).await.unwrap(),
            Response::tool_call("select", &json!({"target": "Player 2"})).reporting(reported())
        );
        let error = provider
            .complete(&by_day(&["Player 2", "Player 3"], 60, "Day 1."))
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("400 Bad Request"), "{shown}");
        assert!(
            shown.contains(&day_refusal("qwen2.5-7b-instruct")),
            "{shown}"
        );
    }

    #[tokio::test]
    async fn the_playing_provider_refuses_a_completion_naming_no_model_and_goes_on_serving() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
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
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
    }

    #[tokio::test]
    async fn the_provider_wanting_a_key_lists_its_models_and_answers_every_completion_401() {
        let base_url = wanting_a_key(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
        let error = provider
            .complete(&selecting(&["Player 2"]))
            .await
            .unwrap_err();
        let shown = format!("{error:#}");
        assert!(shown.contains("401 Unauthorized"), "{shown}");
        assert!(shown.contains("an API key is required"), "{shown}");
    }

    #[tokio::test]
    async fn the_playing_provider_outlives_a_hang_up_and_finds_nothing_at_any_other_path() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
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
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
    }
}
