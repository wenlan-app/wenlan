// SPDX-License-Identifier: AGPL-3.0-only
use super::*;
use crate::remote_relay::now_ms;
use crate::remote_relay::reverse_protocol::{decode_body, HeaderMap};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_hdr_async;

fn device() -> DeviceCredential {
    DeviceCredential {
        id: "d".repeat(64),
        management_token: "m".repeat(64),
        expires_at: now_ms() + 60_000,
    }
}

#[test]
fn upgrade_is_pinned_and_credentials_stay_out_of_url_and_debug() {
    let credential = device();
    let origin = reqwest::Url::parse(RELAY_ORIGIN).unwrap();
    let request = upgrade_request(&origin, &credential).unwrap();
    assert_eq!(
        request.uri(),
        "wss://relay.wenlan.app/devices/reverse/connect"
    );
    assert_eq!(
        request.headers()["authorization"],
        format!("Bearer {}", credential.management_token)
    );
    assert!(request.headers()["authorization"].is_sensitive());
    for forbidden in ["origin", "cookie"] {
        assert!(!request.headers().contains_key(forbidden));
    }
    assert!(!format!("{request:?}").contains(&credential.management_token));
    for origin in [
        "https://evil.invalid",
        "https://relay.wenlan.app/other",
        "http://relay.wenlan.app",
        "http://127.0.0.1:1234/other",
    ] {
        assert_eq!(
            upgrade_request(&reqwest::Url::parse(origin).unwrap(), &credential).unwrap_err(),
            RelayError::InvalidInput
        );
    }
    let mut expired = credential;
    expired.expires_at = 1;
    assert_eq!(
        upgrade_request(&origin, &expired).unwrap_err(),
        RelayError::Unauthorized
    );
}

#[test]
fn server_upgrade_requires_exact_protocol_and_connection_identity() {
    for (protocol, id) in [
        ("other", "a".repeat(64)),
        (PROTOCOL, "A".repeat(64)),
        (PROTOCOL, "a".repeat(63)),
    ] {
        let response = tokio_tungstenite::tungstenite::http::Response::builder()
            .status(101)
            .header("sec-websocket-protocol", protocol)
            .header("x-wenlan-connection-id", id)
            .body(None)
            .unwrap();
        assert_eq!(connection_id(&response), Err(RelayError::InvalidResponse));
    }
    let response = tokio_tungstenite::tungstenite::http::Response::builder()
        .status(101)
        .header("sec-websocket-protocol", PROTOCOL)
        .header("x-wenlan-connection-id", "a".repeat(64))
        .body(None)
        .unwrap();
    assert_eq!(connection_id(&response).unwrap(), "a".repeat(64));
}

#[test]
fn transport_logs_are_disabled_even_with_a_trace_subscriber() {
    use tracing_subscriber::prelude::*;
    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Buffer(bytes.clone());
    let subscriber = tracing_subscriber::registry()
        .with(transport_log_filter())
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(tracing_subscriber::EnvFilter::new("trace")),
        );
    tracing::subscriber::with_default(subscriber, || {
        tracing::trace!(target:"tungstenite::handshake::client", "PRIVATE_BEARER");
        tracing::debug!(target:"tokio_tungstenite", "PRIVATE_BODY");
        tracing::info!(target:"wenlan_lib::remote_relay", "visible_sanitized_status");
    });
    let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(!output.contains("PRIVATE"));
    assert!(output.contains("visible_sanitized_status"));
}

async fn socket_server() -> (RelayClient, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin =
        reqwest::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    (RelayClient::build(origin, false).unwrap(), listener)
}

#[tokio::test]
async fn cancelling_shutdown_does_not_detach_the_owned_task() {
    let (started, started_rx) = oneshot::channel();
    let (finished, mut finished_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _finished = finished;
        let _ = started.send(());
        std::future::pending::<Result<(), RelayError>>().await
    });
    let abort = task.abort_handle();
    started_rx.await.unwrap();
    let connection = ReverseConnection {
        connection_id: "a".repeat(64),
        stop: None,
        task: Some(task),
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(10), connection.shutdown())
            .await
            .is_err()
    );
    let released = tokio::time::timeout(Duration::from_millis(100), &mut finished_rx)
        .await
        .is_ok();
    // Clean up the owned fixture even when the regression is present.
    abort.abort();
    if !released {
        let _ = tokio::time::timeout(Duration::from_secs(1), finished_rx)
            .await
            .expect("owned fixture must stop after explicit cleanup");
    }
    assert!(released, "cancelled shutdown detached its background task");
}

#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "The tungstenite handshake callback requires its unboxed ErrorResponse type"
)]
async fn actual_socket_forwards_a_credited_loopback_response_and_shuts_down() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = backend.local_addr().unwrap().port();
    let http = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut received = Vec::new();
        while !received.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut buf = [0; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0);
            received.extend_from_slice(&buf[..n]);
            assert!(received.len() < 8192);
        }
        let request = String::from_utf8(received).unwrap();
        assert!(request.starts_with("GET /connector-info HTTP/1.1"));
        assert!(request.contains(&format!("authorization: Bearer {}", "b".repeat(43))));
        assert!(!request.contains(&"m".repeat(64)));
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
    });
    let (client, listener) = socket_server().await;
    let (finished_tx, finished_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hdr_async(stream, |request:&Request,mut response:tokio_tungstenite::tungstenite::handshake::server::Response| {
            assert_eq!(request.uri().path(),"/devices/reverse/connect");
            assert!(request.uri().query().is_none());
            assert_eq!(request.headers()["authorization"],format!("Bearer {}","m".repeat(64)));
            assert!(!request.headers().contains_key("cookie"));
            response.headers_mut().insert("sec-websocket-protocol",PROTOCOL.parse().unwrap());
            response.headers_mut().insert("x-wenlan-connection-id","a".repeat(64).parse().unwrap());
            Ok(response)
        }).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization".into(), format!("Bearer {}", "b".repeat(43)));
        let request = Frame::Request {
            id: "native-1".into(),
            path: "/connector-info".into(),
            method: "GET".into(),
            headers,
            body: String::new(),
        };
        socket
            .send(Message::Text(encode_frame(&request).unwrap().into()))
            .await
            .unwrap();
        let response = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(matches!(
            decode_frame(&response).unwrap(),
            Frame::Response { status: 200, .. }
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(30), socket.next())
                .await
                .is_err()
        );
        socket
            .send(Message::Text(
                encode_frame(&Frame::Credit {
                    id: "native-1".into(),
                    seq: 0,
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let chunk = socket.next().await.unwrap().unwrap().into_text().unwrap();
        match decode_frame(&chunk).unwrap() {
            Frame::Chunk { seq, body, .. } => {
                assert_eq!(seq, 0);
                assert_eq!(decode_body(&body).unwrap(), b"{}");
            }
            _ => panic!("Expected chunk"),
        }
        socket
            .send(Message::Text(
                encode_frame(&Frame::Credit {
                    id: "native-1".into(),
                    seq: 1,
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let end = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(matches!(decode_frame(&end).unwrap(), Frame::End { .. }));
        finished_tx.send(()).unwrap();
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
        let _ = socket.flush().await;
    });
    let connection = client
        .connect_reverse(&device(), port, WriteRoute::default(), "b".repeat(43))
        .await
        .unwrap();
    assert_eq!(connection.connection_id(), "a".repeat(64));
    assert!(!connection.is_finished());
    tokio::time::timeout(Duration::from_secs(5), finished_rx)
        .await
        .unwrap()
        .unwrap();
    connection.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    http.await.unwrap();
}

#[tokio::test]
async fn handshake_rejection_is_generic_and_never_retried() {
    let (client, listener) = socket_server().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0; 4096];
        let _ = stream.read(&mut buf).await.unwrap();
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    });
    let error = client
        .connect_reverse(&device(), 12345, WriteRoute::default(), "b".repeat(43))
        .await
        .unwrap_err();
    assert_eq!(error, RelayError::Unauthorized);
    assert!(!error.to_string().contains("PRIVATE"));
    server.await.unwrap();
}

#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "The tungstenite handshake callback requires its unboxed ErrorResponse type"
)]
async fn dropping_socket_owner_cancels_a_pending_local_http_request() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = backend.local_addr().unwrap().port();
    let (started_tx, started_rx) = oneshot::channel();
    let http = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut buf = [0; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            assert!(bytes.len() < 8192);
        }
        started_tx.send(()).unwrap();
        let mut buf = [0; 1];
        match stream.read(&mut buf).await {
            Ok(n) => assert_eq!(n, 0),
            Err(error) => assert!(matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            )),
        }
    });
    let (client, listener) = socket_server().await;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket=accept_hdr_async(stream, |_:&Request,mut response:tokio_tungstenite::tungstenite::handshake::server::Response| {
            response.headers_mut().insert("sec-websocket-protocol",PROTOCOL.parse().unwrap());
            response.headers_mut().insert("x-wenlan-connection-id","a".repeat(64).parse().unwrap());
            Ok(response)
        }).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization".into(), format!("Bearer {}", "b".repeat(43)));
        let frame = Frame::Request {
            id: "pending-local".into(),
            path: "/mcp".into(),
            method: "GET".into(),
            headers,
            body: String::new(),
        };
        socket
            .send(Message::Text(encode_frame(&frame).unwrap().into()))
            .await
            .unwrap();
        match socket.next().await {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {}
            _ => panic!("Unexpected response after owner drop"),
        }
    });
    let connection = client
        .connect_reverse(&device(), port, WriteRoute::default(), "b".repeat(43))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), started_rx)
        .await
        .unwrap()
        .unwrap();
    drop(connection);
    tokio::time::timeout(Duration::from_secs(3), http)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
