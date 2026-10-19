// SPDX-License-Identifier: Apache-2.0
mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::truth_contract::{CONTRACT_HEADER, INTENT_HEADER};
use wenlan_server::{router::build_router, state::ServerState};
use wenlan_types::requests::{
    CreateConceptRequest, ExportFormat, ExportPageRequest, ExportPagesRequest, RefreshPageRequest,
    SearchPagesRequest, UpdatePageRequest,
};
use wenlan_types::responses::{
    CreatePageResponse, ExportPageResponse, ExportStats, ListPageRevisionsResponse,
    OrphanLinksResponse, PageInventoryResponse, PageLinksResponse, PageWriteResponse,
    SearchPagesResponse, SuccessResponse,
};
use wenlan_types::{Page, PageSourceWithMemory, RawDocument, WriteSpaceTarget};

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: String,
}

#[derive(Debug, Deserialize)]
struct PageEnvelope {
    page: Page,
}

#[derive(Debug, Deserialize)]
struct StatusEnvelope {
    status: String,
}

#[derive(Clone)]
struct RouteCase {
    method: Method,
    uri: String,
    body: Option<Vec<u8>>,
    marked: bool,
}

fn encoded<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    Some(serde_json::to_vec(value).unwrap())
}

fn page_routes(id: &str) -> Vec<RouteCase> {
    vec![
        RouteCase {
            method: Method::GET,
            uri: "/api/pages".to_string(),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::GET,
            uri: format!("/api/pages/{id}"),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::GET,
            uri: format!("/api/pages/{id}/sources"),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::POST,
            uri: format!("/api/pages/{id}/archive"),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::DELETE,
            uri: format!("/api/pages/{id}"),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::POST,
            uri: "/api/pages/search".to_string(),
            body: encoded(&SearchPagesRequest {
                query: "route probe".to_string(),
                limit: Some(5),
                page_type: None,
                space: None,
            }),
            marked: false,
        },
        RouteCase {
            method: Method::POST,
            uri: "/api/pages".to_string(),
            body: encoded(&CreateConceptRequest {
                title: "Route probe".to_string(),
                content: "Route probe content".to_string(),
                summary: None,
                entity_id: None,
                space: WriteSpaceTarget::Inherit,
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".to_string()),
                workspace: None,
            }),
            marked: false,
        },
        RouteCase {
            method: Method::POST,
            uri: "/api/pages/export".to_string(),
            body: encoded(&ExportPagesRequest {
                vault_path: Some("/tmp/wenlan-page-route-contract-no-db".to_string()),
                format: None,
            }),
            marked: false,
        },
        RouteCase {
            method: Method::POST,
            uri: format!("/api/pages/{id}/export"),
            body: encoded(&ExportPageRequest {
                vault_path: "/tmp/wenlan-page-route-contract-no-db".to_string(),
            }),
            marked: false,
        },
        RouteCase {
            method: Method::GET,
            uri: format!("/api/pages/{id}/links"),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::GET,
            uri: "/api/pages/orphan-links".to_string(),
            body: None,
            marked: false,
        },
        RouteCase {
            method: Method::POST,
            uri: format!("/api/memory/{id}/update-page"),
            body: encoded(&UpdatePageRequest {
                content: "Manual route probe".to_string(),
                source_memory_ids: Vec::new(),
                expected_version: None,
                caller_id: None,
                operation_id: None,
            }),
            marked: false,
        },
        RouteCase {
            method: Method::PUT,
            uri: format!("/api/pages/{id}"),
            body: encoded(&RefreshPageRequest {
                basis: Vec::new(),
                minor: None,
                content: "Refreshed route probe".to_string(),
                source_memory_ids: vec!["route-probe-source".to_string()],
                summary: None,
                expected_version: None,
            }),
            marked: false,
        },
        RouteCase {
            method: Method::GET,
            uri: format!("/api/pages/{id}/revisions"),
            body: None,
            marked: false,
        },
    ]
}

async fn request_bytes(router: &common::AppRouter, case: &RouteCase) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(case.method.clone())
        .uri(&case.uri);
    if case.body.is_some() {
        request = request.header("content-type", "application/json");
    }
    if case.marked {
        request = request
            .header(INTENT_HEADER, "explicit")
            .header(CONTRACT_HEADER, "1")
            .header("x-agent-name", "page-route-contract");
    }
    let response = router
        .clone()
        .oneshot(
            request
                .body(
                    case.body
                        .clone()
                        .map(Body::from)
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn request_typed<T>(router: &common::AppRouter, case: RouteCase) -> (StatusCode, T)
where
    T: DeserializeOwned,
{
    let (status, bytes) = request_bytes(router, &case).await;
    let decoded = serde_json::from_slice::<T>(&bytes).unwrap_or_else(|error| {
        panic!(
            "{} {} returned a non-contract response ({error}): {}",
            case.method,
            case.uri,
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, decoded)
}

fn get(uri: impl Into<String>, marked: bool) -> RouteCase {
    RouteCase {
        method: Method::GET,
        uri: uri.into(),
        body: None,
        marked,
    }
}

fn mutation<T: Serialize>(method: Method, uri: impl Into<String>, body: Option<&T>) -> RouteCase {
    RouteCase {
        method,
        uri: uri.into(),
        body: body.and_then(encoded),
        marked: false,
    }
}

/// Every test here points `WENLAN_DATA_DIR` at its own vault, and the variable is
/// process-wide, so the tests take this lock for their whole body.
fn data_dir_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct WritableKnowledgeConfig {
    previous: Option<std::ffi::OsString>,
    tmp: tempfile::TempDir,
}

impl WritableKnowledgeConfig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            serde_json::json!({ "knowledge_path": pages.to_string_lossy() }).to_string(),
        )
        .unwrap();
        let previous = std::env::var_os("WENLAN_DATA_DIR");
        std::env::set_var("WENLAN_DATA_DIR", tmp.path());
        Self { previous, tmp }
    }

    fn pages_dir(&self) -> std::path::PathBuf {
        self.tmp.path().join("pages")
    }
}

impl Drop for WritableKnowledgeConfig {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

#[tokio::test]
async fn page_routes_preserve_typed_contracts() {
    let _guard = data_dir_lock().lock().await;
    let config = WritableKnowledgeConfig::new();
    assert_eq!(
        wenlan_core::config::load_config().knowledge_path_or_default(),
        config.pages_dir(),
        "the contract must never resolve the default live Page vault"
    );

    let (router, tmp, db) = common::test_app_no_gate_with_page_root().await;
    let source_content = "A grounded source for Page route contracts with [[Missing Route Topic]].";
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: "page-route-source".to_string(),
        title: "Page route source".to_string(),
        content: source_content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".to_string()),
        source_agent: Some("page-route-contract".to_string()),
        confidence: Some(0.9),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();

    let read_id = common::create_page_fixture(
        &db,
        "Page Route Read",
        source_content,
        None,
        &["page-route-source"],
        "authored",
    )
    .await;
    let manual_id = common::create_page_fixture(
        &db,
        "Page Route Manual",
        "Manual edit before",
        None,
        &[],
        "authored",
    )
    .await;
    let refresh_id = common::create_page_fixture(
        &db,
        "Page Route Refresh",
        source_content,
        None,
        &["page-route-source"],
        "distilled",
    )
    .await;
    let archive_id = common::create_page_fixture(
        &db,
        "Page Route Archive",
        "Archive content",
        None,
        &[],
        "authored",
    )
    .await;
    let delete_id = common::create_page_fixture(
        &db,
        "Page Route Delete",
        "Delete content",
        None,
        &[],
        "authored",
    )
    .await;

    let create_request = CreateConceptRequest {
        title: "Page Route Created".to_string(),
        content: "Created through the HTTP Page route".to_string(),
        summary: Some("Created Page route".to_string()),
        entity_id: None,
        space: WriteSpaceTarget::Inherit,
        source_memory_ids: Vec::new(),
        creation_kind: Some("authored".to_string()),
        workspace: None,
    };
    let (status, created): (StatusCode, CreatePageResponse) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages", Some(&create_request)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!created.id.is_empty());

    let (status, listed): (StatusCode, SearchPagesResponse) =
        request_typed(&router, get("/api/pages?limit=50", true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed.pages.iter().any(|page| page.id == read_id));

    let (status, page): (StatusCode, PageEnvelope) =
        request_typed(&router, get(format!("/api/pages/{read_id}"), true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.page.title, "Page Route Read");

    let (status, sources): (StatusCode, Vec<PageSourceWithMemory>) =
        request_typed(&router, get(format!("/api/pages/{read_id}/sources"), true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source.memory_source_id, "page-route-source");

    let search_request = SearchPagesRequest {
        query: "Page Route Read".to_string(),
        limit: Some(10),
        page_type: None,
        space: None,
    };
    let (status, searched): (StatusCode, SearchPagesResponse) = request_typed(
        &router,
        RouteCase {
            marked: true,
            ..mutation(Method::POST, "/api/pages/search", Some(&search_request))
        },
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(searched.pages.iter().any(|page| page.id == read_id));

    let (status, links): (StatusCode, PageLinksResponse) =
        request_typed(&router, get(format!("/api/pages/{read_id}/links"), true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(links.outbound.len(), 1);

    let (status, orphans): (StatusCode, OrphanLinksResponse) =
        request_typed(&router, get("/api/pages/orphan-links", false)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(orphans.min_count, 2);

    let (status, revisions): (StatusCode, ListPageRevisionsResponse) = request_typed(
        &router,
        get(format!("/api/pages/{read_id}/revisions"), true),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revisions.page_id, read_id);

    let bulk_export_dir = tmp.path().join("bulk-export");
    let bulk_export = ExportPagesRequest {
        vault_path: Some(bulk_export_dir.to_string_lossy().into_owned()),
        format: None,
    };
    let (status, exported): (StatusCode, ExportStats) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&bulk_export)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(exported.exported >= 1);

    let single_export_dir = tmp.path().join("single-export");
    let single_export = ExportPageRequest {
        vault_path: single_export_dir.to_string_lossy().into_owned(),
    };
    let (status, exported): (StatusCode, ExportPageResponse) = request_typed(
        &router,
        mutation(
            Method::POST,
            format!("/api/pages/{read_id}/export"),
            Some(&single_export),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(std::path::Path::new(&exported.path).starts_with(&single_export_dir));

    let manual = UpdatePageRequest {
        content: "Manual edit after".to_string(),
        source_memory_ids: Vec::new(),
        expected_version: None,
        caller_id: None,
        operation_id: None,
    };
    let (status, updated): (StatusCode, SuccessResponse) = request_typed(
        &router,
        mutation(
            Method::POST,
            format!("/api/memory/{manual_id}/update-page"),
            Some(&manual),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(updated.ok);

    let refresh = RefreshPageRequest {
        basis: Vec::new(),
        minor: None,
        content: "Refresh content after, grounded in its source memory.".to_string(),
        source_memory_ids: vec!["page-route-source".to_string()],
        summary: Some("Refreshed Page route".to_string()),
        expected_version: None,
    };
    let (status, refreshed): (StatusCode, PageWriteResponse) = request_typed(
        &router,
        mutation(
            Method::PUT,
            format!("/api/pages/{refresh_id}"),
            Some(&refresh),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(refreshed.ok);

    let (status, archived): (StatusCode, StatusEnvelope) = request_typed(
        &router,
        mutation::<()>(
            Method::POST,
            format!("/api/pages/{archive_id}/archive"),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived.status, "archived");

    let (status, deleted): (StatusCode, StatusEnvelope) = request_typed(
        &router,
        mutation::<()>(Method::DELETE, format!("/api/pages/{delete_id}"), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted.status, "deleted");

    let no_db = build_router(Arc::new(RwLock::new(ServerState::default())));
    for case in page_routes("missing-page") {
        let (status, error): (StatusCode, ErrorEnvelope) =
            request_typed(&no_db, case.clone()).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{} {}: {}",
            case.method,
            case.uri,
            error.error
        );
        assert_eq!(error.error, "Database not initialized");
    }
}

#[tokio::test]
async fn outbound_links_show_live_target_title_and_hide_ungranted_titles() {
    let _guard = data_dir_lock().lock().await;
    let (router, tmp, db) = common::test_app_no_gate().await;
    let target_id = common::create_page_fixture(
        &db,
        "Old linked title",
        "Target page body",
        None,
        &[],
        "authored",
    )
    .await;
    let source_id = common::create_page_fixture(
        &db,
        "Link source",
        "This keeps the original mention [[Old linked title]].",
        None,
        &[],
        "authored",
    )
    .await;
    db.replace_page_links(
        &source_id,
        &[wenlan_core::synthesis::wikilinks::Wikilink {
            label: "Old linked title".to_string(),
            target_page_id: Some(target_id.clone()),
        }],
    )
    .await
    .unwrap();

    let before = db.get_page(&target_id).await.unwrap().unwrap();
    wenlan_core::post_write::rename_page(
        &db,
        &target_id,
        "Current linked title",
        before.version,
        &wenlan_core::read_scope::ReadScope::Global,
        None,
    )
    .await
    .unwrap();

    let (status, visible): (StatusCode, PageLinksResponse) =
        request_typed(&router, get(format!("/api/pages/{source_id}/links"), false)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(visible.outbound.len(), 1);
    assert_eq!(visible.outbound[0].label, "Old linked title");
    assert_eq!(
        visible.outbound[0].target_page_id.as_deref(),
        Some(target_id.as_str())
    );
    assert_eq!(
        visible.outbound[0].target_title.as_deref(),
        Some("Current linked title")
    );

    // A named-page grant opens the source, but the target needs a real current
    // unsupported verdict to exercise the route's target-title redaction.
    let target = db.get_page(&target_id).await.unwrap().unwrap();
    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO page_truth_state
            (page_id,page_version,support_status,human_reviewed,updated_at,evaluated_at)
         VALUES (?1,?2,'provisional',0,1,1)",
        libsql::params![target_id.as_str(), target.version],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO claim_derivation_markers
            (page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at)
         VALUES (?1,?2,?3,?4,1,0)",
        libsql::params![
            target_id.as_str(),
            target.version,
            wenlan_core::provenance::revision_content_digest(&target.content),
            wenlan_core::db::EXTRACTOR_VERSION
        ],
    )
    .await
    .unwrap();
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
    let visibility = db
        .page_visibility(
            &wenlan_core::truth_contract::TruthGrant::Automatic,
            std::slice::from_ref(&target_id),
        )
        .await
        .unwrap();
    assert_eq!(
        visibility.get(&target_id),
        Some(&wenlan_core::truth_contract::Visibility::Hidden),
        "the target fixture must be genuinely hidden before testing response redaction"
    );
    let (status, hidden): (StatusCode, PageLinksResponse) =
        request_typed(&router, get(format!("/api/pages/{source_id}/links"), true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hidden.outbound.len(), 1);
    assert_eq!(hidden.outbound[0].label, "Old linked title");
    assert_eq!(hidden.outbound[0].target_page_id, None);
    assert_eq!(hidden.outbound[0].target_title, None);
}

#[tokio::test]
async fn export_okf_format_writes_bundle_and_enforces_safety() {
    let _guard = data_dir_lock().lock().await;
    let _config = WritableKnowledgeConfig::new();
    let (router, tmp, db) = common::test_app_no_gate_with_page_root().await;
    common::create_page_fixture(
        &db,
        "OKF Hub",
        "Links onward to [[OKF Leaf]].",
        None,
        &[],
        "authored",
    )
    .await;
    common::create_page_fixture(&db, "OKF Leaf", "Leaf body.", None, &[], "authored").await;

    // `format: "okf"` into a temp dir writes the bundle and returns stats.
    let bundle = tmp.path().join("okf-bundle");
    let okf = ExportPagesRequest {
        vault_path: Some(bundle.to_string_lossy().into_owned()),
        format: Some(ExportFormat::Okf),
    };
    let (status, stats): (StatusCode, ExportStats) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&okf)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(stats.exported >= 2, "{stats:?}");
    let hub = std::fs::read_to_string(bundle.join("pages/okf-hub.md")).unwrap();
    assert!(hub.contains("[OKF Leaf](/pages/okf-leaf.md)"), "{hub}");
    assert!(!hub.contains("[["), "{hub}");
    assert!(!hub.contains("<!-- origin:"), "{hub}");
    assert!(bundle.join("index.md").is_file());
    assert!(bundle.join(".wenlan-okf-export.json").is_file());
    // A second run over the previous export succeeds.
    let (status, _): (StatusCode, ExportStats) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&okf)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Missing or relative `vault_path` is a validation error naming the problem.
    let missing = ExportPagesRequest {
        vault_path: None,
        format: Some(ExportFormat::Okf),
    };
    let (status, error): (StatusCode, ErrorEnvelope) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&missing)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error.error.contains("vault_path"), "{}", error.error);
    let relative = ExportPagesRequest {
        vault_path: Some("relative/bundle".to_string()),
        format: Some(ExportFormat::Okf),
    };
    let (status, error): (StatusCode, ErrorEnvelope) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&relative)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error.error.contains("absolute"), "{}", error.error);

    // A foreign non-empty directory is a conflict naming the problem.
    let foreign = tmp.path().join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("README.md"), b"not yours").unwrap();
    let foreign_req = ExportPagesRequest {
        vault_path: Some(foreign.to_string_lossy().into_owned()),
        format: Some(ExportFormat::Okf),
    };
    let (status, error): (StatusCode, ErrorEnvelope) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&foreign_req)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        error.error.contains("not a Wenlan OKF export"),
        "{}",
        error.error
    );
}

#[tokio::test]
async fn export_okf_reads_past_the_obsidian_window() {
    let _guard = data_dir_lock().lock().await;
    let _config = WritableKnowledgeConfig::new();
    let (router, tmp, db) = common::test_app_no_gate_with_page_root().await;
    for i in 0..1001 {
        common::create_page_fixture(
            &db,
            &format!("Window Page {i:04}"),
            "body",
            None,
            &[],
            "authored",
        )
        .await;
    }
    // The OKF branch reads every active page in one query, not the
    // Obsidian 1000-row window: all 1001 pages must be exported.
    let bundle = tmp.path().join("okf-window-bundle");
    let okf = ExportPagesRequest {
        vault_path: Some(bundle.to_string_lossy().into_owned()),
        format: Some(ExportFormat::Okf),
    };
    let (status, stats): (StatusCode, ExportStats) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&okf)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats.exported, 1001, "{stats:?}");
    assert_eq!(stats.failed, 0, "{stats:?}");
    assert_eq!(stats.skipped, 0, "{stats:?}");
    let md_count = std::fs::read_dir(bundle.join("pages"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .count();
    assert_eq!(md_count, 1001);
}

#[tokio::test]
async fn export_okf_refuses_during_a_truth_cutover() {
    let _guard = data_dir_lock().lock().await;
    let _config = WritableKnowledgeConfig::new();
    let (router, tmp, db) = common::test_app_no_gate_with_page_root().await;
    common::create_page_fixture(&db, "Fence Page", "body", None, &[], "authored").await;
    let bundle = tmp.path().join("okf-fence-bundle");
    let okf = ExportPagesRequest {
        vault_path: Some(bundle.to_string_lossy().into_owned()),
        format: Some(ExportFormat::Okf),
    };
    let (status, stats): (StatusCode, ExportStats) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&okf)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats.exported, 1, "{stats:?}");
    let page_file = bundle.join("pages/fence-page.md");
    let before = std::fs::read(&page_file).expect("first export wrote the page");

    // Mid-ceremony every page is declined; exporting then would delete the
    // bundle's page files as stale, so the route refuses and touches nothing.
    let lease = db.begin_cutover().await.unwrap();
    let (status, _): (StatusCode, serde_json::Value) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&okf)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(std::fs::read(&page_file).unwrap(), before);

    db.abort_cutover(lease).await.unwrap();
    let (status, stats): (StatusCode, ExportStats) = request_typed(
        &router,
        mutation(Method::POST, "/api/pages/export", Some(&okf)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats.exported, 1, "{stats:?}");
    assert_eq!(std::fs::read(&page_file).unwrap(), before);
}

/// A hidden page inside a `GET /api/pages` window must not end the list for
/// a pager. The app's `listAllPages` and MCP `list_pages` both stop at the
/// first window shorter than `limit`; before windows were counted in visible
/// pages, the five hidden pages below made the first 500-row window return
/// 495 pages and the last 100 pages never loaded.
#[tokio::test]
async fn page_list_windows_count_visible_pages_so_paging_reaches_the_end() {
    const PAGES: usize = 600;
    const LIMIT: usize = 500;
    let _guard = data_dir_lock().lock().await;
    let (router, tmp, db) = common::test_app_no_gate().await;
    let mut ids = Vec::with_capacity(PAGES);
    for index in 0..PAGES {
        ids.push(
            common::create_page_fixture(
                &db,
                &format!("Paged page {index:03}"),
                &format!("Body of paged page {index:03}."),
                None,
                &[],
                "authored",
            )
            .await,
        );
    }

    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    // Distinct timestamps make the newest-first order, and so which window
    // holds which page, deterministic.
    for (index, id) in ids.iter().enumerate() {
        let modified = chrono::DateTime::from_timestamp(1_700_000_000 + index as i64, 0)
            .unwrap()
            .to_rfc3339();
        conn.execute(
            "UPDATE pages SET last_modified = ?1 WHERE id = ?2",
            libsql::params![modified, id.as_str()],
        )
        .await
        .unwrap();
    }
    // Hide five of the newest pages, all inside the first window.
    let hidden: Vec<String> = ids
        .iter()
        .rev()
        .skip(3)
        .step_by(7)
        .take(5)
        .cloned()
        .collect();
    for id in &hidden {
        let page = db.get_page(id).await.unwrap().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO page_truth_state
                (page_id,page_version,support_status,human_reviewed,updated_at,evaluated_at)
             VALUES (?1,?2,'provisional',0,1,1)",
            libsql::params![id.as_str(), page.version],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO claim_derivation_markers
                (page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at)
             VALUES (?1,?2,?3,?4,1,0)",
            libsql::params![
                id.as_str(),
                page.version,
                wenlan_core::provenance::revision_content_digest(&page.content),
                wenlan_core::db::EXTRACTOR_VERSION
            ],
        )
        .await
        .unwrap();
    }
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
    let visibility = db
        .page_visibility(&wenlan_core::truth_contract::TruthGrant::Automatic, &ids)
        .await
        .unwrap();
    for id in &ids {
        let expected = if hidden.contains(id) {
            wenlan_core::truth_contract::Visibility::Hidden
        } else {
            wenlan_core::truth_contract::Visibility::Full
        };
        assert_eq!(
            visibility.get(id),
            Some(&expected),
            "fixture visibility for {id}"
        );
    }

    // Page exactly as `listAllPages.ts` does: advance by what came back and
    // stop at the first short window.
    let mut seen = Vec::new();
    let mut windows = Vec::new();
    loop {
        let (status, window): (StatusCode, PageInventoryResponse) = request_typed(
            &router,
            get(
                format!("/api/pages?limit={LIMIT}&offset={}", seen.len()),
                false,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        windows.push(window.pages.len());
        seen.extend(window.pages.into_iter().map(|entry| entry.page.id));
        if windows.last() < Some(&LIMIT) {
            break;
        }
    }

    assert_eq!(windows, vec![LIMIT, PAGES - hidden.len() - LIMIT]);
    let mut expected: Vec<String> = ids
        .iter()
        .rev()
        .filter(|id| !hidden.contains(id))
        .cloned()
        .collect();
    assert_eq!(seen.len(), expected.len(), "no page may load twice");
    assert_eq!(seen, expected, "every visible page, newest first");
    expected.sort();
    expected.dedup();
    assert_eq!(expected.len(), PAGES - hidden.len());
}
