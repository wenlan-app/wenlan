// SPDX-License-Identifier: Apache-2.0
//! Looks: an HTML layout an AI tool drew for one page (UX-LOOK-1..6).
//!
//! - `GET /api/pages/{id}/look`: the page's active look and its versions.
//! - `PUT /api/pages/{id}/look`: save a look. Refused with a plain reason
//!   when it reaches for the internet or copies the page's text.
//! - `DELETE /api/pages/{id}/look`: take the look off; every version stays.
//! - `POST /api/pages/{id}/look/restore`: put an earlier version back.
//! - `POST /api/pages/{id}/look/error`: what went wrong while showing it.
//! - `GET /api/pages/{id}/look/data`: `window.wenlan` for the page, the same
//!   data the app puts in the look's frame and the MCP tool shows an AI.
//!
//! A refusal answers 422 `look_refused: <plain reason>`, which the MCP passes
//! on word for word so the AI can act on it.
//!
//! A look is stored in its own table, so the page file and the vault do not
//! change when one is set. Every route answers only for a page the caller can
//! see in its Space and under its truth grant; any other page reads as absent.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use tokio::sync::RwLock;

use wenlan_core::db::MemoryDB;
use wenlan_types::page_look::{
    LookData, PageLookErrorRequest, PageLookErrorResponse, PageLookResponse,
    RemovePageLookResponse, RestorePageLookRequest, SetPageLookRequest, SetPageLookResponse,
    LOOK_API, LOOK_SPACE_PAGES_MAX,
};
use wenlan_types::pages::Page;

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/pages/{id}/look",
            get(handle_get_page_look)
                .put(handle_set_page_look)
                .delete(handle_remove_page_look),
        )
        .route(
            "/api/pages/{id}/look/restore",
            post(handle_restore_page_look),
        )
        .route("/api/pages/{id}/look/error", post(handle_page_look_error))
        .route("/api/pages/{id}/look/data", get(handle_get_page_look_data))
}

async fn open_db(state: &Arc<RwLock<ServerState>>) -> Result<Arc<MemoryDB>, ServerError> {
    state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)
}

/// The page, when the caller may see it in full; absent otherwise.
async fn visible_page(
    db: &MemoryDB,
    view: &crate::truth_guard::TruthView,
    header_space: Option<&str>,
    id: &str,
) -> Result<Page, ServerError> {
    let scope = crate::read_scope::effective_read_scope(db, None, header_space).await?;
    let visible = crate::page_basis_routes::visible_ids(
        db,
        &view.grant,
        &scope,
        std::slice::from_ref(&id.to_string()),
    )
    .await?;
    if !visible.contains(id) {
        return Err(ServerError::NotFound(format!("page {id} not found")));
    }
    db.get_page_scoped_browse(id, &scope)
        .await?
        .ok_or_else(|| ServerError::NotFound(format!("page {id} not found")))
}

/// A page that may take a look: a normal, active page of the person's.
async fn lookable_page(
    db: &MemoryDB,
    view: &crate::truth_guard::TruthView,
    header_space: Option<&str>,
    id: &str,
) -> Result<Page, ServerError> {
    let page = visible_page(db, view, header_space, id).await?;
    if page.kind == "entity" {
        return Err(ServerError::ValidationError(
            "look_refused: A topic's page cannot have a look. Pick one of the person's own pages."
                .into(),
        ));
    }
    if page.status != "active" {
        return Err(ServerError::ValidationError(
            "look_refused: This page is archived, so it cannot have a look.".into(),
        ));
    }
    if page.creation_kind == "original" {
        return Err(ServerError::ValidationError(
            "look_refused: A kept original shows as it was made and cannot have a look.".into(),
        ));
    }
    Ok(page)
}

/// GET /api/pages/{id}/look
pub async fn handle_get_page_look(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<PageLookResponse>, ServerError> {
    let db = open_db(&state).await?;
    visible_page(&db, &view, header_space.as_deref(), &id).await?;
    Ok(Json(PageLookResponse {
        look: db.get_active_page_look(&id).await?,
        versions: db.list_page_look_versions(&id).await?,
    }))
}

/// PUT /api/pages/{id}/look
pub async fn handle_set_page_look(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<SetPageLookRequest>,
) -> Result<Json<SetPageLookResponse>, ServerError> {
    let db = open_db(&state).await?;
    let page = lookable_page(&db, &view, header_space.as_deref(), &id).await?;
    wenlan_core::page_look::check_look(&req.html, &page.content)
        .map_err(|reason| ServerError::ValidationError(format!("look_refused: {reason}")))?;
    let session = crate::session_routes::extract_session_ref(&headers).map(|s| s.to_string());
    let (look_id, created) = db
        .set_page_look(&id, &req.html, LOOK_API, session.as_deref())
        .await?;
    Ok(Json(SetPageLookResponse {
        page_id: id,
        look_id,
        created,
    }))
}

/// DELETE /api/pages/{id}/look
pub async fn handle_remove_page_look(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<RemovePageLookResponse>, ServerError> {
    let db = open_db(&state).await?;
    visible_page(&db, &view, header_space.as_deref(), &id).await?;
    let removed_id = db.remove_page_look(&id).await?;
    Ok(Json(RemovePageLookResponse {
        page_id: id,
        removed_id,
    }))
}

/// POST /api/pages/{id}/look/restore
pub async fn handle_restore_page_look(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<RestorePageLookRequest>,
) -> Result<Json<SetPageLookResponse>, ServerError> {
    let db = open_db(&state).await?;
    lookable_page(&db, &view, header_space.as_deref(), &id).await?;
    let look_id = db.restore_page_look(&id, req.id).await?.ok_or_else(|| {
        ServerError::NotFound(format!("page {id} has no earlier look to restore"))
    })?;
    Ok(Json(SetPageLookResponse {
        page_id: id,
        look_id,
        created: false,
    }))
}

/// POST /api/pages/{id}/look/error
pub async fn handle_page_look_error(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
    Json(req): Json<PageLookErrorRequest>,
) -> Result<Json<PageLookErrorResponse>, ServerError> {
    let db = open_db(&state).await?;
    visible_page(&db, &view, header_space.as_deref(), &id).await?;
    let message: String = req.message.trim().chars().take(2000).collect();
    let recorded = !message.is_empty() && db.record_page_look_error(&id, &message).await?;
    Ok(Json(PageLookErrorResponse { recorded }))
}

/// GET /api/pages/{id}/look/data
///
/// `window.wenlan` for the page: its part for people cut into sections, its
/// links, and its Space's page list. The theme is the daemon's default; the
/// app fills in the one in use.
pub async fn handle_get_page_look_data(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<LookData>, ServerError> {
    let db = open_db(&state).await?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let page = visible_page(&db, &view, header_space.as_deref(), &id).await?;
    let space_name = page.space.clone().unwrap_or_default();

    // A link names a page only when the caller may see that page.
    let outbound = db.get_page_outbound_links_scoped(&id, &scope).await?;
    let targets: Vec<String> = outbound
        .iter()
        .filter_map(|link| link.target_page_id.clone())
        .collect();
    let visible_targets =
        crate::page_basis_routes::visible_ids(&db, &view.grant, &scope, &targets).await?;
    let link_targets = wenlan_core::page_look::look_link_targets(outbound.iter().map(|link| {
        (
            link.label.as_str(),
            link.target_page_id
                .as_deref()
                .filter(|target| visible_targets.contains(*target)),
        )
    }));

    // The Space's pages, newest first, without their text.
    let space_key = if space_name.is_empty() {
        "uncategorized"
    } else {
        space_name.as_str()
    };
    let listed = db
        .list_pages_by_space("active", Some(space_key), LOOK_SPACE_PAGES_MAX + 1, 0)
        .await?;
    let listed_ids: Vec<String> = listed.iter().map(|p| p.id.clone()).collect();
    let visible =
        crate::page_basis_routes::visible_ids(&db, &view.grant, &scope, &listed_ids).await?;
    let more = listed.len() > LOOK_SPACE_PAGES_MAX;
    let space_pages: Vec<Page> = listed
        .into_iter()
        .filter(|p| visible.contains(&p.id) && p.creation_kind != "original")
        .collect();

    Ok(Json(wenlan_core::page_look::look_data(
        &page,
        &link_targets,
        &space_name,
        &space_pages,
        more,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    struct ScratchDataDir {
        previous: Option<std::ffi::OsString>,
        _tmp: tempfile::TempDir,
    }

    impl ScratchDataDir {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let pages = tmp.path().join("pages");
            std::fs::create_dir_all(&pages).unwrap();
            std::fs::write(
                tmp.path().join("config.json"),
                json!({ "knowledge_path": pages.to_string_lossy() }).to_string(),
            )
            .unwrap();
            let previous = std::env::var_os("WENLAN_DATA_DIR");
            std::env::set_var("WENLAN_DATA_DIR", tmp.path());
            Self {
                previous,
                _tmp: tmp,
            }
        }
    }

    impl Drop for ScratchDataDir {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
                None => std::env::remove_var("WENLAN_DATA_DIR"),
            }
        }
    }

    async fn send(
        app: &crate::router::AppRouter,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (u16, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(value) => {
                req = req.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), 4 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    /// UX-LOOK-2/3/5 at the route: a look is saved, shown with the page's
    /// data, replaced with the old one kept, refused with a reason when it
    /// reaches for the internet, taken off and put back.
    #[tokio::test]
    async fn ux_look_routes_save_read_refuse_remove_and_restore() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let _env = ScratchDataDir::new();
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let docs = ["a", "b", "c"]
            .iter()
            .map(|n| wenlan_core::sources::RawDocument {
                source: "memory".to_string(),
                source_id: format!("mem-look-{n}"),
                title: format!("mem-look-{n}"),
                content: format!("Kyoto trip fact {n}"),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                source_agent: Some("test".to_string()),
                confidence: Some(0.9),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        db.upsert_documents(docs).await.unwrap();
        let mut tuning = wenlan_core::tuning::TuningConfig::default();
        tuning.distillation.page_min_cluster_size = 3;
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            tuning,
            ..Default::default()
        }));
        state.read().await.maintenance_coordinator.finish_recovery();
        let app = crate::router::build_router(state);

        let (status, page) = send(
            &app,
            "POST",
            "/api/pages",
            Some(json!({
                "title": "Kyoto trip",
                "content": "## Plan\n\n- [ ] book the ryokan\n- [x] buy the rail pass",
                "summary": "Kyoto plan",
                "source_memory_ids": ["mem-look-a", "mem-look-b", "mem-look-c"],
            })),
        )
        .await;
        assert_eq!(status, 200, "{page}");
        let id = page["id"].as_str().unwrap().to_string();
        let uri = format!("/api/pages/{id}/look");

        let (status, none) = send(&app, "GET", &uri, None).await;
        assert_eq!(status, 200);
        assert!(none["look"].is_null());

        let first = "<!doctype html><html><body><h1 id=t></h1><script>document.getElementById('t').textContent = window.wenlan.page.title</script></body></html>";
        let (status, saved) = send(&app, "PUT", &uri, Some(json!({ "html": first }))).await;
        assert_eq!(status, 200, "{saved}");
        assert_eq!(saved["created"], true);
        let (_, same) = send(&app, "PUT", &uri, Some(json!({ "html": first }))).await;
        assert_eq!(same["created"], false, "the same look is not a new version");

        let second = first.replace("<h1", "<h2").replace("</h1>", "</h2>");
        let (status, _) = send(&app, "PUT", &uri, Some(json!({ "html": second }))).await;
        assert_eq!(status, 200);
        let (_, got) = send(&app, "GET", &uri, None).await;
        assert_eq!(got["look"]["html"], second);
        assert_eq!(got["versions"].as_array().unwrap().len(), 2);

        // Refused, with a reason the AI can act on; nothing changes.
        let (status, refused) = send(
            &app,
            "PUT",
            &uri,
            Some(json!({ "html": "<html><img src=\"https://example.com/x.png\"></html>" })),
        )
        .await;
        assert_eq!(status, 422);
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .starts_with("look_refused: "),
            "{refused}"
        );
        let (_, still) = send(&app, "GET", &uri, None).await;
        assert_eq!(still["look"]["html"], second);

        // The data the frame gets.
        let (status, data) = send(&app, "GET", &format!("{uri}/data"), None).await;
        assert_eq!(status, 200, "{data}");
        assert_eq!(data["api"], 1);
        assert_eq!(data["page"]["title"], "Kyoto trip");
        assert_eq!(data["page"]["sections"][0]["heading"], "Plan");

        // Off, then back.
        let (status, removed) = send(&app, "DELETE", &uri, None).await;
        assert_eq!(status, 200);
        assert!(removed["removed_id"].is_number());
        let (_, off) = send(&app, "GET", &uri, None).await;
        assert!(off["look"].is_null());
        assert_eq!(off["versions"].as_array().unwrap().len(), 2);
        let (status, _) = send(&app, "POST", &format!("{uri}/restore"), Some(json!({}))).await;
        assert_eq!(status, 200);
        let (_, back) = send(&app, "GET", &uri, None).await;
        assert_eq!(back["look"]["html"], second);

        // The app says what went wrong; an AI reads it next time.
        let (_, rec) = send(
            &app,
            "POST",
            &format!("{uri}/error"),
            Some(json!({ "message": "ReferenceError: x is not defined" })),
        )
        .await;
        assert_eq!(rec["recorded"], true);
        let (_, withe) = send(&app, "GET", &uri, None).await;
        assert_eq!(
            withe["look"]["last_error"],
            "ReferenceError: x is not defined"
        );

        // An unknown page reads as absent.
        let (status, _) = send(&app, "GET", "/api/pages/pg_nope/look", None).await;
        assert_eq!(status, 404);
    }
}
