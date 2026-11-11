// SPDX-License-Identifier: Apache-2.0
use crate::error::ServerError;
use crate::route_registry::{get, post, put, TrackedRouter};
use crate::state::{ServerState, SharedState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::requests::SetDefaultSpaceRequest;
use wenlan_types::responses::DefaultSpaceResponse;

pub(crate) async fn registered_request_space(
    db: &wenlan_core::db::MemoryDB,
    requested: &Option<String>,
    context: &str,
) -> Result<Option<String>, ServerError> {
    let proposed = requested
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let registered = db
        .registered_space_or_none(requested.as_deref())
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    if registered.is_none() {
        if let Some(space) = proposed {
            tracing::warn!(
                "[memory] ignoring unregistered space {:?} for {}; using unscoped fallback",
                space,
                context
            );
        }
    }
    Ok(registered)
}

pub(crate) fn register_core(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/spaces",
            get(handle_list_spaces).post(handle_create_space),
        )
        .route(
            "/api/spaces/default",
            get(handle_get_default_space)
                .put(handle_set_default_space)
                .delete(handle_clear_default_space),
        )
        .route(
            "/api/spaces/{name}",
            put(handle_update_space).delete(handle_delete_space),
        )
        .route("/api/spaces/{from}/move-to/{to}", post(handle_move_space))
}

// ===== Space CRUD Handlers =====

#[derive(Debug, Deserialize)]
pub struct CreateSpaceRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSpaceRequest {
    pub new_name: Option<String>,
    pub description: Option<String>,
}

pub async fn handle_get_default_space(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<DefaultSpaceResponse>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    Ok(Json(DefaultSpaceResponse {
        space: db.get_default_space().await?,
    }))
}

pub async fn handle_set_default_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<SetDefaultSpaceRequest>,
) -> Result<Json<DefaultSpaceResponse>, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    Ok(Json(DefaultSpaceResponse {
        space: Some(db.set_default_space(&request.space_id).await?),
    }))
}

pub async fn handle_clear_default_space(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<StatusCode, ServerError> {
    let db = {
        let state = state.read().await;
        state.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    db.clear_default_space().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn handle_list_spaces(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<Vec<wenlan_core::db::Space>>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let spaces = db
        .list_spaces()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(spaces))
}

pub async fn handle_create_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<CreateSpaceRequest>,
) -> Result<Json<wenlan_core::db::Space>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let space = db
        .create_space(&req.name, req.description.as_deref(), false)
        .await
        .map_err(ServerError::from)?;
    Ok(Json(space))
}

pub async fn handle_update_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    Json(req): Json<UpdateSpaceRequest>,
) -> Result<Json<wenlan_core::db::Space>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let new_name = req.new_name.as_deref().unwrap_or(&name);
    let space = db
        .update_space(&name, new_name, req.description.as_deref())
        .await
        .map_err(ServerError::from)?;
    Ok(Json(space))
}

pub async fn handle_delete_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    db.delete_space(&name, "keep")
        .await
        .map_err(ServerError::from)?;
    Ok(Json(serde_json::json!({"deleted": name})))
}

pub async fn handle_move_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path((from, to)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    // `From<WenlanError>` keeps `Validation` (a refused merge) a 400, not a 500.
    let affected = db.reassign_memories_space(&from, &to).await?;
    Ok(Json(serde_json::json!({"affected": affected})))
}

pub(crate) fn register_extended(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/spaces/{name}/pin", post(handle_pin_space))
        .route("/api/spaces/{name}/confirm", post(handle_confirm_space))
        .route("/api/spaces/reorder", post(handle_reorder_space))
        .route("/api/spaces/{name}/star", post(handle_toggle_space_starred))
        .route("/api/spaces/{name}/shared/sync", post(handle_shared_sync))
        .route(
            "/api/spaces/{name}/shared",
            get(handle_shared_status).delete(handle_shared_leave),
        )
        .route("/api/pages/{id}/shared/sync", post(handle_page_shared_sync))
        .route(
            "/api/pages/{id}/shared",
            get(handle_page_shared_status).delete(handle_page_shared_leave),
        )
        .route("/api/library/shared/sync", post(handle_library_shared_sync))
        .route(
            "/api/library/shared",
            get(handle_library_shared_status).delete(handle_library_shared_leave),
        )
        .route("/api/shared", get(handle_shared_list))
        .route("/api/shared/folder", post(handle_shared_folder))
        .route(
            "/api/documents/{source_id}/space",
            post(handle_set_document_space),
        )
}

/// The name other members see for this person: the one asked for, else
/// the profile's.
async fn shared_member_name(
    db: &wenlan_core::db::MemoryDB,
    asked: Option<&str>,
) -> Result<String, ServerError> {
    if let Some(name) = asked.map(str::trim).filter(|name| !name.is_empty()) {
        return Ok(name.to_string());
    }
    if let Some(name) = wenlan_core::shared_space::person_name(db).await? {
        return Ok(name);
    }
    // No display name yet: the computer account's name becomes it, so the
    // person sees and changes it in Settings, where every share reads it.
    let Some(name) = computer_user_name() else {
        return Err(ServerError::ValidationError(
            "no name to show the others: set a display name in Settings".into(),
        ));
    };
    if let Some(profile) = db
        .get_profile()
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
    {
        db.update_profile(&profile.id, None, Some(&name), None, None, None)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;
    }
    Ok(name)
}

/// The full name of this computer's account, else the account name.
///
/// Each lookup simply fails where it does not apply, so one code path serves
/// every platform: `id -F` prints the full name on macOS (and errors on GNU
/// `id`), the passwd entry carries it on other Unix systems, and Windows has
/// neither, so it falls through to `USERNAME`.
fn computer_user_name() -> Option<String> {
    let clean = |name: String| {
        let name = name.trim().to_string();
        (!name.is_empty()).then_some(name)
    };
    if std::env::var_os("WENLAN_TEST_NO_COMPUTER_NAME").is_some() {
        return None;
    }
    if let Some(name) = std::process::Command::new("id")
        .arg("-F")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| clean(String::from_utf8_lossy(&out.stdout).into_owned()))
    {
        return Some(name);
    }
    let account = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .and_then(clean)?;
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|passwd| {
            passwd.lines().find_map(|line| {
                let fields: Vec<&str> = line.split(':').collect();
                (fields.first() == Some(&account.as_str()))
                    .then(|| fields.get(4).and_then(|gecos| gecos.split(',').next()))
                    .flatten()
                    .map(str::to_string)
            })
        })
        .and_then(clean)
        .or(Some(account))
}

fn shared_folder_path(folder: &str) -> Result<std::path::PathBuf, ServerError> {
    let path = std::path::PathBuf::from(folder.trim());
    if !path.is_absolute() {
        return Err(ServerError::ValidationError(format!(
            "folder must be an absolute path (got {folder:?})"
        )));
    }
    Ok(path)
}

async fn shared_sync(
    state: &Arc<RwLock<ServerState>>,
    scope: wenlan_core::shared_space::Scope,
    req: wenlan_types::shared_space::SharedSyncRequest,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    let (db, page_root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let settings = wenlan_core::shared_space::settings_path();
    if let Some(host) = req.host {
        return host_sync(&db, scope, host, &settings, page_root.as_deref()).await;
    }
    let folder = shared_folder_path(&req.folder)?;
    let member_name = shared_member_name(&db, req.member_name.as_deref()).await?;
    let scope = match scope {
        wenlan_core::shared_space::Scope::Space(name) => wenlan_core::shared_space::Scope::Space(
            db.registered_space_or_none(Some(&name))
                .await
                .map_err(|e| ServerError::Internal(e.to_string()))?
                .ok_or_else(|| ServerError::NotFound(format!("no Space named {name:?}")))?,
        ),
        page => page,
    };
    let member_id = req
        .member_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let counts = wenlan_core::shared_space::sync_space(
        &db,
        wenlan_core::shared_space::SyncTarget {
            scope: &scope,
            folder: &folder,
            member_name: &member_name,
            settings_path: &settings,
            knowledge_path: page_root.as_deref(),
            member_id,
            host: None,
        },
    )
    .await?;
    Ok(Json(counts))
}

/// A Space synced through a team server: the server says who the key
/// belongs to (the member id and name) and whether they may use the Space;
/// this Wenlan keeps its copy of the server's folder under the data root.
/// The Space is made here when there is none with that name yet.
async fn host_sync(
    db: &wenlan_core::db::MemoryDB,
    scope: wenlan_core::shared_space::Scope,
    host: wenlan_types::shared_space::SharedHost,
    settings: &std::path::Path,
    page_root: Option<&std::path::Path>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    use wenlan_core::shared_space::{remote, HostLink, Scope};
    let Scope::Space(name) = scope else {
        return Err(ServerError::ValidationError(
            "a team server shares whole Spaces, not single pages".into(),
        ));
    };
    let (url, key) = crate::team_routes::join_parts(&host.code, host.key.as_deref())?;
    let me = remote::me(&url, &key)
        .await
        .map_err(ServerError::ValidationError)?;
    let remote_space = host
        .space
        .as_deref()
        .map(str::trim)
        .filter(|space| !space.is_empty())
        .unwrap_or(name.trim())
        .to_string();
    if !me.spaces.iter().any(|entry| entry.space == remote_space) {
        return Err(ServerError::ValidationError(format!(
            "the team server does not let {} use {remote_space:?}",
            me.name
        )));
    }
    let space = match db
        .registered_space_or_none(Some(&name))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
    {
        Some(space) => space,
        None => {
            db.create_space(name.trim(), None, false)
                .await
                .map_err(|e| ServerError::Internal(e.to_string()))?;
            db.registered_space_or_none(Some(&name))
                .await
                .map_err(|e| ServerError::Internal(e.to_string()))?
                .ok_or_else(|| ServerError::NotFound(format!("no Space named {name:?}")))?
        }
    };
    let folder = remote::mirror_folder(settings, &url, &remote_space);
    let scope = Scope::Space(space);
    let counts = wenlan_core::shared_space::sync_space(
        db,
        wenlan_core::shared_space::SyncTarget {
            scope: &scope,
            folder: &folder,
            member_name: &me.name,
            settings_path: settings,
            knowledge_path: page_root,
            member_id: Some(&me.member_id),
            host: Some(HostLink {
                url,
                key,
                space: remote_space,
            }),
        },
    )
    .await?;
    Ok(Json(counts))
}

async fn space_scope(
    state: &Arc<RwLock<ServerState>>,
    name: &str,
) -> Result<wenlan_core::shared_space::Scope, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let space = db
        .registered_space_or_none(Some(name))
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("no Space named {name:?}")))?;
    Ok(wenlan_core::shared_space::Scope::Space(space))
}

fn shared_status(
    scope: &wenlan_core::shared_space::Scope,
) -> Result<Json<wenlan_types::shared_space::SharedSyncStatus>, ServerError> {
    let settings = wenlan_core::shared_space::settings_path();
    wenlan_core::shared_space::share_status_at(&settings, scope)?
        .map(Json)
        .ok_or_else(|| ServerError::NotFound("not shared from here".into()))
}

fn shared_leave(
    scope: &wenlan_core::shared_space::Scope,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    let settings = wenlan_core::shared_space::settings_path();
    if !wenlan_core::shared_space::leave_at(&settings, scope)? {
        return Err(ServerError::NotFound("not shared from here".into()));
    }
    Ok(Json(wenlan_types::responses::SuccessResponse { ok: true }))
}

/// POST /api/spaces/{name}/shared/sync — one sync pass of the Space's pages
/// with its shared folder (`wenlan_core::shared_space`). The first call
/// adds this person to the folder's `space.json` and remembers the folder.
/// Which pages are shared is decided by each page's own Space, never by a
/// request header. Returns counts only.
pub async fn handle_shared_sync(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
    Json(req): Json<wenlan_types::shared_space::SharedSyncRequest>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    shared_sync(&state, wenlan_core::shared_space::Scope::Space(name), req).await
}

/// GET /api/spaces/{name}/shared — how this Wenlan shares the Space (its
/// folder, member names) and the last sync pass since the daemon started:
/// when, the counts, and why it is paused if it is. 404 when the Space is
/// not shared from here.
pub async fn handle_shared_status(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncStatus>, ServerError> {
    shared_status(&space_scope(&state, &name).await?)
}

/// DELETE /api/spaces/{name}/shared — stops sharing the Space from here.
/// The pages here and everything in the folder stay as they are.
pub async fn handle_shared_leave(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    shared_leave(&space_scope(&state, &name).await?)
}

/// POST /api/pages/{id}/shared/sync — one sync pass of one page shared
/// alone. The page is here, or it is the page the folder shares (joining
/// it: it is made here with no Space). Returns counts only.
pub async fn handle_page_shared_sync(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(id): Path<String>,
    Json(req): Json<wenlan_types::shared_space::SharedSyncRequest>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    shared_sync(&state, wenlan_core::shared_space::Scope::Page(id), req).await
}

/// GET /api/pages/{id}/shared — how this Wenlan shares one page alone.
pub async fn handle_page_shared_status(
    Path(id): Path<String>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncStatus>, ServerError> {
    shared_status(&wenlan_core::shared_space::Scope::Page(id))
}

/// DELETE /api/pages/{id}/shared — stops sharing one page from here.
pub async fn handle_page_shared_leave(
    Path(id): Path<String>,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    shared_leave(&wenlan_core::shared_space::Scope::Page(id))
}

/// POST /api/library/shared/sync — one sync pass of the whole library with
/// its folder, between this person's own computers. The first call makes
/// the folder's `space.json` (or joins it as the member id in the request,
/// found in that file). Pages of a Space or a page that has its own share
/// stay in that share. Returns counts only.
pub async fn handle_library_shared_sync(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<wenlan_types::shared_space::SharedSyncRequest>,
) -> Result<Json<wenlan_types::shared_space::SharedSyncCounts>, ServerError> {
    shared_sync(&state, wenlan_core::shared_space::Scope::Library, req).await
}

/// GET /api/library/shared — how this Wenlan shares the library.
pub async fn handle_library_shared_status(
) -> Result<Json<wenlan_types::shared_space::SharedSyncStatus>, ServerError> {
    shared_status(&wenlan_core::shared_space::Scope::Library)
}

/// DELETE /api/library/shared — stops sharing the library from here; the
/// pages and the folder stay as they are.
pub async fn handle_library_shared_leave(
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    shared_leave(&wenlan_core::shared_space::Scope::Library)
}

/// GET /api/shared — every Space and page this Wenlan shares, with their
/// member names and last pass. No page text.
pub async fn handle_shared_list(
) -> Result<Json<wenlan_types::shared_space::SharedListResponse>, ServerError> {
    let settings = wenlan_core::shared_space::settings_path();
    Ok(Json(wenlan_types::shared_space::SharedListResponse {
        shares: wenlan_core::shared_space::share_statuses_at(&settings)?,
    }))
}

/// POST /api/shared/folder — what a folder shares (a Space or one page,
/// its name and member names), before joining it. `shared` is null when
/// the folder shares nothing yet.
pub async fn handle_shared_folder(
    Json(req): Json<wenlan_types::shared_space::SharedFolderRequest>,
) -> Result<Json<wenlan_types::shared_space::SharedFolderResponse>, ServerError> {
    let folder = shared_folder_path(&req.folder)?;
    let shared =
        tokio::task::spawn_blocking(move || wenlan_core::shared_space::peek_folder(&folder))
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))??;
    Ok(Json(wenlan_types::shared_space::SharedFolderResponse {
        shared,
    }))
}

// =====================================================================
// Batch 4 — Space CRUD
// =====================================================================

/// POST /api/spaces/{name}/pin — toggle space pinned (starred) state
pub async fn handle_pin_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    // pin_space maps to toggle_space_starred in the DB
    db.toggle_space_starred(&name)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(wenlan_types::responses::SuccessResponse { ok: true }))
}

/// POST /api/spaces/{name}/confirm
pub async fn handle_confirm_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    db.confirm_space(&name)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(wenlan_types::responses::SuccessResponse { ok: true }))
}

/// POST /api/spaces/reorder
pub async fn handle_reorder_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<wenlan_types::requests::ReorderSpaceRequest>,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    db.reorder_space(&req.name, req.new_order)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(wenlan_types::responses::SuccessResponse { ok: true }))
}

/// POST /api/spaces/{name}/star — toggle starred state
pub async fn handle_toggle_space_starred(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let starred = db
        .toggle_space_starred(&name)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "starred": starred })))
}

/// POST /api/documents/{source_id}/space — assign a document to a space (domain)
pub async fn handle_set_document_space(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(source_id): Path<String>,
    Json(req): Json<wenlan_types::requests::SetDocumentSpaceRequest>,
) -> Result<Json<wenlan_types::responses::SuccessResponse>, ServerError> {
    let db = {
        let s = state.read().await;
        s.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let requested_space = Some(req.space_name);
    let registered_space =
        registered_request_space(&db, &requested_space, "set_document_space").await?;
    db.update_memory_space_opt(&source_id, registered_space.as_deref())
        .await
        .map_err(ServerError::from)?;
    Ok(Json(wenlan_types::responses::SuccessResponse { ok: true }))
}

#[cfg(test)]
mod not_found_mapping_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::state::ServerState;

    async fn build_state_with_db() -> (
        std::sync::Arc<tokio::sync::RwLock<ServerState>>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().expect("failed to create tempdir");
        let emitter: std::sync::Arc<dyn wenlan_core::events::EventEmitter> =
            std::sync::Arc::new(wenlan_core::events::NoopEmitter);
        let db = wenlan_core::db::MemoryDB::new(tmp.path(), emitter)
            .await
            .expect("MemoryDB::new should succeed");
        let server_state = ServerState {
            db: Some(std::sync::Arc::new(db)),
            ..Default::default()
        };
        (
            std::sync::Arc::new(tokio::sync::RwLock::new(server_state)),
            tmp,
        )
    }

    /// Same #586 pattern as the blocked-agent store handler: `delete_space`
    /// returns `WenlanError::NotFound`, which maps to `ServerError::NotFound`
    /// (404). Forcing it through `ServerError::Internal` would hide a normal
    /// "no such space" refusal behind a fake daemon fault (500).
    #[tokio::test]
    async fn delete_missing_space_is_not_found_not_internal_error() {
        let (state, _tmp) = build_state_with_db().await;
        let app = crate::router::build_router(state.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/spaces/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(resp.into_body(), 1_048_576)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("parse error body");
        let message = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .expect("error body must carry a message");
        assert!(
            message.contains("not found"),
            "error message must say the space was not found, got: {message}"
        );
    }
}
