// SPDX-License-Identifier: AGPL-3.0-only
//! Owned outbound socket. Successful upgrade is not verified readiness or consent.

use super::reverse_peer::{run_peer, WriteRoute};
use super::reverse_protocol::{decode_frame, encode_frame, Frame, MAX_WIRE_BYTES};
use super::{valid_id, DeviceCredential, RelayClient, RelayError, RELAY_ORIGIN};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};

const PROTOCOL: &str = "wenlan.reverse.v1";
const IO_TIMEOUT: Duration = Duration::from_secs(5);
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Enforced separately from user-supplied logging directives: the upstream
/// library traces raw handshake credentials and message bodies.
pub(crate) fn transport_log_filter() -> tracing_subscriber::filter::Targets {
    tracing_subscriber::filter::Targets::new()
        .with_default(tracing::level_filters::LevelFilter::TRACE)
        .with_target("tungstenite", tracing::level_filters::LevelFilter::OFF)
        .with_target(
            "tokio_tungstenite",
            tracing::level_filters::LevelFilter::OFF,
        )
}

pub(crate) struct ReverseConnection {
    connection_id: String,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), RelayError>>>,
}

impl std::fmt::Debug for ReverseConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReverseConnection([redacted])")
    }
}

impl ReverseConnection {
    pub(crate) fn connection_id(&self) -> &str {
        &self.connection_id
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Stops transport only. It never represents server-confirmed revocation.
    pub(crate) async fn shutdown(mut self) -> Result<(), RelayError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Retain ownership across await so cancellation still runs Drop's abort.
        if let Some(task) = self.task.as_mut() {
            match tokio::time::timeout(IO_TIMEOUT, &mut *task).await {
                Ok(result) => result.map_err(|_| RelayError::Unavailable)?,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    Err(RelayError::Unavailable)
                }
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for ReverseConnection {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Aborting the owner drops its JoinSet and aborts all local HTTP tasks.
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn upgrade_request(
    origin: &reqwest::Url,
    credential: &DeviceCredential,
) -> Result<Request, RelayError> {
    credential.validate()?;
    let production = origin.as_str().trim_end_matches('/') == RELAY_ORIGIN;
    let fixture = cfg!(test)
        && origin.scheme() == "http"
        && origin.host_str() == Some("127.0.0.1")
        && origin.port().is_some()
        && origin.username().is_empty()
        && origin.password().is_none()
        && origin.path() == "/"
        && origin.query().is_none()
        && origin.fragment().is_none();
    if !production && !fixture {
        return Err(RelayError::InvalidInput);
    }
    let mut target = origin.clone();
    target
        .set_scheme(if production { "wss" } else { "ws" })
        .map_err(|_| RelayError::InvalidInput)?;
    target.set_path("/devices/reverse/connect");
    target.set_query(None);
    target.set_fragment(None);
    let mut request = target
        .as_str()
        .into_client_request()
        .map_err(|_| RelayError::InvalidInput)?;
    let headers = request.headers_mut();
    let mut bearer = format!("Bearer {}", credential.management_token)
        .parse::<reqwest::header::HeaderValue>()
        .map_err(|_| RelayError::InvalidInput)?;
    bearer.set_sensitive(true);
    headers.insert("authorization", bearer);
    headers.insert(
        "x-wenlan-device-id",
        credential
            .id
            .parse()
            .map_err(|_| RelayError::InvalidInput)?,
    );
    headers.insert(
        "sec-websocket-protocol",
        PROTOCOL.parse().map_err(|_| RelayError::InvalidInput)?,
    );
    Ok(request)
}

fn connection_id(response: &Response) -> Result<String, RelayError> {
    if response.status() != 101
        || response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok())
            != Some(PROTOCOL)
    {
        return Err(RelayError::InvalidResponse);
    }
    let id = response
        .headers()
        .get("x-wenlan-connection-id")
        .and_then(|value| value.to_str().ok())
        .ok_or(RelayError::InvalidResponse)?;
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(RelayError::InvalidResponse);
    }
    Ok(id.to_owned())
}

fn upgrade_error(error: SocketError) -> RelayError {
    match &error {
        SocketError::Http(response) => log::warn!(
            "[remote-access] relay upgrade refused: HTTP {}",
            response.status().as_u16()
        ),
        other => log::warn!("[remote-access] relay upgrade failed: {other}"),
    }
    match error {
        SocketError::Http(response) => match response.status().as_u16() {
            401 | 403 => RelayError::Unauthorized,
            429 => RelayError::RateLimited {
                retry_after_seconds: response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|v| *v <= 3600),
            },
            _ => RelayError::Unavailable,
        },
        _ => RelayError::Unavailable,
    }
}

impl RelayClient {
    pub(crate) async fn connect_reverse(
        &self,
        credential: &DeviceCredential,
        port: u16,
        write_route: WriteRoute,
        backend_token: String,
    ) -> Result<ReverseConnection, RelayError> {
        if port == 0 || !valid_id(&backend_token, 32) {
            return Err(RelayError::InvalidInput);
        }
        let request = upgrade_request(&self.origin, credential)?;
        let config = WebSocketConfig::default()
            .read_buffer_size(16 * 1024)
            .write_buffer_size(0)
            .max_write_buffer_size(2 * MAX_WIRE_BYTES)
            .max_message_size(Some(MAX_WIRE_BYTES))
            .max_frame_size(Some(MAX_WIRE_BYTES));
        // connect_async performs one handshake, without redirect or proxy fallback.
        let (socket, response) = tokio::time::timeout(
            IO_TIMEOUT,
            connect_async_with_config(request, Some(config), true),
        )
        .await
        .map_err(|_| {
            log::warn!("[remote-access] relay upgrade timed out");
            RelayError::Unavailable
        })?
        .map_err(upgrade_error)?;
        let id = connection_id(&response)?;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(run_socket(
            socket,
            port,
            write_route,
            backend_token,
            stopped,
        ));
        Ok(ReverseConnection {
            connection_id: id,
            stop: Some(stop),
            task: Some(task),
        })
    }
}

/// Records why an established relay channel ended. Reasons are fixed labels,
/// close metadata or error kinds, never frame bodies or credentials.
fn ended(reason: impl std::fmt::Display, error: RelayError) -> RelayError {
    log::warn!("[remote-access] relay connection ended: {reason}");
    error
}

fn close_reason(frame: Option<&CloseFrame>) -> String {
    match frame {
        Some(frame) => {
            let reason: String = frame.reason.as_str().chars().take(64).collect();
            format!(
                "relay closed it (code {}, {reason:?})",
                u16::from(frame.code)
            )
        }
        None => "relay closed it (no close code)".into(),
    }
}

async fn run_socket(
    socket: Socket,
    port: u16,
    write_route: WriteRoute,
    token: String,
    mut stopped: oneshot::Receiver<()>,
) -> Result<(), RelayError> {
    let (mut sink, mut stream) = socket.split();
    let (requests, incoming) = mpsc::channel::<Frame>(16);
    let (outgoing, mut frames) = mpsc::channel::<Frame>(16);
    let mut peers = JoinSet::new();
    peers.spawn(run_peer(port, write_route, token, incoming, outgoing));
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;
    let mut awaiting_pong = false;
    let mut frame_window = tokio::time::Instant::now();
    let mut frame_count = 0usize;
    let result = async {
        loop {
            tokio::select! {
                _ = &mut stopped => return Ok(()),
                completed = peers.join_next() => return match completed {
                    Some(Ok(Ok(()))) => {
                        log::warn!("[remote-access] relay connection ended: local peer stopped");
                        Ok(())
                    }
                    Some(Ok(Err(error))) => {
                        let reason = format!("local peer failed: {error}");
                        Err(ended(reason, error))
                    }
                    _ => Err(ended("local peer task failed", RelayError::Unavailable)),
                },
                _ = ping.tick() => {
                    if awaiting_pong { return Err(ended("missed pong", RelayError::Unavailable)); }
                    awaiting_pong = true;
                    tokio::time::timeout(IO_TIMEOUT, sink.send(Message::Ping(Vec::new().into())))
                        .await.map_err(|_| ended("ping send timed out", RelayError::Unavailable))?
                        .map_err(|error| ended(format_args!("ping send failed: {error}"), RelayError::Unavailable))?;
                },
                frame = frames.recv() => {
                    let frame = frame.ok_or_else(|| ended("local peer closed its frame queue", RelayError::Unavailable))?;
                    let text = encode_frame(&frame).map_err(|_| ended("frame encode failed", RelayError::InvalidResponse))?;
                    tokio::time::timeout(IO_TIMEOUT, sink.send(Message::Text(text.into())))
                        .await.map_err(|_| ended("frame send timed out", RelayError::Unavailable))?
                        .map_err(|error| ended(format_args!("frame send failed: {error}"), RelayError::Unavailable))?;
                },
                message = stream.next() => {
                    if frame_window.elapsed() >= Duration::from_secs(60) {
                        frame_window = tokio::time::Instant::now(); frame_count = 0;
                    }
                    frame_count += 1;
                    if frame_count > 16_384 { return Err(ended("frame rate cap exceeded", RelayError::Unavailable)); }
                    match message.ok_or_else(|| ended("stream ended without a close frame", RelayError::Unavailable))?
                        .map_err(|error| ended(format_args!("read failed: {error}"), RelayError::Unavailable))? {
                        Message::Text(text) => {
                            let frame = decode_frame(text.as_str()).map_err(|_| ended("frame decode failed", RelayError::InvalidResponse))?;
                            tokio::time::timeout(IO_TIMEOUT, requests.send(frame)).await
                                .map_err(|_| ended("local peer queue full", RelayError::Unavailable))?
                                .map_err(|_| ended("local peer queue closed", RelayError::Unavailable))?;
                        },
                        Message::Pong(_) => awaiting_pong = false,
                        Message::Ping(_) => {
                            tokio::time::timeout(IO_TIMEOUT, sink.flush()).await
                                .map_err(|_| ended("pong flush timed out", RelayError::Unavailable))?
                                .map_err(|error| ended(format_args!("pong flush failed: {error}"), RelayError::Unavailable))?;
                        },
                        Message::Close(frame) => return Err(ended(close_reason(frame.as_ref()), RelayError::Unavailable)),
                        _ => return Err(ended("unexpected message type", RelayError::InvalidResponse)),
                    }
                },
            }
        }
    }.await;
    drop(requests);
    peers.abort_all();
    while peers.join_next().await.is_some() {}
    let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    result
}

#[cfg(test)]
mod tests;
