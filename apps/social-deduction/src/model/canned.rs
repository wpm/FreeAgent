//! Providers for tests to talk to: each a listener on a port of the OS's
//! choosing in this process, so that no test reaches the network. Nothing
//! but tests uses this module.

use super::fake::first_enumerated;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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

    /// The method and the path of the request line, such as `get` and
    /// `/v1/models`.
    fn asked(&self) -> (&str, &str) {
        let mut words = self.head[0].split_whitespace();
        (words.next().unwrap(), words.next().unwrap())
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
/// that are not read.
pub fn listing(ids: &[&str]) -> String {
    let data: Vec<_> = ids
        .iter()
        .map(|id| serde_json::json!({"id": id, "object": "model", "owned_by": "test"}))
        .collect();
    serde_json::json!({"object": "list", "data": data}).to_string()
}

/// A provider that serves `ids`: the root of an API that answers one
/// request with their listing.
pub fn serving(ids: &[&str]) -> String {
    answering("200 OK", &listing(ids)).0
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
/// every listing request with `ids`, and every completion request by
/// calling the tool the request forces with the first value its enumerated
/// parameter allows, as a player's `select` tool asks for a `target` from
/// an `enum` of candidates. Anything else asked of it is not found.
pub fn playing(ids: &[&str]) -> String {
    let (listener, base_url) = bound();
    let listing = listing(ids);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some(served) = receive(&mut stream) else {
                continue;
            };
            let answer = match served.asked() {
                ("get", "/v1/models") => response("200 OK", &listing),
                ("post", "/v1/chat/completions") => {
                    response("200 OK", &selecting_first(&served.json()))
                }
                _ => response("404 Not Found", "{}"),
            };
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    base_url
}

/// The completion that calls the one tool `request`, a completion request
/// in the protocol's JSON, offers, with the first value its enumerated
/// parameter allows.
fn selecting_first(request: &serde_json::Value) -> String {
    let function = &request["tools"][0]["function"];
    let (parameter, first) = first_enumerated(&function["parameters"]).unwrap();
    completion(
        function["name"].as_str().unwrap(),
        &serde_json::json!({parameter: first}),
    )
}

/// A completion in OpenAI's shape whose one choice calls the tool `name`
/// with `arguments`, with the fields a provider adds that are not read.
pub fn completion(name: &str, arguments: &serde_json::Value) -> String {
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
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": name, "arguments": arguments.to_string()},
                }],
            },
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fake::selecting;
    use crate::model::{Model, Provider, REQUEST_TIMEOUT, Response};
    use serde_json::json;

    #[tokio::test]
    async fn the_playing_provider_lists_its_models_and_selects_the_first_candidate_every_time() {
        let base_url = playing(&["qwen2.5-7b-instruct"]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert_eq!(provider.models().await.unwrap(), ["qwen2.5-7b-instruct"]);
        provider.check_serves("qwen2.5-7b-instruct").await.unwrap();
        for candidates in [&["player3", "player2"][..], &["player1"]] {
            assert_eq!(
                provider.complete(&selecting(candidates)).await.unwrap(),
                Response::tool_call("select", &json!({"target": candidates[0]}))
            );
        }
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
