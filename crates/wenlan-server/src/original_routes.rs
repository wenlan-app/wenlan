// SPDX-License-Identifier: Apache-2.0
//! Where a page made from a kept original came from.
//!
//! An imported chat's artifacts and canvases are kept as files with their own
//! pages (`wenlan_core::chat_import::originals`). This route tells the app,
//! for one such page, the vendor, the vendor's conversation id and URL (so it
//! can open the conversation in the original AI tool) and the kept file. It
//! answers only for a page the caller can see in its Space and under its
//! truth grant; any other page, and a page not made from an original, reads
//! as absent.
//!
//! `POST /api/originals` keeps what an AI tool made in a live session (an
//! HTML page, an SVG, a Markdown document) the same way, as its own page that
//! names the session (UX-ART-1..3).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use tokio::sync::RwLock;

use wenlan_types::originals::{KeepOriginalRequest, KeepOriginalResponse, PageOriginalResponse};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/pages/{id}/original", get(handle_get_page_original))
        .route("/api/originals", post(handle_keep_original))
}

/// POST /api/originals
///
/// Keep an HTML page, SVG or Markdown document an AI tool made in this
/// session as it is, as its own page. The session comes from the
/// `x-wenlan-session` header and is stamped on the page, so the page shows
/// which conversation made it and when. Keeping the same content again in
/// the same session returns the page it already has.
pub async fn handle_keep_original(
    State(state): State<Arc<RwLock<ServerState>>>,
    headers: HeaderMap,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<KeepOriginalRequest>,
) -> Result<Json<KeepOriginalResponse>, ServerError> {
    use wenlan_core::chat_import::originals::{
        keep_session_original, SessionOriginal, SessionOriginalFormat,
    };
    let format = SessionOriginalFormat::from_name(&req.format)
        .ok_or_else(|| ServerError::ValidationError("original_format_unsupported".into()))?;
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let session = crate::session_routes::extract_session_ref(&headers);
    // Kept already: the same page, and no second structure memory.
    if let Some(kept) = wenlan_core::chat_import::originals::find_session_original(
        &db,
        session.as_ref(),
        &req.content,
    )
    .await?
    {
        let title = db
            .get_page(&kept.page_id)
            .await?
            .map(|page| page.title)
            .unwrap_or_else(|| req.title.trim().to_string());
        return Ok(Json(KeepOriginalResponse {
            page_id: kept.page_id,
            title,
            created: false,
        }));
    }
    if req.title.trim().is_empty() {
        return Err(ServerError::ValidationError(
            "original_title_missing".into(),
        ));
    }
    // The structure is a memory of this session, written the way a capture
    // is (same Space, session and agent), so the night's tidy folds it into
    // the session's thing and later recall finds it; the page cites it.
    let mut source_memory_ids = Vec::new();
    if let Some(structure) = req
        .structure
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let memory: wenlan_types::requests::StoreMemoryRequest =
            serde_json::from_value(serde_json::json!({
                "content": format!(
                    "Diagram \"{}\", kept as its own page from this conversation. What it shows:\n{structure}",
                    req.title.trim()
                ),
                "title": format!("Diagram: {}", req.title.trim()),
            }))
            .map_err(|e| ServerError::Internal(e.to_string()))?;
        let Json(stored) = crate::memory_routes::handle_store_memory(
            State(state.clone()),
            headers.clone(),
            crate::space_header::SpaceHeader(header_space.clone()),
            Json(memory),
        )
        .await?;
        source_memory_ids.push(stored.source_id);
    }
    let write_started_at = chrono::Utc::now().timestamp();
    let (kept, created, space) = {
        let _space_write_guard = db.lock_space_writes().await;
        let resolved = db
            .resolve_write_space(
                &wenlan_types::WriteSpaceTarget::Inherit,
                header_space.as_deref(),
            )
            .await?;
        let space = match resolved.space_name.as_ref() {
            Some(name) => wenlan_types::WriteSpaceTarget::Named(name.clone()),
            None => wenlan_types::WriteSpaceTarget::Uncategorized,
        };
        let data_root = wenlan_core::config::data_root();
        let (kept, created) = keep_session_original(
            &db,
            &data_root,
            page_root.as_deref(),
            SessionOriginal {
                session: session.as_ref(),
                title: &req.title,
                content: &req.content,
                format,
                space,
                workspace: resolved.space_name.clone(),
                source_memory_ids,
            },
        )
        .await?;
        (kept, created, resolved.space_name)
    };
    // Attribution is best effort: the page is already written.
    if let (Some(session), true) = (session.as_ref(), created) {
        if let Err(error) = db
            .stamp_page_history_session(&kept.page_id, write_started_at, session, space.as_deref())
            .await
        {
            tracing::warn!(
                "[session] failed to stamp original {} with {session}: {error}",
                kept.page_id
            );
        }
    }
    let title = db
        .get_page(&kept.page_id)
        .await?
        .map(|page| page.title)
        .unwrap_or_else(|| req.title.trim().to_string());
    Ok(Json(KeepOriginalResponse {
        page_id: kept.page_id,
        title,
        created,
    }))
}

/// GET /api/pages/{id}/original
pub async fn handle_get_page_original(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    Path(id): Path<String>,
) -> Result<Json<PageOriginalResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    let visible =
        crate::page_basis_routes::visible_ids(&db, &view.grant, &scope, std::slice::from_ref(&id))
            .await?;
    if !visible.contains(&id) {
        return Err(ServerError::NotFound(format!("page {id} not found")));
    }
    wenlan_core::chat_import::originals::page_original(&db, &id)
        .await?
        .map(Json)
        .ok_or_else(|| ServerError::NotFound(format!("page {id} was not made from an original")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    struct ScratchDataDir(Option<std::ffi::OsString>);

    impl Drop for ScratchDataDir {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
                None => std::env::remove_var("WENLAN_DATA_DIR"),
            }
        }
    }

    #[tokio::test]
    async fn ux_art_2_keeping_a_session_diagram_keeps_its_structure_as_a_memory_of_that_session() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        std::fs::create_dir_all(&data_root).unwrap();
        let _env = ScratchDataDir(std::env::var_os("WENLAN_DATA_DIR"));
        std::env::set_var("WENLAN_DATA_DIR", &data_root);
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(
                &dir.path().join("db"),
                Arc::new(wenlan_core::events::NoopEmitter),
            )
            .await
            .unwrap(),
        );
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        state.read().await.maintenance_coordinator.finish_recovery();
        let app = crate::router::build_router(state);
        let keep = |body: serde_json::Value| {
            Request::builder()
                .method("POST")
                .uri("/api/originals")
                .header("content-type", "application/json")
                .header("x-agent-name", "claude-code")
                .header("x-wenlan-session", "claude-code:route-s1")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let body = serde_json::json!({
            "title": "Release map",
            "content": "<!doctype html><h1>Release map</h1>",
            "format": "html",
            "structure": "Release map\n├─ decided: ship daily\n└─ open: who tests the handoff",
        });
        let mut kept = Vec::new();
        for _ in 0..2 {
            let res = app.clone().oneshot(keep(body.clone())).await.unwrap();
            assert_eq!(res.status(), 200);
            let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap();
            kept.push(serde_json::from_slice::<KeepOriginalResponse>(&bytes).unwrap());
        }
        assert!(kept[0].created);
        assert!(!kept[1].created);
        assert_eq!(kept[0].page_id, kept[1].page_id);

        // The page cites one memory: the structure, written in that session.
        let page = db.get_page(&kept[0].page_id).await.unwrap().unwrap();
        assert_eq!(page.creation_kind, "original");
        assert_eq!(
            page.source_memory_ids.len(),
            1,
            "a second keep adds no memory"
        );
        let session = wenlan_types::sessions::SessionRef::parse("claude-code:route-s1").unwrap();
        let detail = db
            .get_session_detail(
                &wenlan_core::db::sessions::session_row_id(&session),
                &wenlan_core::read_scope::ReadScope::Global,
            )
            .await
            .unwrap()
            .expect("the session is known");
        assert!(detail
            .memories
            .iter()
            .any(|memory| memory.source_id == page.source_memory_ids[0]));
        assert!(detail.pages.iter().any(|entry| entry.id == page.id));
        let memory = db
            .get_memory_detail(&page.source_memory_ids[0])
            .await
            .unwrap()
            .unwrap();
        assert!(memory.content.contains("└─ open: who tests the handoff"));

        // A format it cannot show as it was is refused.
        let res = app
            .oneshot(keep(serde_json::json!({
                "title": "Script", "content": "print(1)", "format": "py",
            })))
            .await
            .unwrap();
        assert_eq!(res.status(), 422);
    }
}
