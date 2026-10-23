// SPDX-License-Identifier: Apache-2.0
//! Knowledge directory inspection endpoints.

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::SharedState;
use axum::response::Json;
use wenlan_types::responses::{KnowledgeCountResponse, KnowledgePathResponse};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/knowledge/path", get(handle_get_knowledge_path))
        .route("/api/knowledge/count", get(handle_get_knowledge_count))
        .route(
            "/api/knowledge/folders",
            get(handle_list_knowledge_folders).post(handle_create_knowledge_folder),
        )
        .route("/api/attachments", post(handle_attach_file))
}

/// POST /api/attachments
///
/// Copy an image or video on this computer into the pages folder's
/// `_attachments/` and return the Markdown line that shows it. Reads only the
/// named file, and only when its bytes are real media.
pub async fn handle_attach_file(
    axum::extract::State(state): axum::extract::State<SharedState>,
    Json(req): Json<wenlan_types::requests::AttachFileRequest>,
) -> Result<Json<wenlan_types::responses::AttachFileResponse>, ServerError> {
    let source = std::path::PathBuf::from(req.source_path.trim());
    if !source.is_absolute() {
        return Err(ServerError::ValidationError(
            "attachment_path_not_absolute".into(),
        ));
    }
    let root = {
        let s = state.read().await;
        s.lint_config.page_root().map(std::path::Path::to_path_buf)
    }
    .unwrap_or_else(|| wenlan_core::config::load_config().knowledge_path_or_default());
    let stored = tokio::task::spawn_blocking(move || {
        wenlan_core::attachments::store_attachment(&root, &source)
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))??;
    Ok(Json(wenlan_types::responses::AttachFileResponse {
        markdown: stored.markdown(req.alt.as_deref()),
        path: stored.path,
        absolute_path: stored.absolute_path.to_string_lossy().to_string(),
        kind: stored.kind.as_str().to_string(),
        bytes: stored.bytes,
        created: stored.created,
    }))
}

/// GET /api/knowledge/path
pub async fn handle_get_knowledge_path() -> Result<Json<KnowledgePathResponse>, ServerError> {
    let cfg = wenlan_core::config::load_config();
    let path = cfg.knowledge_path_or_default();
    Ok(Json(KnowledgePathResponse {
        path: path.to_string_lossy().to_string(),
    }))
}

/// GET /api/knowledge/count
pub async fn handle_get_knowledge_count() -> Result<Json<KnowledgeCountResponse>, ServerError> {
    let cfg = wenlan_core::config::load_config();
    let path = cfg.knowledge_path_or_default();
    if !path.exists() {
        return Ok(Json(KnowledgeCountResponse { count: 0 }));
    }
    let count = std::fs::read_dir(&path)
        .map_err(|e| ServerError::Internal(e.to_string()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
        })
        .count();
    Ok(Json(KnowledgeCountResponse {
        count: count as u64,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_knowledge_count_returns_ok() {
        let result = handle_get_knowledge_count().await;
        assert!(result.is_ok());
    }

    #[test]
    fn count_md_files_in_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.md"), "x").unwrap();
        std::fs::write(dir.path().join("b.md"), "y").unwrap();
        std::fs::write(dir.path().join("c.txt"), "z").unwrap();

        let count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 2);
    }
}

/// Global browsing lists real directories. Scoped browsing derives only authorized ancestors.
pub async fn handle_list_knowledge_folders(
    axum::extract::State(state): axum::extract::State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    view: crate::truth_guard::TruthView,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<wenlan_types::responses::KnowledgeFoldersResponse>, ServerError> {
    use wenlan_types::responses::{KnowledgeFolderEntry, KnowledgeFoldersResponse};
    let (db, root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let scope = crate::read_scope::effective_read_scope(
        &db,
        params.get("space").map(String::as_str),
        header_space.as_deref(),
    )
    .await?;
    let Some(root) = root else {
        return Ok(Json(KnowledgeFoldersResponse {
            folders: vec![],
            truncated: false,
        }));
    };
    let folders = if scope == wenlan_core::read_scope::ReadScope::Global {
        tokio::task::spawn_blocking(move || {
            wenlan_core::export::knowledge::list_knowledge_folders(&root)
        })
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))??
        .into_iter()
        .map(|f| KnowledgeFolderEntry {
            path: f.path,
            parent_path: f.parent_path,
            name: f.name,
        })
        .collect()
    } else {
        let pages = db.list_pages_scoped("active", i64::MAX, 0, &scope).await?;
        let pages = wenlan_core::truth_adapter::filter_pages(&db, &view.grant, pages).await?;
        let ids: Vec<String> = pages
            .into_iter()
            .filter(wenlan_core::pages::is_active_file_page)
            .map(|p| p.id)
            .collect();
        let paths = tokio::task::spawn_blocking(move || {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            wenlan_core::export::knowledge::KnowledgeWriter::new(root, &db)
                .live_page_filenames(&ids)
        })
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;
        let mut folders = std::collections::BTreeMap::new();
        for path in paths.values() {
            let components: Vec<&str> = path.split('/').collect();
            for depth in 1..components.len() {
                let path = components[..depth].join("/");
                folders.insert(
                    path.clone(),
                    KnowledgeFolderEntry {
                        path,
                        parent_path: components[..depth - 1].join("/"),
                        name: components[depth - 1].into(),
                    },
                );
            }
        }
        folders.into_values().collect()
    };
    Ok(Json(KnowledgeFoldersResponse {
        folders,
        truncated: false,
    }))
}

pub async fn handle_create_knowledge_folder(
    axum::extract::State(state): axum::extract::State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<wenlan_types::requests::CreateKnowledgeFolderRequest>,
) -> Result<Json<wenlan_types::responses::CreateKnowledgeFolderResponse>, ServerError> {
    let (db, root) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.lint_config.page_root().map(std::path::Path::to_path_buf),
        )
    };
    let scope = crate::read_scope::effective_read_scope(&db, None, header_space.as_deref()).await?;
    if scope != wenlan_core::read_scope::ReadScope::Global {
        return Err(ServerError::ValidationError(
            "Scoped callers cannot create unowned empty Wiki folders".into(),
        ));
    }
    let root = root
        .ok_or_else(|| ServerError::ValidationError("Wiki projection is not configured".into()))?;
    let projection_write = db.begin_page_projection_write();
    let path = tokio::task::spawn_blocking(move || {
        let _projection_write = projection_write;
        wenlan_core::export::knowledge::create_knowledge_folder(&root, &req.parent_path, &req.name)
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))??;
    Ok(Json(
        wenlan_types::responses::CreateKnowledgeFolderResponse { path },
    ))
}

#[cfg(test)]
#[path = "folder_routes_test.rs"]
mod folder_route_tests;
