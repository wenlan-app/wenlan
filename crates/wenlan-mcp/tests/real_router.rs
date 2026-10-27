// SPDX-License-Identifier: Apache-2.0
//! Cross-crate integration test for Spec C-2 mutate wrappers.
//!
//! Adversarial-finding mitigation from PR #101: pure wiremock cannot prove
//! URL alignment between wrapper and daemon route. This test boots the real
//! `origin-server::router::build_router` in-process and invokes each wrapper
//! through a real HTTP server. If a wrapper URL drifts from a daemon route,
//! the real router returns 404 and the assertion fails.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

use rmcp::model::{CallToolResult, ContentBlock};
use wenlan_core::{db::MemoryDB, NoopEmitter};
use wenlan_mcp::{
    client::WenlanClient,
    tools::{
        AcceptRevisionRequest, DismissRevisionRequest, SteepAction, SteepAnswer, SteepParams,
        TransportMode, WenlanMcpServer, WritePageParams,
    },
};
use wenlan_types::{RawDocument, RevisionAcceptResponse, RevisionDismissResponse};

// ── helpers ────────────────────────────────────────────────────────────────

async fn boot_test_server() -> (String, Arc<MemoryDB>) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let db = Arc::new(
        MemoryDB::new(&db_path, Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );

    let state = Arc::new(RwLock::new(wenlan_server::state::ServerState {
        db: Some(db.clone()),
        ..wenlan_server::state::ServerState::default()
    }));
    let router = wenlan_server::router::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });

    // Keep the tempdir alive for the process lifetime via intentional forget.
    std::mem::forget(tmp);
    (format!("http://{}", addr), db)
}

fn make_server(base_url: &str) -> WenlanMcpServer {
    let client = WenlanClient::new(base_url.to_string()).with_agent_name("test-agent".to_string());
    WenlanMcpServer::new(client, TransportMode::Stdio, "test-agent".into(), None)
}

/// Extract the text body from a successful CallToolResult.
fn text_of(result: &CallToolResult) -> String {
    for content in &result.content {
        if let ContentBlock::Text(t) = &content {
            return t.text.clone();
        }
    }
    panic!(
        "expected at least one text Content block; got: {:?}",
        result.content
    );
}

/// Seed a pending revision via public API (upsert_documents).
///
/// The target memory is a normal confirmed memory; the revision memory has
/// `pending_revision: true` and `supersedes: Some(target_source_id)`.
async fn seed_pending_revision(db: &MemoryDB, target_source_id: &str, revision_source_id: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Insert target (original, confirmed)
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: target_source_id.to_string(),
        title: "original".to_string(),
        content: "original content".to_string(),
        last_modified: now,
        confirmed: Some(true),
        stability: Some("confirmed".to_string()),
        memory_type: Some("fact".to_string()),
        ..RawDocument::default()
    }])
    .await
    .unwrap();

    // Insert revision (pending_revision=true, supersedes=target)
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: revision_source_id.to_string(),
        title: "revision".to_string(),
        content: "revised content".to_string(),
        last_modified: now,
        confirmed: Some(false),
        stability: Some("new".to_string()),
        memory_type: Some("fact".to_string()),
        supersedes: Some(target_source_id.to_string()),
        pending_revision: true,
        ..RawDocument::default()
    }])
    .await
    .unwrap();
}

/// Count agent_activity rows matching action + agent.
async fn activity_count(db: &MemoryDB, action: &str, agent: &str) -> usize {
    db.list_agent_activity(100, Some(agent), None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == action)
        .count()
}

// ── tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn accept_revision_aligns_with_real_router() {
    let (base_url, db) = boot_test_server().await;
    seed_pending_revision(&db, "mem_real_acc", "mem_real_acc_rev").await;

    let server = make_server(&base_url);
    let result = server
        .accept_revision_impl(AcceptRevisionRequest {
            target_source_id: "mem_real_acc".into(),
        })
        .await
        .unwrap();

    let body = text_of(&result);
    let parsed: RevisionAcceptResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed.target_source_id, "mem_real_acc");
    assert_eq!(parsed.revision_source_id, "mem_real_acc_rev");

    assert_eq!(
        activity_count(&db, "revision_accept", "test-agent").await,
        1
    );
}

#[tokio::test]
async fn dismiss_revision_aligns_with_real_router() {
    let (base_url, db) = boot_test_server().await;
    seed_pending_revision(&db, "mem_real_dis", "mem_real_dis_rev").await;

    let server = make_server(&base_url);
    let result = server
        .dismiss_revision_impl(DismissRevisionRequest {
            target_source_id: "mem_real_dis".into(),
        })
        .await
        .unwrap();

    let body = text_of(&result);
    let parsed: RevisionDismissResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed.target_source_id, "mem_real_dis");
    assert!(parsed.dismissed);
    assert!(!parsed.wrote);

    assert_eq!(
        activity_count(&db, "revision_dismiss", "test-agent").await,
        1
    );
}

#[tokio::test]
async fn plain_mcp_host_gets_guarded_page_flow_from_daemon_replies() {
    let (base_url, db) = boot_test_server().await;
    wenlan_core::post_write::create_page(
        &db,
        wenlan_types::requests::CreateConceptRequest {
            title: "Household Finances".into(),
            content: "Household finances track budgets, invoices, and renovation payments.".into(),
            summary: None,
            entity_id: None,
            space: wenlan_types::WriteSpaceTarget::Uncategorized,
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".into()),
            workspace: None,
        },
        "test",
        None,
    )
    .await
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut docs = vec![
        (
            "plain-flow-a",
            "The kitchen renovation budget follows Household Finances for cabinet payments.",
        ),
        (
            "plain-flow-b",
            "The kitchen renovation schedule records cabinet work and uses Household Finances for invoice checks.",
        ),
    ];
    docs.extend([
        ("plain-wait-1", "Tomatoes need full sun and regular water."),
        (
            "plain-wait-2",
            "The running plan increases mileage each week.",
        ),
        (
            "plain-wait-3",
            "The reading list starts with maritime history.",
        ),
        (
            "plain-wait-4",
            "The Lisbon itinerary reserves one museum day.",
        ),
    ]);
    db.upsert_documents(
        docs.into_iter()
            .map(|(source_id, content)| RawDocument {
                source: "memory".into(),
                source_id: source_id.into(),
                title: source_id.into(),
                content: content.into(),
                last_modified: now,
                source_agent: Some("plain-host".into()),
                ..RawDocument::default()
            })
            .collect(),
    )
    .await
    .unwrap();

    let server = make_server(&base_url);
    let params = |action, token: Option<String>| SteepParams {
        action,
        page_id: None,
        item_ids: Some(vec!["plain-flow-a".into(), "plain-flow-b".into()]),
        token,
        title: Some("kitchen renovation".into()),
        content: None,
        answers: None,
        limit: None,
        cursor: None,
        space: None,
        session: None,
    };

    let listed = server
        .steep_impl(params(SteepAction::New, None))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(listed["next_step"].as_str().unwrap().contains("claim"));

    let claimed = server
        .steep_impl(params(SteepAction::ClaimNew, None))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let token = claimed["token"].as_str().unwrap().to_string();
    assert!(claimed["next_step"]
        .as_str()
        .unwrap()
        .contains("organize_tasks"));
    assert!(claimed["warnings"][0]
        .as_str()
        .unwrap()
        .contains("Thin group"));

    let tasks = server
        .steep_impl(params(SteepAction::OrganizeTasks, Some(token.clone())))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(tasks["next_step"].as_str().unwrap().contains("organize"));
    let answers = tasks["organize"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| SteepAnswer {
            n: task["n"].as_u64().unwrap() as u32,
            answer: if task["stage"] == "classify" {
                r#"{"memory_type":"fact","domain":"home","tags":["renovation"],"quality":"high","importance":6}"#.into()
            } else {
                r#"{"entities":[],"relations":[]}"#.into()
            },
        })
        .collect();
    let mut organize_params = params(SteepAction::Organize, Some(token.clone()));
    organize_params.answers = Some(answers);
    let organized = server
        .steep_impl(organize_params)
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(organized["remaining"], 0);
    assert!(organized["next_step"].as_str().unwrap().contains("links"));

    let links = server
        .steep_impl(params(SteepAction::Links, Some(token.clone())))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert!(links["link_targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target == "Household Finances"));
    assert!(links["next_step"].as_str().unwrap().contains("write"));

    let mut write_params = params(SteepAction::Write, Some(token));
    write_params.content = Some(
        "The kitchen renovation budget follows Household Finances for cabinet payments [1]. \
         The schedule records cabinet work and uses Household Finances for invoice checks [2]."
            .into(),
    );
    let written = server
        .steep_impl(write_params)
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(written["outcome"], "created");
    assert_eq!(written["title"], "Kitchen Renovation");
    assert!(written["link_targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target == "Household Finances"));
    assert!(!written["link_targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target == "Kitchen Renovation"));
    assert!(written["next_step"]
        .as_str()
        .unwrap()
        .contains("next coherent group"));

    let duplicate = text_of(
        &server
            .write_page_impl(WritePageParams {
                page_id: None,
                title: Some("kitchen renovation".into()),
                content: "A second page with the same title must not land.".into(),
                summary: None,
                entity_id: None,
                space: None,
                source_memory_ids: Vec::new(),
                expected_version: None,
                session: None,
                basis: Vec::new(),
                minor: None,
                code_basis: Vec::new(),
            })
            .await
            .unwrap(),
    );
    assert!(duplicate.contains("already exists; no duplicate was created"));
    assert!(duplicate.contains("next_step:"));
    let concept_pages = db.list_pages("active", 20, 0).await.unwrap();
    assert_eq!(concept_pages.len(), 2, "related page plus one new page");
}
