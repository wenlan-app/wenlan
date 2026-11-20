// SPDX-License-Identifier: Apache-2.0
//! Who counts as the person (UX-SKILL-13, UX-SKILL-14): the marker is
//! believed only next to the person key.

use super::*;
use axum::body::Body;
use axum::http::{HeaderValue, Request as HttpRequest, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;
use wenlan_types::requests::PERSON_CALLER_VALUE;

fn marked(key: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CALLER_HEADER, HeaderValue::from_static(PERSON_CALLER_VALUE));
    if let Some(key) = key {
        headers.insert(PERSON_KEY_HEADER, HeaderValue::from_str(key).unwrap());
    }
    headers
}

fn sifted(key: Option<&PersonKey>, mut headers: HeaderMap) -> HeaderMap {
    sift(key, &mut headers);
    headers
}

#[test]
fn ux_skill_13_right_key_counts_as_the_person() {
    let key = PersonKey::new("ab".repeat(32));
    let headers = sifted(Some(&key), marked(Some(&"ab".repeat(32))));
    assert!(crate::memory_routes::caller_is_person(&headers));
    assert!(
        !headers.contains_key(PERSON_KEY_HEADER),
        "the key never reaches a handler"
    );
    // An agent name still makes it an AI tool's, key or not.
    let mut named = marked(Some(&"ab".repeat(32)));
    named.insert("x-agent-name", HeaderValue::from_static("claude-code"));
    assert!(!crate::memory_routes::caller_is_person(&sifted(
        Some(&key),
        named
    )));
}

#[test]
fn ux_skill_13_missing_key_is_an_unnamed_caller() {
    let key = PersonKey::new("ab".repeat(32));
    let headers = sifted(Some(&key), marked(None));
    assert!(!crate::memory_routes::caller_is_person(&headers));
    assert!(
        !headers.contains_key(CALLER_HEADER),
        "the same as a request that never had the marker"
    );
}

#[test]
fn ux_skill_13_wrong_key_is_an_unnamed_caller() {
    let key = PersonKey::new("ab".repeat(32));
    for wrong in [
        "ac".repeat(32),
        "AB".repeat(32),
        "ab".repeat(31),
        String::new(),
        "person".to_string(),
    ] {
        let headers = sifted(Some(&key), marked(Some(&wrong)));
        assert!(
            !crate::memory_routes::caller_is_person(&headers),
            "{wrong:?}"
        );
        assert!(!headers.contains_key(PERSON_KEY_HEADER));
    }
}

#[test]
fn ux_skill_13_no_key_set_believes_nobody() {
    let headers = sifted(None, marked(Some(&"ab".repeat(32))));
    assert!(!crate::memory_routes::caller_is_person(&headers));
}

#[test]
fn ux_skill_13_team_forwarded_call_never_counts_as_the_person() {
    let key = PersonKey::new("ab".repeat(32));
    let mut headers = marked(Some(&"ab".repeat(32)));
    headers.insert(
        crate::team_actor::SECRET_HEADER,
        HeaderValue::from_str(crate::team_actor::secret()).unwrap(),
    );
    headers.insert(
        crate::team_actor::KIND_HEADER,
        HeaderValue::from_static("person"),
    );
    headers.insert(
        crate::team_actor::MEMBER_HEADER,
        HeaderValue::from_static("m-amy"),
    );
    let headers = sifted(Some(&key), headers);
    assert!(!crate::memory_routes::caller_is_person(&headers));
    // A forged team header with no real secret does not help either.
    let mut forged = marked(Some(&"ab".repeat(32)));
    forged.insert(
        crate::team_actor::SECRET_HEADER,
        HeaderValue::from_static("guess"),
    );
    assert!(!crate::memory_routes::caller_is_person(&sifted(
        Some(&key),
        forged
    )));
}

#[test]
fn ux_skill_13_a_browser_pass_works_once() {
    let key = PersonKey::new("ab".repeat(32));
    let pass = key.new_browser_pass().unwrap();
    assert!(wenlan_types::person_key::well_formed(&pass));
    assert_ne!(pass, "ab".repeat(32), "a pass is not the key");
    assert!(key.redeem_browser_pass(&"cd".repeat(32)).is_none());
    let token = key.redeem_browser_pass(&pass).expect("a session token");
    assert_ne!(token, "ab".repeat(32), "the page never gets the key");
    assert!(key.believes(token.as_bytes()));
    assert!(key.redeem_browser_pass(&pass).is_none(), "used up");
    // Asking for many keeps only the newest few.
    let first = key.new_browser_pass().unwrap();
    for _ in 0..MAX_PASSES {
        key.new_browser_pass().unwrap();
    }
    assert!(key.redeem_browser_pass(&first).is_none());
    assert!(
        !format!("{key:?}").contains("ab"),
        "Debug never shows the key"
    );
}

fn state() -> SharedState {
    Arc::new(tokio::sync::RwLock::new(crate::state::ServerState::new()))
}

async fn send(
    path: &str,
    caller: Option<&str>,
    key: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = HttpRequest::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(caller) = caller {
        request = request.header(CALLER_HEADER, caller);
    }
    if let Some(key) = key {
        request = request.header(PERSON_KEY_HEADER, key);
    }
    let response = crate::router::build_router(state())
        .oneshot(request.body(Body::from("{}")).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn refusal(answer: &serde_json::Value) -> &str {
    answer["error"].as_str().unwrap_or_default()
}

/// UX-SKILL-14 / card accept at the daemon's own door: a card is accepted
/// only by a request with the marker and the right key.
#[tokio::test]
async fn ux_skill_14_router_believes_the_marker_only_with_the_right_key() {
    let right = test_key();
    let accept = "/api/memory/revision/no-such-card/accept";
    for (caller, key) in [
        (Some(PERSON_CALLER_VALUE), None),
        (Some(PERSON_CALLER_VALUE), Some("ac".repeat(32))),
        (None, Some(right.to_string())),
    ] {
        let (status, answer) = send(accept, caller, key.as_deref()).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{caller:?} {key:?}"
        );
        assert!(
            refusal(&answer).starts_with("card_needs_the_person"),
            "{answer}"
        );
    }
    // With the right key the person check passes; this test daemon has no
    // store, so the answer is that, not the refusal.
    let (_, answer) = send(accept, Some(PERSON_CALLER_VALUE), Some(right)).await;
    assert!(
        !refusal(&answer).starts_with("card_needs_the_person"),
        "{answer}"
    );
}

/// UX-SKILL-13: the browser pass is the person's, and it is not the key.
#[tokio::test]
async fn ux_skill_13_browser_pass_route_needs_the_person() {
    let right = test_key();
    let path = "/api/person/browser-pass";
    let (status, answer) = send(path, Some(PERSON_CALLER_VALUE), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refusal(&answer).starts_with("browser_pass_needs_the_person"));
    let (status, answer) = send(path, Some(PERSON_CALLER_VALUE), Some(right)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let pass = answer["pass"].as_str().unwrap();
    assert!(wenlan_types::person_key::well_formed(pass));
    assert!(!answer.to_string().contains(right), "never the key itself");
}

async fn send_with(
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let mut request = HttpRequest::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = crate::router::build_router(state())
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// The marker with no key, and with a wrong one, at each door only the
/// person may use.
fn marker_without_the_key() -> Vec<Vec<(&'static str, String)>> {
    vec![
        vec![(CALLER_HEADER, PERSON_CALLER_VALUE.to_string())],
        vec![
            (CALLER_HEADER, PERSON_CALLER_VALUE.to_string()),
            (PERSON_KEY_HEADER, "0f".repeat(32)),
        ],
    ]
}

/// UX-SKILL-13: installing, taking in and removing a skill at the daemon's
/// own door need the key next to the marker.
#[tokio::test]
async fn ux_skill_13_skill_changes_refuse_the_marker_without_the_right_key() {
    test_key();
    let install = serde_json::to_string(&wenlan_types::skill_registry::SkillInstallRequest {
        source: "o/r".into(),
        skill_id: "meeting-notes".into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        repo_path: "skills/meeting-notes".into(),
        scope: wenlan_types::skill_registry::Scope::All,
    })
    .unwrap();
    for (path, body) in [
        ("/api/skills/registry/install", install.as_str()),
        ("/api/skills/registry/import", r#"{"names":["my-review"]}"#),
        ("/api/skills/registry/remove", r#"{"name":"meeting-notes"}"#),
    ] {
        for headers in marker_without_the_key() {
            let headers: Vec<_> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
            let (status, answer) = send_with(path, &headers, body).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{path} {headers:?}"
            );
            assert!(
                refusal(&answer).starts_with("skills_need_the_person"),
                "{path}: {answer}"
            );
        }
    }
    // With the right key the person check passes: this skill is not held.
    let (_, answer) = send_with(
        "/api/skills/registry/remove",
        &[
            (CALLER_HEADER, PERSON_CALLER_VALUE),
            (PERSON_KEY_HEADER, test_key()),
        ],
        r#"{"name":"meeting-notes"}"#,
    )
    .await;
    assert!(
        !refusal(&answer).starts_with("skills_need_the_person"),
        "{answer}"
    );
}

/// UX-SKILL-14: the app's one-press Remove of a saved rule needs the key;
/// without it the request is an unnamed caller's, which must quote the
/// person's words.
#[tokio::test]
async fn ux_skill_14_one_press_rule_remove_refuses_the_marker_without_the_right_key() {
    test_key();
    let path = "/api/skills/rules/0123456789abcdef01234567/remove";
    let press = r#"{"person_asked":""}"#;
    for headers in marker_without_the_key() {
        let headers: Vec<_> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let (status, answer) = send_with(path, &headers, press).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{headers:?}");
        assert!(
            refusal(&answer).contains("person_asked must quote"),
            "{answer}"
        );
    }
    // With the key the press is the person's: it gets past the words (this
    // test daemon has no store, so the answer is that).
    let (_, answer) = send_with(
        path,
        &[
            (CALLER_HEADER, PERSON_CALLER_VALUE),
            (PERSON_KEY_HEADER, test_key()),
        ],
        press,
    )
    .await;
    assert!(!refusal(&answer).contains("person_asked"), "{answer}");
}

/// UX-SKILL-13: a call the team gate forwards is never the person's, even
/// with the marker and the right key, at the daemon's own door.
#[tokio::test]
async fn ux_skill_13_router_never_counts_a_team_forwarded_call_as_the_person() {
    use wenlan_core::{db::MemoryDB, events::NoopEmitter};

    const SPACE: &str = "Project";
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space(SPACE, None, false).await.unwrap();
    let draft = db
        .create_page_draft("My note", "I wrote this.", Some(SPACE), None)
        .await
        .unwrap();
    db.publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap();
    let state = state();
    state.write().await.db = Some(db);
    let app = crate::router::build_router(state);
    let archive = |forwarded| {
        // This page route admits a forwarded call and consults the person marker.
        let mut request = HttpRequest::builder()
            .method("DELETE")
            .uri(format!("/api/pages/{}", draft.id))
            .header(crate::space_header::HEADER_NAME, SPACE)
            .header(CALLER_HEADER, PERSON_CALLER_VALUE)
            .header(PERSON_KEY_HEADER, test_key());
        if forwarded {
            request = request
                .header(
                    crate::team_actor::SECRET_HEADER,
                    crate::team_actor::secret(),
                )
                .header(crate::team_actor::KIND_HEADER, "person")
                .header(crate::team_actor::MEMBER_HEADER, "m-amy");
        }
        app.clone().oneshot(request.body(Body::empty()).unwrap())
    };
    let member = archive(true).await.unwrap();
    assert_eq!(member.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(member.into_body(), 64 * 1024)
        .await
        .unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(answer["code"], "page_is_the_persons", "{answer}");
    assert_eq!(archive(false).await.unwrap().status(), StatusCode::OK);
}

/// UX-SKILL-13: the browser's session token works at the door like the
/// key, and a key holder that starts afresh (a restarted daemon) does not
/// know it.
#[tokio::test]
async fn ux_skill_13_browser_session_token_works_and_dies_with_the_daemon() {
    test_key();
    let daemon = installed().expect("a key is set");
    let pass = daemon.new_browser_pass().unwrap();
    let token = daemon.redeem_browser_pass(&pass).unwrap();
    let accept = "/api/memory/revision/no-such-card/accept";
    let (_, answer) = send_with(
        accept,
        &[
            (CALLER_HEADER, PERSON_CALLER_VALUE),
            (PERSON_KEY_HEADER, &token),
        ],
        "{}",
    )
    .await;
    assert!(
        !refusal(&answer).starts_with("card_needs_the_person"),
        "{answer}"
    );
    let restarted = PersonKey::new(test_key().to_string());
    assert!(!restarted.believes(token.as_bytes()));
}

/// The repair router has no route only the person may use, so the gate's
/// effect cannot be seen there through a request; this pins that both
/// builders put it in front of every route.
#[test]
fn both_routers_put_the_person_gate_in_front_of_every_route() {
    let source = include_str!("router.rs");
    let builders = [
        "pub fn build_router_with_shutdown(",
        "pub fn build_repair_router(",
    ];
    for builder in builders {
        let start = source.find(builder).expect(builder);
        let body = &source[start..];
        let end = body.find("\n}\n").expect("builder end");
        assert!(
            body[..end].contains(".layer(axum::middleware::from_fn(crate::person_key::gate))"),
            "{builder} has no person gate"
        );
    }
}

/// Which AI tools get the rules, and the rule files in the person's home
/// folder, are the person's to see: a program on this computer without the
/// key (or with a wrong one) is refused before anything is read.
#[tokio::test]
async fn rules_delivery_needs_the_person_key() {
    test_key();
    async fn get(headers: &[(&str, &str)]) -> (StatusCode, serde_json::Value) {
        let mut request = HttpRequest::builder()
            .method("GET")
            .uri("/api/skills/delivery");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = crate::router::build_router(state())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }
    let mut strangers: Vec<Vec<(&str, String)>> = vec![vec![]];
    strangers.extend(marker_without_the_key());
    for headers in strangers {
        let headers: Vec<_> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let (status, answer) = get(&headers).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{headers:?}");
        assert!(
            refusal(&answer).starts_with("rules_delivery_needs_the_person"),
            "{headers:?}: {answer}"
        );
    }
    // With the key the person check passes (this test daemon has no store).
    let (_, answer) = get(&[
        (CALLER_HEADER, PERSON_CALLER_VALUE),
        (PERSON_KEY_HEADER, test_key()),
    ])
    .await;
    assert!(
        !refusal(&answer).starts_with("rules_delivery_needs_the_person"),
        "{answer}"
    );
}
