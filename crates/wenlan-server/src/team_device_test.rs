// SPDX-License-Identifier: Apache-2.0
//! Connecting an AI tool with a short code, and the member's own AI tools
//! and desktop link: what each credential may do, and what a short code
//! alone never yields.

use super::*;
use serde_json::{json, Value};
use std::time::Duration;
use wenlan_types::team::{TeamDeviceGrant, TeamDeviceStart, TeamToolKey};

fn quick(f: &mut Fixture) {
    // Polls may follow each other at once.
    f.state.device =
        Arc::new(device::DeviceStore::default().tuned(device::DEVICE_TTL, Duration::ZERO));
}

async fn send(
    state: &TeamState,
    method: Method,
    uri: &str,
    bearer: Option<&str>,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    if let Some(token) = cookie {
        req = req
            .header(header::COOKIE, format!("__Host-wenlan-session={token}"))
            .header(header::ORIGIN, "https://team.example.com");
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(serde_json::to_vec(&body).unwrap())
    };
    let response = team_router(state.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn ask(state: &TeamState, tool: &str) -> TeamDeviceStart {
    let (status, body) = send(
        state,
        Method::POST,
        "/auth/device",
        None,
        None,
        json!({ "tool": tool }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}

async fn poll(state: &TeamState, device_code: &str) -> (StatusCode, Value) {
    send(
        state,
        Method::POST,
        "/auth/device/token",
        None,
        None,
        json!({ "device_code": device_code }),
    )
    .await
}

#[tokio::test]
async fn a_member_key_connects_a_tool_and_the_key_is_handed_over_once() {
    let mut f = fixture();
    with_url(&f);
    quick(&mut f);
    let started = ask(&f.state, "Claude Code").await;
    assert_eq!(
        started.verification_uri,
        "https://team.example.com/app/device"
    );
    assert_eq!(started.user_code.len(), 9);
    assert_eq!((started.expires_in, started.interval), (600, 5));
    assert_eq!(
        poll(&f.state, &started.device_code).await.1["error"],
        "authorization_pending"
    );

    let key = f.writer.1.clone();
    // Typed in any case, with or without the dash.
    let typed = started.user_code.to_lowercase().replace('-', "");
    let (status, seen) = send(
        &f.state,
        Method::POST,
        "/auth/device/lookup",
        Some(&key),
        None,
        json!({ "user_code": typed }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["tool"], "Claude Code");
    assert_eq!(seen["spaces"][0]["space"], "Design");
    let (status, _) = send(
        &f.state,
        Method::POST,
        "/auth/device/approve",
        Some(&key),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = poll(&f.state, &started.device_code).await;
    assert_eq!(status, StatusCode::OK);
    let grant: TeamDeviceGrant = serde_json::from_value(body).unwrap();
    assert!(wenlan_types::team::is_team_key(&grant.key));
    assert_eq!(grant.spaces[0].space, "Design");
    // Once: after that the request is gone.
    assert_eq!(
        poll(&f.state, &started.device_code).await.1["error"],
        "expired_token"
    );

    // The tool key is a tool key: /mcp only, never the member's own calls.
    let found = team::authenticate(&f.state.team_path, &grant.key)
        .unwrap()
        .unwrap();
    assert_eq!(
        found.credential,
        team::Credential::ToolKey {
            id: grant.key_id.clone()
        }
    );
    for (method, uri) in [
        (Method::GET, "/team/v1/me"),
        (Method::GET, "/team/v1/account/keys"),
        (Method::POST, "/auth/device/approve"),
        (Method::POST, "/team/v1/account/desktop-link"),
        (Method::DELETE, "/team/v1/account/accounts/google"),
    ] {
        let (status, _) = send(&f.state, method, uri, Some(&grant.key), None, Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
    // The member's key does not change.
    assert!(team::authenticate(&f.state.team_path, &key)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn the_short_code_alone_never_collects_a_key() {
    let mut f = fixture();
    with_url(&f);
    quick(&mut f);
    let started = ask(&f.state, "Codex").await;
    send(
        &f.state,
        Method::POST,
        "/auth/device/approve",
        Some(&f.writer.1),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    for guess in [
        started.user_code.clone(),
        started.user_code.replace('-', ""),
    ] {
        assert_eq!(poll(&f.state, &guess).await.1["error"], "expired_token");
    }
    // The real code still collects it.
    assert_eq!(poll(&f.state, &started.device_code).await.0, StatusCode::OK);
}

#[tokio::test]
async fn polling_too_fast_is_told_to_slow_down() {
    let f = fixture();
    with_url(&f);
    let started = ask(&f.state, "Codex").await;
    assert_eq!(
        poll(&f.state, &started.device_code).await.1["error"],
        "authorization_pending"
    );
    assert_eq!(
        poll(&f.state, &started.device_code).await.1["error"],
        "slow_down"
    );
}

#[tokio::test]
async fn five_wrong_codes_stop_a_member_from_trying_more() {
    let mut f = fixture();
    with_url(&f);
    quick(&mut f);
    let started = ask(&f.state, "Codex").await;
    for _ in 0..device::MISSES_ALLOWED {
        let (status, body) = send(
            &f.state,
            Method::POST,
            "/auth/device/lookup",
            Some(&f.writer.1),
            None,
            json!({ "user_code": "AAAA-AAAA" }),
        )
        .await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::NOT_FOUND, Some("unknown_code"))
        );
    }
    let (status, body) = send(
        &f.state,
        Method::POST,
        "/auth/device/approve",
        Some(&f.writer.1),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "too_many_tries");
    // Another member is not held back by someone else's guesses.
    let (status, _) = send(
        &f.state,
        Method::POST,
        "/auth/device/lookup",
        Some(&f.reader.1),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_browser_must_have_signed_in_again_just_now_to_allow_it() {
    let mut f = fixture();
    with_url(&f);
    quick(&mut f);
    let started = ask(&f.state, "Claude Code").await;
    let code = json!({ "user_code": started.user_code });
    let stale = f.state.sessions.create(&f.writer.0.id).unwrap();
    // Seeing which tool asks needs no new sign-in; allowing it does.
    let (status, _) = send(
        &f.state,
        Method::POST,
        "/auth/device/lookup",
        None,
        Some(&stale),
        code.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &f.state,
        Method::POST,
        "/auth/device/approve",
        None,
        Some(&stale),
        code.clone(),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("reauth_required"))
    );
    let fresh = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let (status, _) = send(
        &f.state,
        Method::POST,
        "/auth/device/approve",
        None,
        Some(&fresh),
        code,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(poll(&f.state, &started.device_code).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_refused_tool_is_told_no() {
    let mut f = fixture();
    with_url(&f);
    quick(&mut f);
    let started = ask(&f.state, "Cursor").await;
    let (status, _) = send(
        &f.state,
        Method::POST,
        "/auth/device/deny",
        Some(&f.writer.1),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        poll(&f.state, &started.device_code).await.1["error"],
        "access_denied"
    );
    assert!(team::member_by_id(&f.state.team_path, &f.writer.0.id)
        .unwrap()
        .unwrap()
        .tool_keys()
        .is_empty());
}

#[tokio::test]
async fn an_expired_request_cannot_be_allowed_or_collected() {
    let mut f = fixture();
    with_url(&f);
    f.state.device =
        Arc::new(device::DeviceStore::default().tuned(Duration::from_millis(1), Duration::ZERO));
    let started = ask(&f.state, "Codex").await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (status, _) = send(
        &f.state,
        Method::POST,
        "/auth/device/approve",
        Some(&f.writer.1),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        poll(&f.state, &started.device_code).await.1["error"],
        "expired_token"
    );
}

#[tokio::test]
async fn a_member_lists_and_removes_their_own_tools_and_the_key_stops() {
    let f = fixture();
    let (keep_id, _) = team::issue_tool_key(&f.state.team_path, &f.writer.0.id, "Codex").unwrap();
    let (gone_id, gone) =
        team::issue_tool_key(&f.state.team_path, &f.writer.0.id, "Claude Code").unwrap();
    let (other_id, _) = team::issue_tool_key(&f.state.team_path, &f.reader.0.id, "Cursor").unwrap();
    let (status, body) = send(
        &f.state,
        Method::GET,
        "/team/v1/account/keys",
        Some(&f.writer.1),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<TeamToolKey> = serde_json::from_value(body).unwrap();
    assert_eq!(
        listed.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(),
        ["Codex", "Claude Code"]
    );
    // Someone else's tool is not theirs to remove.
    let (status, _) = send(
        &f.state,
        Method::DELETE,
        &format!("/team/v1/account/keys/{other_id}"),
        Some(&f.writer.1),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&f.state, Method::POST, "/mcp", Some(&gone), Vec::new()).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &f.state,
        Method::DELETE,
        &format!("/team/v1/account/keys/{gone_id}"),
        Some(&f.writer.1),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The next request is refused.
    let (status, _) = call(&f.state, Method::POST, "/mcp", Some(&gone), Vec::new()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let member = team::member_by_id(&f.state.team_path, &f.writer.0.id)
        .unwrap()
        .unwrap();
    assert_eq!(member.tool_keys().len(), 1);
    assert_eq!(member.tool_keys()[0].id, keep_id);
    assert_eq!(member.sign_in().unwrap().tools, 1);
}

#[tokio::test]
async fn the_owner_sees_and_removes_a_members_tools() {
    let f = fixture();
    with_url(&f);
    let (_, owner_key) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let (id, key) = team::issue_tool_key(&f.state.team_path, &f.writer.0.id, "Codex").unwrap();
    let uri = format!("/team/v1/admin/members/{}/keys", f.writer.0.id);
    let (status, body) = send(
        &f.state,
        Method::GET,
        &uri,
        Some(&owner_key),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["name"], "Codex");
    // A member cannot look at someone's tools through the owner's calls.
    let (status, _) = send(
        &f.state,
        Method::GET,
        &uri,
        Some(&f.reader.1),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(
        &f.state,
        Method::DELETE,
        &format!("{uri}/{id}"),
        Some(&owner_key),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!([]));
    let (status, _) = call(&f.state, Method::POST, "/mcp", Some(&key), Vec::new()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_desktop_link_needs_a_browser_that_signed_in_again_just_now() {
    let f = fixture();
    with_url(&f);
    let path = "/team/v1/account/desktop-link";
    // A member key already is the desktop's key: a new one would stop it.
    let (status, body) = send(
        &f.state,
        Method::POST,
        path,
        Some(&f.writer.1),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("has_key"))
    );
    let stale = f.state.sessions.create(&f.writer.0.id).unwrap();
    let (status, body) = send(
        &f.state,
        Method::POST,
        path,
        None,
        Some(&stale),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("reauth_required"))
    );
    let fresh = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let (status, body) = send(
        &f.state,
        Method::POST,
        path,
        None,
        Some(&fresh),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let link = body["link"].as_str().unwrap();
    let (url, key) = wenlan_types::team::split_join_code(link).unwrap();
    assert_eq!(url, "https://team.example.com");
    let found = team::authenticate(&f.state.team_path, &key)
        .unwrap()
        .unwrap();
    assert_eq!(found.member.id, f.writer.0.id);
    assert_eq!(found.credential, team::Credential::LegacyKey);
}

#[tokio::test]
async fn a_tool_name_must_be_something_to_show() {
    let f = fixture();
    with_url(&f);
    for tool in ["", "   ", "bad\nname"] {
        let (status, _) = send(
            &f.state,
            Method::POST,
            "/auth/device",
            None,
            None,
            json!({ "tool": tool }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{tool:?}");
    }
    let long = "x".repeat(200);
    let started = ask(&f.state, &long).await;
    let (_, seen) = send(
        &f.state,
        Method::POST,
        "/auth/device/lookup",
        Some(&f.writer.1),
        None,
        json!({ "user_code": started.user_code }),
    )
    .await;
    assert_eq!(seen["tool"].as_str().unwrap().len(), 60);
}

#[tokio::test]
async fn the_invite_page_says_so_when_sign_in_is_off() {
    let f = fixture();
    let page = response_text(join_page_response(&f.state, None).await).await;
    assert!(page.contains("const providers = [];"));
    assert!(page.contains("Sign-in isn't on for this team yet"));
    assert!(page.contains("if (!providers.includes('google')) accept.hidden = true;"));
}

#[test]
fn short_codes_use_only_the_unambiguous_letters() {
    let store = device::DeviceStore::default();
    for _ in 0..50 {
        let (_, code) = store.start("tool").unwrap();
        assert_eq!(code.len(), 8);
        assert!(code
            .bytes()
            .all(|b| wenlan_types::team::DEVICE_CODE_ALPHABET.contains(&b)));
        assert_eq!(
            wenlan_types::team::normalize_user_code(&device::shown_code(&code)),
            Some(code)
        );
    }
    for bad in ["ABCD-EFG", "ABCD-EFG0", "ABCDEFGHI", "IIII-OOOO"] {
        assert_eq!(wenlan_types::team::normalize_user_code(bad), None, "{bad}");
    }
}

#[tokio::test]
async fn a_member_unlinks_google_or_github_but_not_their_only_way_in() {
    let f = fixture();
    with_url(&f);
    let link = |member: &str, issuer: &str, subject: &str| {
        team::link_identity(
            &f.state.team_path,
            member,
            team::Identity {
                issuer: issuer.into(),
                subject: subject.into(),
                email: Some("amy@example.com".into()),
                linked_at: String::new(),
            },
        )
        .unwrap();
    };
    link(&f.writer.0.id, "https://accounts.google.com", "g1");
    link(&f.writer.0.id, "https://github.com", "7");
    let session = f.state.sessions.create(&f.writer.0.id).unwrap();
    let google = "/team/v1/account/accounts/google";

    // From the browser, with only the session: the GitHub account stays.
    let (status, body) = send(
        &f.state,
        Method::DELETE,
        google,
        None,
        Some(&session),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([{"provider": "github", "email": "amy@example.com"}])
    );
    assert!(
        team::member_by_identity(&f.state.team_path, "https://accounts.google.com", "g1")
            .unwrap()
            .is_none()
    );
    let (status, body) = send(
        &f.state,
        Method::DELETE,
        google,
        None,
        Some(&session),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_linked"))
    );
    let okta = "/team/v1/account/accounts/okta";
    let (status, _) = send(
        &f.state,
        Method::DELETE,
        okta,
        None,
        Some(&session),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // With the member key too; the key keeps working after the last account.
    let github = "/team/v1/account/accounts/github";
    let (status, body) = send(
        &f.state,
        Method::DELETE,
        github,
        Some(&f.writer.1),
        None,
        Value::Null,
    )
    .await;
    assert_eq!((status, body), (StatusCode::OK, json!([])));

    // Someone who signs in only with GitHub cannot unlink it.
    let (bo, _) = team::create_invited_member(&f.state.team_path, "Bo", None, &[]).unwrap();
    link(&bo.id, "https://github.com", "8");
    let bo_session = f.state.sessions.create(&bo.id).unwrap();
    let (status, body) = send(
        &f.state,
        Method::DELETE,
        github,
        None,
        Some(&bo_session),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("last_sign_in"))
    );
    assert!(
        team::member_by_identity(&f.state.team_path, "https://github.com", "8")
            .unwrap()
            .is_some()
    );
}
