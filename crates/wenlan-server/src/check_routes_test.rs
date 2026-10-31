// SPDX-License-Identifier: Apache-2.0
//! Route contracts for `/api/check`, `/api/check/context` and
//! `/api/pages/instructions`.

use crate::state::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::chat_check::{
    ChatCheckResponse, ChatCheckSkipped, ChatContextResponse, InstructionsPageResponse,
};

async fn state_with_db() -> (Arc<RwLock<ServerState>>, Arc<MemoryDB>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    let state = ServerState {
        db: Some(db.clone()),
        ..Default::default()
    };
    (Arc::new(RwLock::new(state)), db, tmp)
}

/// The in-chat check is opt-in. Holds the shared data-dir lock and points
/// `WENLAN_DATA_DIR` at a fresh folder whose config has the check set to
/// `enabled`, until dropped.
struct CheckSetting {
    previous: Option<std::ffi::OsString>,
    _tmp: tempfile::TempDir,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

async fn check_setting(enabled: bool) -> CheckSetting {
    let lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("WENLAN_DATA_DIR");
    std::env::set_var("WENLAN_DATA_DIR", tmp.path());
    let config = wenlan_core::config::Config {
        chat_check_enabled: enabled,
        ..Default::default()
    };
    wenlan_core::config::save_config(&config).unwrap();
    CheckSetting {
        previous,
        _tmp: tmp,
        _lock: lock,
    }
}

impl Drop for CheckSetting {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

async fn store(db: &MemoryDB, source_id: &str, memory_type: &str, content: &str) {
    db.upsert_documents(vec![wenlan_types::sources::RawDocument {
        source: "memory".to_string(),
        source_id: source_id.to_string(),
        title: content.chars().take(40).collect(),
        content: content.to_string(),
        last_modified: 1_760_000_000,
        memory_type: Some(memory_type.to_string()),
        confirmed: Some(true),
        enrichment_status: "enriched".to_string(),
        supersede_mode: "archive".to_string(),
        ..Default::default()
    }])
    .await
    .unwrap();
}

async fn call(
    state: &Arc<RwLock<ServerState>>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let app = crate::router::build_router(state.clone());
    let mut request = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            request = request.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn check_and_context_stay_off_until_the_person_turns_them_on() {
    let _off = check_setting(false).await;
    let (state, db, _tmp) = state_with_db().await;
    store(
        &db,
        "mem_storage",
        "decision",
        "Use libSQL instead of a dedicated graph database.",
    )
    .await;
    let (status, json) = call(
        &state,
        "POST",
        "/api/check",
        Some(serde_json::json!({"reply": "Let's switch to a dedicated graph database."})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let response: ChatCheckResponse = serde_json::from_value(json.clone()).unwrap();
    assert!(response.reminders.is_empty(), "{json}");
    assert_eq!(response.skipped, Some(ChatCheckSkipped::Off));
    assert!(db.list_chat_reminder_entries().await.unwrap().is_empty());

    let (status, json) = call(
        &state,
        "POST",
        "/api/check/context",
        Some(serde_json::json!({"prompt": "Should we use a graph database?"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let context: ChatContextResponse = serde_json::from_value(json.clone()).unwrap();
    assert!(context.decisions.is_empty(), "{json}");
    assert_eq!(context.skipped, Some(ChatCheckSkipped::Off));
}

#[tokio::test]
async fn check_returns_a_reminder_with_date_and_source_and_logs_it() {
    let _on = check_setting(true).await;
    let (state, db, _tmp) = state_with_db().await;
    store(
        &db,
        "mem_storage",
        "decision",
        "Use libSQL instead of a dedicated graph database.",
    )
    .await;
    let (status, json) = call(
        &state,
        "POST",
        "/api/check",
        Some(serde_json::json!({
            "reply": "Let's switch to a dedicated graph database.",
            "session": "claude-code:abc",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let response: ChatCheckResponse = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(response.reminders.len(), 1, "{json}");
    let reminder = &response.reminders[0];
    assert_eq!(reminder.source.memory_id, "mem_storage");
    assert!(reminder.source.link.starts_with("wenlan://"));
    assert!(!reminder.decided_on.is_empty());
    assert!(reminder.text.contains(&reminder.decided_on));
    assert_eq!(response.remaining_today, 4);
    let log = db.list_chat_reminder_entries().await.unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["op"], "chat.reminder");
    assert_eq!(log[0]["actor"]["session"], "claude-code:abc");
}

#[tokio::test]
async fn check_is_silent_when_nothing_conflicts() {
    let _on = check_setting(true).await;
    let (state, db, _tmp) = state_with_db().await;
    store(
        &db,
        "mem_storage",
        "decision",
        "Use libSQL instead of a dedicated graph database.",
    )
    .await;
    let (status, json) = call(
        &state,
        "POST",
        "/api/check",
        Some(serde_json::json!({"reply": "I renamed the helper and ran the tests."})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({"reminders": [], "remaining_today": 5})
    );
    assert!(db.list_chat_reminder_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn check_rejects_an_unknown_space() {
    let _on = check_setting(true).await;
    let (state, _db, _tmp) = state_with_db().await;
    let (status, _) = call(
        &state,
        "POST",
        "/api/check",
        Some(serde_json::json!({"reply": "x y z", "space": "no-such-space"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn check_context_hands_back_relevant_decisions() {
    let _on = check_setting(true).await;
    let (state, db, _tmp) = state_with_db().await;
    store(
        &db,
        "mem_storage",
        "decision",
        "Use libSQL instead of a dedicated graph database for entity storage.",
    )
    .await;
    let (status, json) = call(
        &state,
        "POST",
        "/api/check/context",
        Some(serde_json::json!({"prompt": "Should entity storage move to a graph database?"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let response: ChatContextResponse = serde_json::from_value(json).unwrap();
    assert_eq!(response.decisions.len(), 1);
    assert_eq!(response.decisions[0].source.memory_id, "mem_storage");
    assert!(response
        .context
        .starts_with("Past decisions in Wenlan that bear on this"));
    assert!(response
        .context
        .contains(&response.decisions[0].source.link));

    let (_, empty) = call(
        &state,
        "POST",
        "/api/check/context",
        Some(serde_json::json!({"prompt": "   "})),
    )
    .await;
    let empty: ChatContextResponse = serde_json::from_value(empty).unwrap();
    assert_eq!(empty.skipped, Some(ChatCheckSkipped::Empty));
}

#[tokio::test]
async fn instructions_page_is_seeded_once_and_found_by_role() {
    let (state, db, _tmp) = state_with_db().await;
    let (status, json) = call(&state, "GET", "/api/pages/instructions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({}));

    store(
        &db,
        "mem_pref",
        "preference",
        "I prefer short answers with the conclusion first.",
    )
    .await;
    let (status, json) = call(&state, "GET", "/api/pages/instructions?locale=en", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let first: InstructionsPageResponse = serde_json::from_value(json).unwrap();
    assert!(first.seeded);
    let page = first.page.expect("page");
    assert_eq!(page.title, "How you want AI to work with you");
    assert_eq!(page.role, "instructions");
    assert!(page.content.contains("I prefer short answers"));
    assert_eq!(page.link, format!("wenlan://page/{}", page.page_id));

    let (_, json) = call(&state, "GET", "/api/pages/instructions", None).await;
    let second: InstructionsPageResponse = serde_json::from_value(json).unwrap();
    assert!(!second.seeded);
    assert_eq!(second.page.unwrap().page_id, page.page_id);

    // The page read carries its role and its body layers.
    let (status, json) = call(&state, "GET", &format!("/api/pages/{}", page.page_id), None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["role"], "instructions");
    assert!(json["body"]["short"]
        .as_str()
        .unwrap()
        .starts_with("This page says how you want AI tools"));
    assert!(json["body"].get("details").is_none());
}

#[tokio::test]
async fn an_ai_change_to_the_instructions_page_is_only_a_proposal() {
    let (state, db, _tmp) = state_with_db().await;
    store(
        &db,
        "mem_pref",
        "preference",
        "I prefer short answers with the conclusion first.",
    )
    .await;
    let (_, json) = call(&state, "GET", "/api/pages/instructions?locale=en", None).await;
    let page: InstructionsPageResponse = serde_json::from_value(json).unwrap();
    let page = page.page.expect("page");

    let (status, json) = call(
        &state,
        "PUT",
        &format!("/api/pages/{}", page.page_id),
        Some(serde_json::json!({
            "content": "Always answer at length.",
            "source_memory_ids": [],
            "expected_version": page.version,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["gated"], true, "{json}");
    assert!(json["revision_card_id"].is_string(), "{json}");
    let stored = db.get_page(&page.page_id).await.unwrap().unwrap();
    assert_eq!(stored.content, page.content);
}
