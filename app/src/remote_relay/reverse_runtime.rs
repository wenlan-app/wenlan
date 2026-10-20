// SPDX-License-Identifier: AGPL-3.0-only
//! Bounded native lifecycle for an authenticated reverse relay connection.

use super::reverse_client::ReverseCandidate;
use super::reverse_peer::WriteRoute;
use super::reverse_socket::ReverseConnection;
use super::runtime::RenewalError;
use super::store::{Profile, Store, StoreError};
use super::{RelayClient, RelayError};
use std::time::Duration;

const READINESS_TIMEOUT: Duration = Duration::from_secs(10);
const INITIAL_STATUS_BACKOFF: Duration = Duration::from_millis(100);
const MAX_STATUS_BACKOFF: Duration = Duration::from_secs(1);
const REVOKE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct ActiveReverse {
    pub(crate) connection: ReverseConnection,
    pub(crate) profile: Profile,
}

impl std::fmt::Debug for ActiveReverse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ActiveReverse([redacted])")
    }
}

/// Open the native reverse connection using the profile currently persisted by
/// the app. The profile is never silently re-enrolled or rotated.
pub(crate) async fn connect(
    profile: Profile,
    port: u16,
    write_route: WriteRoute,
) -> Result<ActiveReverse, RenewalError> {
    connect_at(
        Store::current(),
        RelayClient::new()?,
        profile,
        port,
        write_route,
    )
    .await
}

/// Testable lifecycle entrypoint. The store and client are injected so tests
/// cannot touch the user's profile or the production relay.
pub(super) async fn connect_at(
    store: Store,
    client: RelayClient,
    profile: Profile,
    port: u16,
    write_route: WriteRoute,
) -> Result<ActiveReverse, RenewalError> {
    if port == 0 {
        return Err(RenewalError::Relay(RelayError::InvalidInput));
    }
    same_profile(&store, &profile)
        .await
        .map_err(RenewalError::Profile)?;

    let (profile, credential) = match profile.device().cloned() {
        Some(device) => (profile, device),
        None => {
            let candidate = ReverseCandidate {
                backend_token: profile.backend_token().to_owned(),
                space: profile.space().to_owned(),
            };
            let prepared = client
                .prepare_reverse(&candidate)
                .await
                .map_err(RenewalError::Relay)?;
            let pending = prepared.credential;
            let expected_revision = profile.revision().to_owned();
            let store_for_attach = store.clone();
            let pending_for_store = pending.clone();
            let attached = tokio::task::spawn_blocking(move || {
                store_for_attach.attach_device(&expected_revision, pending_for_store)
            })
            .await
            .map_err(|_| RenewalError::Profile(storage_failure()))
            .and_then(|result| result.map_err(|error| RenewalError::Profile(error.to_string())));

            let profile = match attached {
                Ok(profile) => profile,
                Err(error) => {
                    revoke_pending(&client, &pending).await;
                    return Err(error);
                }
            };
            let credential = profile
                .device()
                .cloned()
                .ok_or_else(|| RenewalError::Profile(StoreError::Stale.to_string()))?;
            (profile, credential)
        }
    };

    same_profile(&store, &profile)
        .await
        .map_err(RenewalError::Profile)?;

    let connection = client
        .connect_reverse(
            &credential,
            port,
            write_route,
            profile.backend_token().to_owned(),
        )
        .await
        .map_err(RenewalError::Relay)?;

    if let Err(error) = wait_until_ready(&client, &connection, &credential).await {
        let _ = connection.shutdown().await;
        return Err(error);
    }

    if let Err(error) = same_profile(&store, &profile).await {
        let _ = connection.shutdown().await;
        return Err(RenewalError::Profile(error));
    }

    Ok(ActiveReverse {
        connection,
        profile,
    })
}

/// Confirm that a captured connection still belongs to the enabled profile and
/// that this exact relay connection is active. This does not renew a lease or
/// mutate local credentials.
pub(crate) async fn check(profile: &Profile, connection_id: &str) -> Result<(), RenewalError> {
    check_at(
        Store::current(),
        RelayClient::new()?,
        profile,
        connection_id,
    )
    .await
}

async fn check_at(
    store: Store,
    client: RelayClient,
    profile: &Profile,
    connection_id: &str,
) -> Result<(), RenewalError> {
    same_profile(&store, profile)
        .await
        .map_err(RenewalError::Profile)?;
    let device = profile
        .device()
        .ok_or_else(|| RenewalError::Profile(StoreError::Stale.to_string()))?;
    let active = client
        .reverse_status(device, connection_id)
        .await
        .map_err(RenewalError::Relay)?;
    if active.is_some() {
        Ok(())
    } else {
        Err(RenewalError::Relay(RelayError::Unavailable))
    }
}

fn not_ready(reason: &str) -> RenewalError {
    log::warn!("[remote-access] relay connection not ready: {reason}");
    RenewalError::Relay(RelayError::Unavailable)
}

async fn wait_until_ready(
    client: &RelayClient,
    connection: &ReverseConnection,
    credential: &super::DeviceCredential,
) -> Result<(), RenewalError> {
    let deadline = tokio::time::Instant::now() + READINESS_TIMEOUT;
    let mut backoff = INITIAL_STATUS_BACKOFF;

    loop {
        if connection.is_finished() {
            return Err(RenewalError::Relay(RelayError::Unavailable));
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(not_ready("the relay did not confirm it in time"));
        }
        let status = tokio::time::timeout(
            remaining,
            client.reverse_status(credential, connection.connection_id()),
        )
        .await
        .map_err(|_| not_ready("the relay status check timed out"))?
        .map_err(|error| {
            log::warn!("[remote-access] relay connection not ready: status check failed: {error}");
            RenewalError::Relay(error)
        })?;

        if connection.is_finished() {
            return Err(RenewalError::Relay(RelayError::Unavailable));
        }
        if status.is_some() {
            return Ok(());
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(not_ready("the relay did not confirm it in time"));
        }
        tokio::time::sleep(backoff.min(remaining)).await;
        backoff = (backoff + backoff).min(MAX_STATUS_BACKOFF);
    }
}

async fn revoke_pending(client: &RelayClient, credential: &super::DeviceCredential) {
    let _ = tokio::time::timeout(REVOKE_TIMEOUT, client.revoke_device(credential)).await;
}

async fn same_profile(store: &Store, expected: &Profile) -> Result<(), String> {
    let store = store.clone();
    let revision = expected.revision().to_owned();
    tokio::task::spawn_blocking(move || {
        let current = store
            .load()?
            .filter(Profile::enabled)
            .ok_or(StoreError::NotConfigured)?;
        if current.revision() == revision {
            Ok(())
        } else {
            Err(StoreError::Stale)
        }
    })
    .await
    .map_err(|_| storage_failure())?
    .map_err(|error| error.to_string())
}

fn storage_failure() -> String {
    "Remote access storage task failed".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_relay::{now_ms, DeviceCredential};
    use futures_util::StreamExt;
    use reqwest::Url;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use tokio_tungstenite::WebSocketStream;

    const PROTOCOL: &str = "wenlan.reverse.v1";

    fn configured() -> (tempfile::TempDir, Store, Profile) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::in_directory(directory.path().join("relay"));
        let profile = store.configure(None, "review").unwrap();
        let profile = store.enable(profile.revision()).unwrap();
        (directory, store, profile)
    }

    fn credential(expires_at: u64) -> DeviceCredential {
        DeviceCredential {
            id: "d".repeat(64),
            management_token: "m".repeat(64),
            expires_at,
        }
    }

    fn client(port: u16) -> RelayClient {
        RelayClient::build(
            Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
            false,
        )
        .unwrap()
    }

    async fn http_request(listener: &TcpListener) -> (TcpStream, String) {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 2048];
            let length = stream.read(&mut buffer).await.unwrap();
            assert!(length > 0);
            bytes.extend_from_slice(&buffer[..length]);
            let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let body_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map(|value| value.parse::<usize>().unwrap())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + body_length {
                return (stream, String::from_utf8(bytes).unwrap());
            }
        }
    }

    async fn http_response(stream: &mut TcpStream, status: u16, body: &str) {
        let response = format!(
            "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }

    #[expect(
        clippy::result_large_err,
        reason = "The tungstenite handshake callback requires its unboxed ErrorResponse type"
    )]
    async fn websocket(listener: &TcpListener) -> WebSocketStream<TcpStream> {
        let (stream, _) = listener.accept().await.unwrap();
        accept_hdr_async(stream, |request: &Request, mut response: Response| {
            assert_eq!(request.uri().path(), "/devices/reverse/connect");
            response
                .headers_mut()
                .insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
            response
                .headers_mut()
                .insert("x-wenlan-connection-id", "a".repeat(64).parse().unwrap());
            Ok(response)
        })
        .await
        .unwrap()
    }

    fn prepared(device: &DeviceCredential) -> String {
        serde_json::json!({
            "id": device.id.clone(),
            "managementToken": device.management_token.clone(),
            "expiresAt": device.expires_at,
            "pendingUntil": now_ms() + 30_000,
        })
        .to_string()
    }

    #[tokio::test]
    async fn stale_before_network_and_missing_or_disabled_profiles_do_not_open_io() {
        let (_directory, store, profile) = configured();
        let current = store
            .attach_device(profile.revision(), credential(now_ms() + 60_000))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = connect_at(
            store.clone(),
            client(listener.local_addr().unwrap().port()),
            profile,
            12345,
            WriteRoute::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, RenewalError::Profile(message) if message == StoreError::Stale.to_string())
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );

        let missing_directory = tempfile::tempdir().unwrap();
        let missing_store = Store::in_directory(missing_directory.path().join("missing"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = connect_at(
            missing_store,
            client(listener.local_addr().unwrap().port()),
            current,
            12345,
            WriteRoute::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, RenewalError::Profile(message) if message == StoreError::NotConfigured.to_string())
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );

        let (_directory, store, profile) = configured();
        let disabled = store.disable(profile.revision()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = connect_at(
            store,
            client(listener.local_addr().unwrap().port()),
            disabled,
            12345,
            WriteRoute::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, RenewalError::Profile(message) if message == StoreError::NotConfigured.to_string())
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn zero_port_fails_before_reverse_prepare() {
        let (_directory, store, profile) = configured();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = connect_at(
            store,
            client(listener.local_addr().unwrap().port()),
            profile,
            0,
            WriteRoute::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            RenewalError::Relay(RelayError::InvalidInput)
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn saved_credential_exists_before_a_rejected_websocket() {
        let (_directory, store, profile) = configured();
        let pending = credential(now_ms() + 60_000);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn({
            let listener = listener;
            let pending = pending.clone();
            async move {
                let (mut request, first) = http_request(&listener).await;
                assert!(first.starts_with("POST /devices/reverse "));
                http_response(&mut request, 201, &prepared(&pending)).await;
                let (mut stream, request) = http_request(&listener).await;
                assert!(request.starts_with("GET /devices/reverse/connect "));
                stream
                    .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE")
                    .await
                    .unwrap();
            }
        });
        let error = connect_at(
            store.clone(),
            client(port),
            profile,
            12345,
            WriteRoute::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            RenewalError::Relay(RelayError::Unauthorized)
        ));
        let saved = store.load().unwrap().unwrap();
        assert_eq!(
            saved.device().unwrap().management_token,
            pending.management_token
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stale_attach_revokes_pending_credential_without_opening_websocket() {
        let (_directory, store, profile) = configured();
        let pending = credential(now_ms() + 60_000);
        let revision = profile.revision().to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn({
            let store = store.clone();
            let listener = listener;
            let pending = pending.clone();
            async move {
                let (mut request, first) = http_request(&listener).await;
                assert!(first.starts_with("POST /devices/reverse "));
                store.disable(&revision).unwrap();
                http_response(&mut request, 201, &prepared(&pending)).await;
                let (mut request, revoke) = http_request(&listener).await;
                assert!(revoke.starts_with("POST /devices/revoke "));
                http_response(&mut request, 200, r#"{"success":true}"#).await;
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            }
        });
        let error = connect_at(store, client(port), profile, 12345, WriteRoute::default())
            .await
            .unwrap_err();
        assert!(
            matches!(error, RenewalError::Profile(message) if message == StoreError::Stale.to_string())
        );
        assert!(server.await.unwrap());
    }

    #[tokio::test]
    async fn existing_device_connects_without_enrollment_or_refresh() {
        let (_directory, store, profile) = configured();
        let device = credential(now_ms() + 60_000);
        let profile = store
            .attach_device(profile.revision(), device.clone())
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn({
            let listener = listener;
            async move {
                let mut socket = websocket(&listener).await;
                let (mut request, first) = http_request(&listener).await;
                assert!(first.starts_with("GET /devices/reverse/status "));
                http_response(&mut request, 200, r#"{"connected":true,"generation":1}"#).await;
                let _ = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
            }
        });
        let active = connect_at(
            store.clone(),
            client(port),
            profile.clone(),
            12345,
            WriteRoute::default(),
        )
        .await;
        let active = active.unwrap();
        assert_eq!(active.profile.revision(), profile.revision());
        assert_eq!(active.profile.device().unwrap().id, device.id);
        active.connection.shutdown().await.unwrap();
        server.await.unwrap();
        let saved = store.load().unwrap().unwrap();
        assert_eq!(
            saved.device().unwrap().management_token,
            device.management_token
        );
    }

    #[tokio::test]
    async fn check_confirms_the_connection_without_mutating_the_profile() {
        let (_directory, store, profile) = configured();
        let profile = store
            .attach_device(profile.revision(), credential(now_ms() + 60_000))
            .unwrap();
        let before = store.load().unwrap().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut request, request_text) = http_request(&listener).await;
            assert!(request_text.starts_with("GET /devices/reverse/status "));
            http_response(&mut request, 200, r#"{"connected":true,"generation":1}"#).await;
        });
        check_at(store.clone(), client(port), &profile, &"a".repeat(64))
            .await
            .unwrap();
        server.await.unwrap();
        let after = store.load().unwrap().unwrap();
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.backend_token(), before.backend_token());
        assert_eq!(
            after.device().unwrap().management_token,
            before.device().unwrap().management_token
        );
    }

    #[tokio::test]
    async fn invalid_or_expired_credentials_fail_before_websocket_io() {
        for malformed in [false, true] {
            let (_directory, store, profile) = configured();
            let device = credential(now_ms() + 60_000);
            let profile = store.attach_device(profile.revision(), device).unwrap();
            // Storage correctly rejects already-invalid credentials. Corrupt
            // only the captured input to exercise the network preflight guard.
            let mut value = serde_json::to_value(&profile).unwrap();
            if malformed {
                value["device"]["managementToken"] = serde_json::json!("short");
            } else {
                value["device"]["expiresAt"] = serde_json::json!(1);
            }
            let profile = serde_json::from_value(value).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let error = connect_at(
                store,
                client(listener.local_addr().unwrap().port()),
                profile,
                12345,
                WriteRoute::default(),
            )
            .await
            .unwrap_err();
            assert!(matches!(
                error,
                RenewalError::Relay(RelayError::Unauthorized)
            ));
            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err()
            );
        }
    }
}
