// SPDX-License-Identifier: Apache-2.0
//! A tiny scripted HTTP stub standing in for the daemon.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Serve one JSON response per `(request line, body)` route, in any order,
/// then stop. A request line is `"GET /api/pages/x"`. Returns the base URL
/// plus a channel of `(request line, request body)` for each request served.
pub fn spawn_stub(routes: Vec<(&'static str, String)>) -> (String, Receiver<(String, String)>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let base = format!("http://{}", listener.local_addr().expect("stub address"));
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..routes.len() {
            let (stream, _) = listener.accept().expect("accept request");
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader
                .read_line(&mut request_line)
                .expect("read request line");
            let request_line = request_line.trim_end().to_string();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).expect("read request body");
            let route = request_line
                .strip_suffix(" HTTP/1.1")
                .unwrap_or(&request_line);
            let (status, payload) = match routes.iter().find(|(r, _)| *r == route) {
                Some((_, payload)) => ("200 OK", payload.clone()),
                None => ("404 Not Found", "{\"error\":\"no such route\"}".to_string()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            // Record the request before answering it. The command under test
            // exits as soon as it has read the response, and its test then reads
            // this channel without waiting; recorded after the write, a request
            // the command already got its answer for could still be missing.
            let _ = sent.send((
                route.to_string(),
                String::from_utf8_lossy(&body).into_owned(),
            ));
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });
    (base, received)
}
