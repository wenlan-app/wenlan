// SPDX-License-Identifier: Apache-2.0
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_server::state::ServerState;
use wenlan_types::onboarding::{MilestoneId, MilestoneRecord};
use wenlan_types::responses::{
    AcceptRefinementResponse, KnowledgeCountResponse, KnowledgePathResponse,
    ListRefinementsResponse, RejectRefinementResponse,
};

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: String,
}

struct DataDirGuard {
    previous: Option<std::ffi::OsString>,
    _root: tempfile::TempDir,
}

impl DataDirGuard {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("WENLAN_DATA_DIR");
        std::env::set_var("WENLAN_DATA_DIR", root.path());
        Self {
            previous,
            _root: root,
        }
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var("WENLAN_DATA_DIR", previous);
        } else {
            std::env::remove_var("WENLAN_DATA_DIR");
        }
    }
}

async fn request_bytes(
    router: &common::AppRouter,
    method: Method,
    uri: &str,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(&method)
                .uri(uri)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn request_typed<T>(
    router: &common::AppRouter,
    method: Method,
    uri: &str,
    body: Body,
) -> (StatusCode, T)
where
    T: DeserializeOwned,
{
    let (status, bytes) = request_bytes(router, method.clone(), uri, body).await;
    let decoded = serde_json::from_slice::<T>(&bytes).unwrap_or_else(|error| {
        panic!(
            "{method} {uri} returned a non-contract response ({error}): {}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, decoded)
}

async fn seed_awaiting_refinement(db: &Arc<MemoryDB>, id: &str) {
    db.insert_refinement_proposal(
        id,
        "detect_contradiction",
        &["source-a".to_string(), "source-b".to_string()],
        None,
        0.8,
    )
    .await
    .unwrap();
    db.resolve_refinement_if_open(id, "awaiting_review")
        .await
        .unwrap();
}

#[tokio::test]
async fn moved_knowledge_onboarding_and_refinery_routes_preserve_typed_contracts() {
    let _config_root = DataDirGuard::new();
    let page_root = tempfile::tempdir().unwrap();
    std::fs::write(page_root.path().join("alpha.md"), "# Alpha").unwrap();
    std::fs::write(page_root.path().join("beta.MD"), "# Beta").unwrap();
    std::fs::write(page_root.path().join("ignored.txt"), "ignored").unwrap();
    let config = wenlan_core::config::Config {
        knowledge_path: Some(page_root.path().to_path_buf()),
        ..Default::default()
    };
    wenlan_core::config::save_config(&config).unwrap();

    let (router, _db_tmp, db) = common::test_app_no_gate().await;

    let (status, path): (StatusCode, KnowledgePathResponse) =
        request_typed(&router, Method::GET, "/api/knowledge/path", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(path.path, page_root.path().to_string_lossy());

    let (status, count): (StatusCode, KnowledgeCountResponse) =
        request_typed(&router, Method::GET, "/api/knowledge/count", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count.count, 2);

    let file_config = wenlan_core::config::Config {
        knowledge_path: Some(page_root.path().join("ignored.txt")),
        ..Default::default()
    };
    wenlan_core::config::save_config(&file_config).unwrap();
    let (status, error): (StatusCode, ErrorEnvelope) =
        request_typed(&router, Method::GET, "/api/knowledge/count", Body::empty()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!error.error.is_empty());
    wenlan_core::config::save_config(&config).unwrap();

    db.record_milestone(MilestoneId::FirstMemory, None)
        .await
        .unwrap();
    let (status, milestones): (StatusCode, Vec<MilestoneRecord>) = request_typed(
        &router,
        Method::GET,
        "/api/onboarding/milestones",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(milestones.len(), 1);
    assert_eq!(milestones[0].id, MilestoneId::FirstMemory);
    assert!(milestones[0].acknowledged_at.is_none());

    let (status, body) = request_bytes(
        &router,
        Method::POST,
        "/api/onboarding/milestones/first-memory/acknowledge",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());

    let (status, milestones): (StatusCode, Vec<MilestoneRecord>) = request_typed(
        &router,
        Method::GET,
        "/api/onboarding/milestones",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(milestones[0].acknowledged_at.is_some());

    let (status, error): (StatusCode, ErrorEnvelope) = request_typed(
        &router,
        Method::POST,
        "/api/onboarding/milestones/not-a-milestone/acknowledge",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error.error.contains("invalid milestone id"));

    let (status, body) = request_bytes(
        &router,
        Method::POST,
        "/api/onboarding/reset",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty());

    let (status, milestones): (StatusCode, Vec<MilestoneRecord>) = request_typed(
        &router,
        Method::GET,
        "/api/onboarding/milestones",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(milestones.is_empty());

    seed_awaiting_refinement(&db, "ref-reject").await;
    seed_awaiting_refinement(&db, "ref-accept").await;
    let (status, refinements): (StatusCode, ListRefinementsResponse) =
        request_typed(&router, Method::GET, "/api/refinery/queue", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refinements.proposals.len(), 2);

    let (status, rejected): (StatusCode, RejectRefinementResponse) = request_typed(
        &router,
        Method::POST,
        "/api/refinery/queue/ref-reject/reject",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rejected.id, "ref-reject");

    let (status, accepted): (StatusCode, AcceptRefinementResponse) = request_typed(
        &router,
        Method::POST,
        "/api/refinery/queue/ref-accept/accept",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted.id, "ref-accept");
    assert_eq!(accepted.action_applied, "detect_contradiction");

    let no_db_router =
        wenlan_server::router::build_router(Arc::new(RwLock::new(ServerState::default())));
    for (method, uri) in [
        (Method::GET, "/api/onboarding/milestones"),
        (
            Method::POST,
            "/api/onboarding/milestones/first-memory/acknowledge",
        ),
        (Method::POST, "/api/onboarding/reset"),
        (Method::GET, "/api/refinery/queue"),
        (Method::POST, "/api/refinery/queue/missing/reject"),
        (Method::POST, "/api/refinery/queue/missing/accept"),
    ] {
        let (status, error): (StatusCode, ErrorEnvelope) =
            request_typed(&no_db_router, method, uri, Body::empty()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert_eq!(error.error, "Database not initialized", "{uri}");
    }
}

#[tokio::test]
async fn a_pages_tags_and_its_image_embeds_read_as_obsidian_reads_them() {
    // The pages folder comes from the server state, so this test leaves the
    // process-wide data dir alone (the other test here sets it).
    let (router, dir, db) = common::test_app_no_gate_with_page_root().await;
    let pages = dir.path().join("pages");
    std::fs::create_dir_all(pages.join("trips")).unwrap();
    std::fs::create_dir_all(pages.join(".obsidian")).unwrap();
    std::fs::write(pages.join("trips/cat.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    std::fs::write(pages.join(".obsidian/bird.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    let page = common::create_page_fixture(
        &db,
        "Trip",
        "Kyoto in spring #travel #旅行.\n\n![[cat.png|300]]",
        None,
        &[],
        "authored",
    )
    .await;

    let (status, read): (StatusCode, wenlan_types::responses::PageReadResponse) = request_typed(
        &router,
        Method::GET,
        &format!("/api/pages/{page}"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read.page.tags, vec!["travel", "旅行"]);

    let (status, embed): (StatusCode, wenlan_types::responses::ResolveEmbedResponse) =
        request_typed(
            &router,
            Method::GET,
            &format!("/api/attachments/resolve?name=cat.png%7C300&page_id={page}"),
            Body::empty(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(embed.relative, "trips/cat.png");
    assert!(embed.path.ends_with("trips/cat.png"), "{}", embed.path);
    assert_eq!(embed.kind, "image");

    for (uri, why) in [
        (
            "/api/attachments/resolve?name=bird.png".to_string(),
            "hidden folder",
        ),
        (
            "/api/attachments/resolve?name=missing.png".to_string(),
            "no such file",
        ),
        (
            "/api/attachments/resolve?name=..%2Fescape.png".to_string(),
            "outside",
        ),
        (
            "/api/attachments/resolve?name=cat.png&page_id=ghost".to_string(),
            "unknown page",
        ),
    ] {
        let (status, _) = request_bytes(&router, Method::GET, &uri, Body::empty()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{why}");
    }
}
