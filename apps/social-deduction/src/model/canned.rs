//! Providers for tests to talk to: each a listener on a port of the OS's
//! choosing in this process, so that no test reaches the network. Nothing
//! but tests uses this module.

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
}

/// A provider that answers one request with `status` and `body`, in a
/// thread that gives back the request it was sent.
pub fn answering(status: &str, body: &str) -> (String, JoinHandle<Served>) {
    let (listener, base_url) = bound();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let served = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let served = receive(&mut stream);
        stream.write_all(response.as_bytes()).unwrap();
        served
    });
    (base_url, served)
}

/// The request arriving on `stream`: its head, and as much body as its
/// `Content-Length` says follows.
fn receive(stream: &mut TcpStream) -> Served {
    let mut received = Vec::new();
    let mut buffer = [0; 1024];
    let end = loop {
        if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "the request ended before its head did");
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
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "the request ended before its body did");
        body.extend_from_slice(&buffer[..n]);
    }
    Served {
        head,
        body: String::from_utf8(body).unwrap(),
    }
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
