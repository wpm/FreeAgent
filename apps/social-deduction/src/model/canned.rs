//! Providers for tests to talk to: each a listener on a port of the OS's
//! choosing in this process, so that no test reaches the network. Nothing
//! but tests uses this module.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

/// A listener on a port of the OS's choosing, and the root of the API it
/// would serve.
pub fn bound() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    (listener, base_url)
}

/// A provider that answers one request with `status` and `body`, in a
/// thread that gives back the head of the request it was sent.
pub fn answering(status: &str, body: &str) -> (String, JoinHandle<String>) {
    let (listener, base_url) = bound();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let served = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut buffer = [0; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0, "the request ended before its head did");
            head.extend_from_slice(&buffer[..n]);
        }
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(head).unwrap()
    });
    (base_url, served)
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
