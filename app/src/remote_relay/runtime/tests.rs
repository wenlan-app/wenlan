// SPDX-License-Identifier: AGPL-3.0-only
use super::*;
use crate::remote_relay::{now_ms, DeviceCredential};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn configured() -> (tempfile::TempDir, Store, Profile) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::in_directory(dir.path().join("relay"));
    let profile = store.configure(None, "review").unwrap();
    let profile = store.enable(profile.revision()).unwrap();
    (dir, store, profile)
}

fn device() -> DeviceCredential {
    DeviceCredential {
        id: "d".repeat(64),
        management_token: "e".repeat(64),
        expires_at: now_ms() + 60_000,
    }
}

async fn serve(
    replies: Vec<(u16, String)>,
    before_reply: impl Fn(usize) + Send + Sync + 'static,
) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for (index, (status, body)) in replies.into_iter().enumerate() {
            let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![];
                loop {
                    let mut buffer = [0; 2048];
                    let len = socket.read(&mut buffer).await.unwrap();
                    assert!(len > 0);
                    bytes.extend_from_slice(&buffer[..len]);
                    assert!(bytes.len() <= 16 * 1024);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = headers.lines().find_map(|line| line.strip_prefix("content-length: ")).map(|value| value.parse().unwrap()).unwrap_or(0);
                        if bytes.len() >= end + 4 + length { break; }
                    }
                }
                before_reply(index);
                if status == 0 {
                    // Simulate a committed server operation whose reply is lost.
                    return String::from_utf8(bytes).unwrap();
                }
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                String::from_utf8(bytes).unwrap()
            }).await.unwrap();
            requests.push(request);
        }
        requests
    });
    (port, task)
}

fn client(port: u16) -> RelayClient {
    RelayClient::build(
        reqwest::Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
        false,
    )
    .unwrap()
}

#[tokio::test]
async fn interrupted_disconnect_survives_reload_until_server_confirms_retry() {
    let (_dir, store, profile) = configured();
    let profile = store.attach_device(profile.revision(), device()).unwrap();
    let off = store.disable(profile.revision()).unwrap();
    let (port, task) = serve(
        vec![(0, String::new()), (200, "{\"success\":true}".into())],
        |_| {},
    )
    .await;
    let client = client(port);
    assert!(crate::remote_relay::disconnect::finish(
        store.clone(),
        client.clone(),
        crate::remote_relay::disconnect::prepare(&store, Some(off.revision()))
    )
    .await
    .is_err());
    let reloaded = store.load().unwrap().unwrap();
    assert!(!reloaded.enabled());
    assert!(reloaded.view().disconnect_pending);
    assert_eq!(reloaded.revision(), off.revision());
    crate::remote_relay::disconnect::finish(
        store.clone(),
        client,
        crate::remote_relay::disconnect::prepare(&store, Some(reloaded.revision())),
    )
    .await
    .unwrap();
    let saved = store.load().unwrap().unwrap();
    assert!(!saved.enabled());
    assert!(!saved.view().disconnect_pending);
    assert_ne!(saved.backend_token(), off.backend_token());
    assert_ne!(saved.revision(), off.revision());
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn renewal_preserves_profile_and_rejects_a_late_response_after_stop() {
    for stop in [false, true] {
        let (_dir, store, profile) = configured();
        let profile = store.attach_device(profile.revision(), device()).unwrap();
        let stop_store = store.clone();
        let revision = profile.revision().to_string();
        let (port, task) = serve(vec![(200, "{\"success\":true}".into())], move |_| {
            if stop {
                stop_store.disable(&revision).unwrap();
            }
        })
        .await;
        let result = renew_at(
            store.clone(),
            client(port),
            profile.clone(),
            "https://synthetic.trycloudflare.com".into(),
        )
        .await;
        assert_eq!(result.is_ok(), !stop);
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.enabled(), !stop);
        assert_eq!(saved.backend_token(), profile.backend_token());
        assert_eq!(
            saved.device().unwrap().management_token,
            profile.device().unwrap().management_token
        );
        if !stop {
            assert_eq!(saved.revision(), profile.revision());
        }
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /devices/refresh "));
    }
}

#[tokio::test]
async fn renewal_cannot_enroll_a_missing_device_or_reenable_a_disabled_profile() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(listener.local_addr().unwrap().port());
    let (_dir, store, profile) = configured();
    assert!(renew_at(
        store.clone(),
        client.clone(),
        profile.clone(),
        "https://synthetic.trycloudflare.com".into()
    )
    .await
    .is_err());
    let saved = store.attach_device(profile.revision(), device()).unwrap();
    store.disable(saved.revision()).unwrap();
    assert!(renew_at(
        store,
        client,
        saved,
        "https://synthetic.trycloudflare.com".into()
    )
    .await
    .is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn renewal_failure_preserves_typed_auth_or_outage_error_without_fallback() {
    for status in [401, 429, 503] {
        let (_dir, store, profile) = configured();
        let profile = store.attach_device(profile.revision(), device()).unwrap();
        let (port, task) = serve(vec![(status, "PRIVATE_ERROR".into())], |_| {}).await;
        let error = renew_at(
            store.clone(),
            client(port),
            profile.clone(),
            "https://synthetic.trycloudflare.com".into(),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("PRIVATE_ERROR"));
        match (status, error) {
            (401, RenewalError::Relay(RelayError::Unauthorized))
            | (429, RenewalError::Relay(RelayError::RateLimited { .. }))
            | (503, RenewalError::Relay(RelayError::Unavailable)) => {}
            (_, error) => panic!("Unexpected renewal error: {error}"),
        }
        assert_eq!(
            store.load().unwrap().unwrap().revision(),
            profile.revision()
        );
        assert_eq!(task.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn disconnect_never_clears_credentials_after_unauthorized_or_unconfirmed_reply() {
    for (status, body) in [(401, "{}"), (503, "{}"), (200, "{\"success\":false}")] {
        let (_dir, store, profile) = configured();
        let profile = store.attach_device(profile.revision(), device()).unwrap();
        let off = store.disable(profile.revision()).unwrap();
        let (port, task) = serve(vec![(status, body.into())], |_| {}).await;
        assert!(crate::remote_relay::disconnect::finish(
            store.clone(),
            client(port),
            crate::remote_relay::disconnect::prepare(&store, Some(off.revision()))
        )
        .await
        .is_err());
        let saved = store.load().unwrap().unwrap();
        assert!(saved.view().disconnect_pending);
        assert_eq!(saved.revision(), off.revision());
        task.await.unwrap();
    }
}

fn info(space: &str) -> String {
    serde_json::json!({"contract_version":1,"server":"wenlan-mcp","tool_profile":"query-only","authentication":"bearer","space":space}).to_string()
}

fn inspected() -> crate::remote_relay::PairingView {
    crate::remote_relay::PairingView {
        pairing_id: "a".repeat(64),
        client_id: "synthetic-client".into(),
        resource: format!("{}/mcp", crate::remote_relay::RELAY_ORIGIN),
        scopes: vec!["wenlan:query".into()],
        expires_at: now_ms() + 60_000,
        client_name: None,
        redirect_host: None,
        known_client: false,
    }
}

#[tokio::test]
async fn approval_rechecks_current_server_intent_before_sending_explicit_consent() {
    let (_dir, store, profile) = configured();
    let profile = store.attach_device(profile.revision(), device()).unwrap();
    let view = inspected();
    let (port, task) = serve(
        vec![
            (200, serde_json::to_string(&view).unwrap()),
            (200, "{\"approved\":true}".into()),
        ],
        |_| {},
    )
    .await;
    approve_inspected_at(
        store.clone(),
        client(port),
        profile.revision().into(),
        view.clone(),
        GrantAccess::Query,
    )
    .await
    .unwrap();
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("GET /pairings/"));
    assert!(requests[1].starts_with("POST /pairings/"));
    let body: serde_json::Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["space"], "review");
    assert_eq!(body["clientId"], view.client_id);
    assert_eq!(body["approved"], true);
    assert_eq!(body["access"], "query");
    // Search only never switches the write server on.
    assert!(!store.load().unwrap().unwrap().write_enabled());
}

#[tokio::test]
async fn approving_write_sends_the_choice_and_records_it_without_changing_the_revision() {
    let (_dir, store, profile) = configured();
    let profile = store.attach_device(profile.revision(), device()).unwrap();
    assert!(!profile.write_enabled());
    let view = inspected();
    let (port, task) = serve(
        vec![
            (200, serde_json::to_string(&view).unwrap()),
            (200, "{\"approved\":true}".into()),
        ],
        |_| {},
    )
    .await;
    approve_inspected_at(
        store.clone(),
        client(port),
        profile.revision().into(),
        view,
        GrantAccess::Write,
    )
    .await
    .unwrap();
    let requests = task.await.unwrap();
    let body: serde_json::Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["access"], "write");
    let saved = store.load().unwrap().unwrap();
    assert!(saved.write_enabled());
    // A running connection captured at this revision stays current.
    assert_eq!(saved.revision(), profile.revision());
}

#[tokio::test]
async fn a_refused_write_approval_leaves_write_off() {
    let (_dir, store, profile) = configured();
    let profile = store.attach_device(profile.revision(), device()).unwrap();
    let view = inspected();
    let (port, task) = serve(
        vec![
            (200, serde_json::to_string(&view).unwrap()),
            (409, "{}".into()),
        ],
        |_| {},
    )
    .await;
    approve_inspected_at(
        store.clone(),
        client(port),
        profile.revision().into(),
        view,
        GrantAccess::Write,
    )
    .await
    .unwrap_err();
    task.await.unwrap();
    assert!(!store.load().unwrap().unwrap().write_enabled());
}

#[tokio::test]
async fn approval_rejects_changed_client_or_local_stop_without_posting_consent() {
    for stop in [false, true] {
        let (_dir, store, profile) = configured();
        let profile = store.attach_device(profile.revision(), device()).unwrap();
        let view = inspected();
        let mut server_view = view.clone();
        if !stop {
            server_view.client_id = "different-client".into();
        }
        let stop_store = store.clone();
        let revision = profile.revision().to_string();
        let (port, task) = serve(
            vec![(200, serde_json::to_string(&server_view).unwrap())],
            move |_| {
                if stop {
                    stop_store.disable(&revision).unwrap();
                }
            },
        )
        .await;
        let result = approve_inspected_at(
            store,
            client(port),
            profile.revision().into(),
            view,
            GrantAccess::Query,
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            if stop {
                StoreError::NotConfigured
            } else {
                StoreError::Stale
            }
            .to_string()
        );
        assert_eq!(task.await.unwrap().len(), 1);
    }
}

#[test]
fn launch_arguments_require_protected_query_only_and_no_secret_on_command_line() {
    let (_dir, _store, profile) = configured();
    let args = mcp_args("http://127.0.0.1:17917", 18080, &profile);
    for pair in [
        ["--host", "127.0.0.1"],
        ["--tool-profile", "query-only"],
        ["--token-env", TOKEN_ENV],
    ] {
        assert!(args
            .windows(2)
            .any(|args| args[0] == pair[0] && args[1] == pair[1]));
    }
    let line = args.join(" ");
    for forbidden in [
        "--no-auth",
        "origin-relay",
        profile.backend_token(),
        profile.space(),
    ] {
        assert!(!line.contains(forbidden));
    }
}

fn whole_library() -> (tempfile::TempDir, Store, Profile) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::in_directory(dir.path().join("relay"));
    let profile = store
        .configure(None, crate::remote_relay::WHOLE_LIBRARY_SPACE)
        .unwrap();
    let profile = store.enable(profile.revision()).unwrap();
    (dir, store, profile)
}

#[test]
fn write_launch_matches_the_query_launch_except_profile_and_agent() {
    let (_dir, _store, pinned) = configured();
    let query = mcp_args("http://127.0.0.1:17917", 18080, &pinned);
    let write = write_mcp_args("http://127.0.0.1:17917", 18081, &pinned);
    for pair in [
        ["--host", "127.0.0.1"],
        ["--port", "18081"],
        ["--tool-profile", "web-write"],
        ["--token-env", TOKEN_ENV],
        ["--agent-name", "web-ai"],
        ["--origin-url", "http://127.0.0.1:17917"],
    ] {
        assert!(
            write
                .windows(2)
                .any(|args| args[0] == pair[0] && args[1] == pair[1]),
            "{pair:?}"
        );
    }
    assert!(write.iter().any(|arg| arg == "serve"));
    assert!(!write.contains(&"--whole-library".to_string()));
    let line = write.join(" ");
    for forbidden in [
        "--no-auth",
        "query-only",
        pinned.backend_token(),
        pinned.space(),
    ] {
        assert!(!line.contains(forbidden), "{forbidden}");
    }
    // The search-only launch is unchanged beside it.
    assert!(query
        .windows(2)
        .any(|pair| pair[0] == "--tool-profile" && pair[1] == "query-only"));
    assert!(query
        .windows(2)
        .any(|pair| pair[0] == "--agent-name" && pair[1] == "remote-mcp"));

    let (_dir, _store, whole) = whole_library();
    let write = write_mcp_args("http://127.0.0.1:17917", 18081, &whole);
    assert_eq!(write.last().map(String::as_str), Some("--whole-library"));
    assert!(write
        .windows(2)
        .any(|pair| pair[0] == "--tool-profile" && pair[1] == "web-write"));
    assert!(!write.iter().any(|arg| arg == "*"));
}

#[tokio::test]
async fn write_readiness_requires_the_write_profile_and_query_readiness_refuses_it() {
    let (_dir, _store, profile) = configured();
    let write_info = |tool_profile: &str| {
        serde_json::json!({"contract_version":1,"server":"wenlan-mcp","tool_profile":tool_profile,"authentication":"bearer","space":"review"}).to_string()
    };
    let (port, task) = serve(
        vec![(200, write_info("web-write")), (401, "denied".into())],
        |_| {},
    )
    .await;
    verify_backend_as(McpKind::Write, port, &profile)
        .await
        .unwrap();
    task.await.unwrap();
    // A search-only server on the write port, or the reverse, is not accepted.
    let (port, task) = serve(vec![(200, write_info("query-only"))], |_| {}).await;
    assert!(verify_backend_as(McpKind::Write, port, &profile)
        .await
        .is_err());
    task.await.unwrap();
    let (port, task) = serve(vec![(200, write_info("web-write"))], |_| {}).await;
    assert!(verify_backend(port, &profile).await.is_err());
    task.await.unwrap();
}

#[test]
fn launch_scope_pins_one_space_or_asks_for_the_whole_library() {
    let (_dir, _store, pinned) = configured();
    assert!(!mcp_args("http://127.0.0.1:17917", 18080, &pinned)
        .contains(&"--whole-library".to_string()));
    assert_eq!(
        mcp_scope_env(&pinned),
        [
            ("WENLAN_SPACE", "review".to_string()),
            ("WENLAN_DEFAULT_SPACE", String::new()),
        ]
    );

    let (_dir, _store, whole) = whole_library();
    let args = mcp_args("http://127.0.0.1:17917", 18080, &whole);
    assert_eq!(args.last().map(String::as_str), Some("--whole-library"));
    assert!(args
        .windows(2)
        .any(|pair| pair[0] == "--tool-profile" && pair[1] == "query-only"));
    assert!(!args.iter().any(|arg| arg == "*"));
    // Both Space variables are cleared, so nothing inherited narrows the scope.
    assert_eq!(
        mcp_scope_env(&whole),
        [
            ("WENLAN_SPACE", String::new()),
            ("WENLAN_DEFAULT_SPACE", String::new()),
        ]
    );
}

#[tokio::test]
async fn readiness_matches_the_whole_library_scope_exactly() {
    let (_dir, _store, profile) = whole_library();
    let (port, task) = serve(vec![(200, info("*")), (401, "denied".into())], |_| {}).await;
    verify_backend(port, &profile).await.unwrap();
    task.await.unwrap();
    let (port, task) = serve(vec![(200, info("review"))], |_| {}).await;
    assert!(verify_backend(port, &profile).await.is_err());
    task.await.unwrap();
    // A one-Space profile never accepts a whole-library sidecar.
    let (_dir, _store, pinned) = configured();
    let (port, task) = serve(vec![(200, info("*"))], |_| {}).await;
    assert!(verify_backend(port, &pinned).await.is_err());
    task.await.unwrap();
}

#[test]
fn missing_or_disabled_profile_is_not_implicit_consent() {
    assert!(require_enabled(None).is_err());
    let (_dir, store, profile) = configured();
    let disabled = store.disable(profile.revision()).unwrap();
    assert!(require_enabled(Some(disabled)).is_err());
    assert!(require_enabled(Some(profile)).is_ok());
}

#[tokio::test]
async fn readiness_requires_matching_protected_contract_and_anonymous_denial() {
    let (_dir, _store, profile) = configured();
    let (port, task) = serve(vec![(200, info("review")), (401, "denied".into())], |_| {}).await;
    verify_backend(port, &profile).await.unwrap();
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("GET /connector-info "));
    assert!(requests[0].contains(profile.backend_token()));
    assert!(!requests[1].contains(profile.backend_token()));
    let (port, task) = serve(vec![(200, info("review")), (200, info("review"))], |_| {}).await;
    assert_eq!(
        verify_backend(port, &profile).await,
        Err(RelayError::InvalidResponse)
    );
    task.await.unwrap();
}

#[tokio::test]
async fn readiness_rejects_wrong_scope_legacy_health_redirect_and_large_response() {
    let (_dir, _store, profile) = configured();
    for (status, body) in [
        (200, info("other")),
        (200, "{\"status\":\"ok\"}".into()),
        (302, "".into()),
        (200, "x".repeat(4097)),
    ] {
        let (port, task) = serve(vec![(status, body)], |_| {}).await;
        assert!(verify_backend(port, &profile).await.is_err());
        task.await.unwrap();
    }
}

#[tokio::test]
async fn enrollment_persists_separate_management_credential_and_refresh_reuses_it() {
    let (_dir, store, profile) = configured();
    let (port, task) = serve(
        vec![(201, serde_json::to_string(&device()).unwrap())],
        |_| {},
    )
    .await;
    register_at(
        store.clone(),
        client(port),
        profile.clone(),
        "https://synthetic.trycloudflare.com".into(),
    )
    .await
    .unwrap();
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("POST /devices "));
    assert!(requests[0].contains(profile.backend_token()));
    assert!(!requests[0].contains(&device().management_token));
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.device().unwrap().id, device().id);
    let (port, task) = serve(vec![(200, "{\"success\":true}".into())], |_| {}).await;
    register_at(
        store.clone(),
        client(port),
        saved.clone(),
        "https://refreshed.trycloudflare.com".into(),
    )
    .await
    .unwrap();
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("POST /devices/refresh "));
    assert!(requests[0].contains(&device().management_token));
    assert_eq!(store.load().unwrap().unwrap().revision(), saved.revision());
}

#[tokio::test]
async fn cancelled_enrollment_cannot_revive_profile_and_revokes_its_orphan() {
    let (_dir, store, profile) = configured();
    let stop_store = store.clone();
    let revision = profile.revision().to_string();
    let (port, task) = serve(
        vec![
            (201, serde_json::to_string(&device()).unwrap()),
            (200, "{\"success\":true}".into()),
        ],
        move |index| {
            if index == 0 {
                stop_store.disable(&revision).unwrap();
            }
        },
    )
    .await;
    assert!(register_at(
        store.clone(),
        client(port),
        profile,
        "https://synthetic.trycloudflare.com".into()
    )
    .await
    .is_err());
    let requests = task.await.unwrap();
    assert!(requests[1].starts_with("POST /devices/revoke "));
    let saved = store.load().unwrap().unwrap();
    assert!(!saved.enabled());
    assert!(saved.device().is_none());
}

#[tokio::test]
async fn failed_enrollment_has_no_fallback_or_persisted_device() {
    let (_dir, store, profile) = configured();
    let (port, task) = serve(vec![(503, "private server details".into())], |_| {}).await;
    let error = register_at(
        store.clone(),
        client(port),
        profile.clone(),
        "https://synthetic.trycloudflare.com".into(),
    )
    .await
    .unwrap_err();
    assert!(!error.contains("private server details"));
    assert!(!error.contains("trycloudflare"));
    assert_eq!(task.await.unwrap().len(), 1);
    assert_eq!(
        store.load().unwrap().unwrap().revision(),
        profile.revision()
    );
    assert!(store.load().unwrap().unwrap().device().is_none());
}

#[tokio::test]
async fn disabled_snapshot_does_not_contact_relay() {
    let (_dir, store, profile) = configured();
    store.disable(profile.revision()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(listener.local_addr().unwrap().port());
    assert!(register_at(
        store,
        client,
        profile,
        "https://synthetic.trycloudflare.com".into()
    )
    .await
    .is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires an explicitly rebuilt WENLAN_TEST_MCP_BIN, no installed runtime"]
async fn actual_sidecar_uses_child_environment_and_protected_contract() {
    use std::process::{Command, Stdio};
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let binary = std::env::var_os("WENLAN_TEST_MCP_BIN").expect("rebuilt binary path required");
    let binary = std::path::PathBuf::from(binary);
    assert!(binary.is_absolute() && binary.is_file());
    let (_dir, _store, profile) = configured();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut command = Command::new(&binary);
    command
        .args(mcp_args("http://127.0.0.1:1", port, &profile))
        .env(TOKEN_ENV, profile.backend_token())
        .envs(mcp_scope_env(&profile))
        .env("WENLAN_NO_AUTOSTART", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = OwnedChild(command.spawn().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "sidecar exited before readiness"
            );
            if verify_backend(port, &profile).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("protected sidecar readiness deadline");
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"native-contract","version":"1"}}});
    assert_eq!(
        http.post(&url)
            .header("accept", "application/json, text/event-stream")
            .json(&init)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let initialized = http
        .post(&url)
        .bearer_auth(profile.backend_token())
        .header("accept", "application/json, text/event-stream")
        .json(&init)
        .send()
        .await
        .unwrap();
    assert!(initialized.status().is_success());
    let session = initialized
        .headers()
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let _ = initialized.bytes().await.unwrap();
    let notification = http
        .post(&url)
        .bearer_auth(profile.backend_token())
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .json(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert!(notification.status().is_success());
    let listing = http
        .post(&url)
        .bearer_auth(profile.backend_token())
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .json(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert!(listing.status().is_success());
    let body = listing.text().await.unwrap();
    let json = if body.trim_start().starts_with('{') {
        body.as_str()
    } else {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .find(|data| !data.trim().is_empty())
            .expect("JSON or SSE data")
    };
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(value["id"], 2);
    let mut names: Vec<_> = value["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["brief", "get_page_sources", "recall"]);
    drop(child);

    // A missing explicit child variable must exit, even if the machine has a
    // default token file. This also detects accidental no-auth fallback.
    let mut missing = OwnedChild(command.env_remove(TOKEN_ENV).spawn().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(status) = missing.0.try_wait().unwrap() {
                assert!(!status.success());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("missing token must stop startup");
}

fn rotated(old: &DeviceCredential) -> String {
    serde_json::json!({
        "id": old.id,
        "managementToken": "n".repeat(64),
        "expiresAt": now_ms() + 90 * 24 * 60 * 60 * 1000,
    })
    .to_string()
}

#[tokio::test]
async fn renewing_keeps_the_device_and_saves_the_new_key_under_a_new_revision() {
    let (_dir, store, profile) = configured();
    let old = device();
    let profile = store
        .attach_device(profile.revision(), old.clone())
        .unwrap();
    let (port, task) = serve(vec![(200, rotated(&old))], |_| {}).await;
    let renewed = rotate_device_at(store.clone(), client(port), profile.revision().into())
        .await
        .unwrap();
    let key = renewed.device().unwrap();
    assert_eq!(key.id, old.id);
    assert_eq!(key.management_token, "n".repeat(64));
    assert!(key.expires_at > old.expires_at + 80 * 24 * 60 * 60 * 1000);
    assert_ne!(renewed.revision(), profile.revision());
    assert!(renewed.enabled());
    assert_eq!(renewed.space(), profile.space());
    assert_eq!(renewed.backend_token(), profile.backend_token());
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.revision(), renewed.revision());
    assert_eq!(saved.device().unwrap().management_token, "n".repeat(64));
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("POST /devices/rotate "));
    assert!(requests[0].contains(&format!("authorization: Bearer {}", old.management_token)));
}

#[tokio::test]
async fn renewing_a_stale_or_unenrolled_profile_never_reaches_the_relay() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay = client(listener.local_addr().unwrap().port());
    let (_dir, store, profile) = configured();
    // No device yet.
    assert!(
        rotate_device_at(store.clone(), relay.clone(), profile.revision().into())
            .await
            .is_err()
    );
    let enrolled = store.attach_device(profile.revision(), device()).unwrap();
    // A revision the person is no longer looking at.
    assert!(
        rotate_device_at(store.clone(), relay.clone(), "x".repeat(64))
            .await
            .is_err()
    );
    // Turned off.
    let off = store.disable(enrolled.revision()).unwrap();
    assert!(rotate_device_at(store, relay, off.revision().into())
        .await
        .is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_renew_that_loses_a_race_with_stop_ends_the_new_key_and_keeps_nothing_new() {
    let (_dir, store, profile) = configured();
    let old = device();
    let profile = store
        .attach_device(profile.revision(), old.clone())
        .unwrap();
    let stop_store = store.clone();
    let revision = profile.revision().to_string();
    let (port, task) = serve(
        vec![(200, rotated(&old)), (200, "{\"success\":true}".into())],
        move |index| {
            if index == 0 {
                stop_store.disable(&revision).unwrap();
            }
        },
    )
    .await;
    assert!(
        rotate_device_at(store.clone(), client(port), profile.revision().into())
            .await
            .is_err()
    );
    let requests = task.await.unwrap();
    assert!(requests[1].starts_with("POST /devices/revoke "));
    assert!(requests[1].contains(&format!("authorization: Bearer {}", "n".repeat(64))));
    let saved = store.load().unwrap().unwrap();
    assert!(!saved.enabled());
    assert_eq!(
        saved.device().unwrap().management_token,
        old.management_token
    );
}

#[tokio::test]
async fn a_failed_renew_keeps_the_saved_key_and_shows_no_relay_text() {
    for status in [401, 429, 503] {
        let (_dir, store, profile) = configured();
        let profile = store.attach_device(profile.revision(), device()).unwrap();
        let (port, task) = serve(vec![(status, "PRIVATE_ERROR".into())], |_| {}).await;
        let error = rotate_device_at(store.clone(), client(port), profile.revision().into())
            .await
            .unwrap_err();
        assert!(!error.contains("PRIVATE_ERROR"));
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.revision(), profile.revision());
        assert_eq!(
            saved.device().unwrap().management_token,
            profile.device().unwrap().management_token
        );
        assert_eq!(task.await.unwrap().len(), 1);
    }
}
