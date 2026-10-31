// SPDX-License-Identifier: Apache-2.0
use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};
use axum::{extract::Path, extract::State, response::Json};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
use wenlan_types::import::{
    ActiveImportBatchesResponse, ImportBatchStatus, ImportChatExportRequest,
    ImportChatExportResponse,
};
use wenlan_types::requests::ImportMemoriesRequest;
use wenlan_types::responses::ImportMemoriesResponse;
use wenlan_types::WriteSpaceSource;

/// Default cap for `GET /api/import/batches/active`.
pub(crate) const DEFAULT_ACTIVE_IMPORT_BATCH_LIMIT: usize = 20;

pub(crate) async fn request_import_priority(
    state: &Arc<RwLock<ServerState>>,
    db: &Arc<wenlan_core::db::MemoryDB>,
    batch_id: Option<&str>,
) {
    let write_signal = {
        let guard = state.read().await;
        guard.write_signal.clone()
    };
    if let Err(error) = write_signal.persist_and_request_import(db, batch_id).await {
        // The import is already durable and searchable. Keep the successful
        // response, but make a failed wake/deadline write visible; the normal
        // scheduler poll can still pick up the same backlog.
        tracing::warn!("[import] failed to persist priority wake: {error}");
    }
}

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/import/memories", post(handle_import_memories))
        .route("/api/import/chat-export", post(handle_chat_export_import))
        .route("/api/import/state", get(handle_list_pending_imports))
        .route(
            "/api/import/batches/active",
            get(handle_active_import_batches),
        )
        .route(
            "/api/import/batches/{batch_id}/status",
            get(handle_import_batch_status),
        )
}

/// POST /api/import/memories
pub async fn handle_import_memories(
    State(state): State<Arc<RwLock<ServerState>>>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Json(req): Json<ImportMemoriesRequest>,
) -> Result<Json<ImportMemoriesResponse>, ServerError> {
    let valid_sources = ["chatgpt", "claude", "other"];
    if !valid_sources.contains(&req.source.as_str()) {
        return Err(ServerError::ValidationError(format!(
            "Invalid source '{}'. Must be one of: chatgpt, claude, other",
            req.source
        )));
    }

    let (db, confidence_cfg) = {
        let s = state.read().await;
        (
            s.db.clone().ok_or(ServerError::DbNotInitialized)?,
            s.tuning.confidence.clone(),
        )
    };
    let _space_write_guard = db.lock_space_writes().await;
    let resolved = db
        .resolve_write_space(&req.space, header_space.as_deref())
        .await?;
    let result = wenlan_core::importer::import_memories_no_llm_in_batch(
        &db,
        &req.content,
        &req.source,
        req.label.as_deref(),
        &confidence_cfg,
        &resolved,
        req.batch_id.as_deref(),
        req.chunk_index,
    )
    .await?;
    let persisted = db.finalize_write_space(&resolved).await?;
    let space_source = if persisted.space_name.is_some() {
        persisted.source
    } else {
        WriteSpaceSource::Uncategorized
    };
    drop(_space_write_guard);
    if result.imported > 0 {
        request_import_priority(&state, &db, Some(&result.batch_id)).await;
    }

    Ok(Json(ImportMemoriesResponse {
        imported: result.imported,
        skipped: result.skipped,
        breakdown: result.breakdown,
        entities_created: result.entities_created,
        observations_added: result.observations_added,
        relations_created: result.relations_created,
        batch_id: result.batch_id,
        space: persisted.space_name,
        space_source: Some(space_source),
    }))
}

// ---------------------------------------------------------------------------
// POST /api/import/chat-export
// ---------------------------------------------------------------------------

/// Maximum ZIP file size we accept (512 MB).
const MAX_IMPORT_ZIP_SIZE: u64 = 512 * 1024 * 1024;

/// Serializes chat-export ingest across the daemon. The per-conversation dedup
/// reads committed rows once, before the import writes anything, so a retry
/// that overlaps a still-running import of the same export (the client timed
/// out, the daemon did not) would store every conversation twice. Holding
/// this lock makes the retry wait for the running import, then dedup against
/// what it committed.
static CHAT_EXPORT_INGEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// POST /api/import/chat-export
///
/// Reads a chat-export ZIP from disk, auto-detects the vendor, bulk-ingests
/// all new conversations as raw memories. Automatic enrichment is admitted
/// later by the global ambient scheduler.
pub async fn handle_chat_export_import(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(req): Json<ImportChatExportRequest>,
) -> Result<Json<ImportChatExportResponse>, ServerError> {
    // 1. Validate file size before reading (I4).
    let meta = tokio::fs::metadata(&req.path)
        .await
        .map_err(|e| ServerError::ChatImport(format!("failed to stat {}: {e}", req.path)))?;
    if meta.len() > MAX_IMPORT_ZIP_SIZE {
        return Err(ServerError::ChatImport(format!(
            "file too large ({} bytes, max {})",
            meta.len(),
            MAX_IMPORT_ZIP_SIZE
        )));
    }

    // 2. Read ZIP from disk (async I/O to avoid blocking Tokio worker — I1).
    let bytes = tokio::fs::read(&req.path)
        .await
        .map_err(|e| ServerError::ChatImport(format!("failed to read {}: {e}", req.path)))?;

    // 3. Dispatch parse — auto-detect vendor.
    let batch = wenlan_core::chat_import::dispatch_parse(&bytes)
        .map_err(|e| ServerError::ChatImport(format!("parse failed: {e}")))?;

    // 4. Snapshot Arc<MemoryDB> and the maintenance coordinator together, out
    //    of the RwLock guard before any awaits.
    let (db, maintenance) = {
        let guard = state.read().await;
        (
            guard
                .db
                .as_ref()
                .ok_or_else(|| ServerError::ChatImport("database not initialized".into()))?
                .clone(),
            guard.maintenance_coordinator.clone(),
        )
    };

    // The import is a detached writer on the shared connection. Take the
    // immediate background fence before any writer starts, so startup
    // recovery, a pending or leased repair, or live analysis refuses the
    // import instead of racing it. Never wait for the fence here: the request
    // may already be running inside a window that holds it.
    let maintenance_guard = maintenance
        .try_begin_background()
        .ok_or_else(|| ServerError::Conflict("repair_write_fence_conflict".to_string()))?;

    // 5-7 run in a spawned task that the handler only awaits. The ingest opens
    // transactions on the shared writer connection; dropping the request
    // future (client timeout or disconnect) must not abandon one part way or
    // leave import_state short of Done/Error, so the import always finishes.
    // The id is chosen here so a task that panics can still end its row. That
    // cleanup runs in a second spawned task that owns the ingest's outcome, so
    // it also happens when the request was dropped before the panic. The
    // supervisor owns the fence lease too, so a repair stays shut out until
    // the ingest and that cleanup have both finished, even with no request
    // left to hold it.
    let import_id = format!("imp_{}", Uuid::new_v4());
    let ingest = tokio::spawn(ingest_chat_export(
        state,
        db.clone(),
        batch,
        req.path,
        import_id.clone(),
    ));
    tokio::spawn(async move {
        let _maintenance_guard = maintenance_guard;
        match ingest.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                fail_unfinished_import(&db, &import_id, &error.to_string()).await;
                Err(error)
            }
            Err(join_error) => {
                let msg = format!("import task failed: {join_error}");
                fail_unfinished_import(&db, &import_id, &msg).await;
                Err(ServerError::Internal(format!(
                    "chat import task {import_id} failed: {join_error}"
                )))
            }
        }
    })
    .await
    .map_err(|e| ServerError::Internal(format!("chat import task: {e}")))?
}

/// Best-effort: end an import whose task died before it reached Done/Error,
/// so `GET /api/import/state` does not report it pending forever. The task is
/// gone, so nothing else writes this row; a missing row (the task failed
/// before creating it) or a row that already reached a terminal stage is left
/// alone, and a failed write is only logged (startup ends leftover rows).
async fn fail_unfinished_import(db: &wenlan_core::db::MemoryDB, import_id: &str, msg: &str) {
    use wenlan_core::chat_import::bulk_ingest::ImportStage;
    match db.load_import_state(import_id).await {
        Ok(Some(import)) if !matches!(import.stage, ImportStage::Done | ImportStage::Error) => {
            if let Err(error) = db
                .update_import_state_stage_with_error(
                    import_id,
                    ImportStage::Error,
                    None,
                    None,
                    Some(msg),
                )
                .await
            {
                tracing::warn!("[import] failed to mark {import_id} as error: {error}");
            }
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("[import] failed to load {import_id}: {error}"),
    }
}

/// Test-only fault: panic inside the ingest task, once its import_state row
/// is in StageA and it holds the ingest lock, for imports of an armed archive
/// path. Keyed by path so a
/// fault cannot fire in another test's import running in parallel.
#[cfg(test)]
static PANIC_IN_INGEST_PATHS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Fault the ordinary Result error path after the import row exists.
#[cfg(test)]
static ERROR_BEFORE_STAGE_A_PATHS: std::sync::Mutex<Vec<String>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn panic_in_ingest_if_armed(path: &str) {
    let armed = PANIC_IN_INGEST_PATHS
        .lock()
        .map(|paths| paths.iter().any(|armed| armed == path))
        .unwrap_or(false);
    if armed {
        panic!("injected chat import task panic");
    }
}

async fn ingest_chat_export(
    state: Arc<RwLock<ServerState>>,
    db: Arc<wenlan_core::db::MemoryDB>,
    batch: wenlan_core::chat_import::ParsedConversationBatch,
    path: String,
    import_id: String,
) -> Result<Json<ImportChatExportResponse>, ServerError> {
    // 5. Create import_state row.
    db.start_import_state(&import_id, batch.vendor, &path)
        .await
        .map_err(|e| ServerError::ChatImport(format!("start_import_state: {e}")))?;
    #[cfg(test)]
    if ERROR_BEFORE_STAGE_A_PATHS.lock().unwrap().contains(&path) {
        return Err(ServerError::ChatImport(format!(
            "injected stage transition failure: {import_id}"
        )));
    }
    db.update_import_state_stage(
        &import_id,
        wenlan_core::chat_import::bulk_ingest::ImportStage::StageA,
        Some(batch.conversations.len() as i64),
        Some(0),
    )
    .await
    .map_err(|e| ServerError::ChatImport(format!("update_import_state_stage: {e}")))?;

    // 6. Bulk ingest conversations. On failure, mark import_state as Error (I3).
    //    The dedup inside runs only once this import holds the single-flight
    //    lock, so an overlapping retry dedups against this import's commits.
    let _ingest_guard = CHAT_EXPORT_INGEST_LOCK.lock().await;
    #[cfg(test)]
    panic_in_ingest_if_armed(&path);
    // TODO: Replace NoopEmitter with a broadcast-channel emitter that forwards
    // progress events to /ws/updates so the Tauri app can display live progress.
    let emitter: Arc<dyn wenlan_core::events::EventEmitter> =
        Arc::new(wenlan_core::events::NoopEmitter);
    // Artifacts and canvases are kept as originals under the data root,
    // with their pages written to the pages folder like any other page.
    let data_root = wenlan_core::config::data_root();
    let page_root = state
        .read()
        .await
        .lint_config
        .page_root()
        .map(std::path::Path::to_path_buf);
    let result =
        match wenlan_core::chat_import::bulk_ingest::bulk_import_conversations_keeping_originals(
            db.clone(),
            &batch.conversations,
            emitter,
            &import_id,
            wenlan_core::chat_import::bulk_ingest::OriginalsTarget {
                data_root: &data_root,
                knowledge_path: page_root.as_deref(),
            },
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("bulk ingest: {e}");
                let _ = db
                    .update_import_state_stage_with_error(
                        &import_id,
                        wenlan_core::chat_import::bulk_ingest::ImportStage::Error,
                        None,
                        None,
                        Some(&msg),
                    )
                    .await;
                return Err(ServerError::ChatImport(msg));
            }
        };

    // 7. The import is complete once every slice is committed and embedded, so
    //    it is keyword- and vector-searchable without a restart. Automatic
    //    classification/extraction remains a global ambient backlog represented
    //    by NULL fields plus versioned enrichment receipts; import_state does
    //    not attempt to track that unrelated, potentially multi-day work.
    db.update_import_state_stage(
        &import_id,
        wenlan_core::chat_import::bulk_ingest::ImportStage::Done,
        None,
        Some(result.conversations_ingested as i64),
    )
    .await
    .map_err(|e| ServerError::ChatImport(format!("update_import_state_stage: {e}")))?;
    drop(_ingest_guard);

    if result.memories_stored > 0 {
        request_import_priority(&state, &db, None).await;
    }

    let vendor_str = batch.vendor.as_str().to_string();
    Ok(Json(ImportChatExportResponse {
        import_id,
        vendor: vendor_str,
        conversations_total: batch.conversations.len(),
        conversations_new: result.conversations_ingested,
        conversations_skipped_existing: result.conversations_skipped_existing,
        memories_stored: result.memories_stored,
        originals_kept: result.originals_kept,
    }))
}

// ---------------------------------------------------------------------------
// GET /api/import/state
// ---------------------------------------------------------------------------

/// GET /api/import/state — list non-terminal imports (pending/in-progress).
pub async fn handle_list_pending_imports(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<Vec<wenlan_types::import::PendingImport>>, ServerError> {
    let db = {
        let guard = state.read().await;
        guard
            .db
            .as_ref()
            .ok_or_else(|| ServerError::ChatImport("database not initialized".into()))?
            .clone()
    };
    let rows = db
        .list_pending_imports()
        .await
        .map_err(|e| ServerError::ChatImport(format!("list_pending_imports: {e}")))?;
    Ok(Json(
        rows.into_iter()
            .map(|s| wenlan_types::import::PendingImport {
                id: s.id,
                vendor: s.vendor.as_str().to_string(),
                stage: s.stage.as_str().to_string(),
                source_path: s.source_path,
                processed_conversations: s.processed_conversations,
                total_conversations: s.total_conversations,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/import/batches/{batch_id}/status
// ---------------------------------------------------------------------------

/// GET /api/import/batches/{batch_id}/status — live progress for one import
/// batch. 404 when no memory carries the batch's source-id prefix.
///
/// Like `GET /api/import/state`, this takes no Space selector: a batch is
/// addressed by its unguessable id and the response carries the batch's own
/// `space`, so there is nothing to scope the request by.
pub async fn handle_import_batch_status(
    State(state): State<Arc<RwLock<ServerState>>>,
    Path(batch_id): Path<String>,
) -> Result<Json<ImportBatchStatus>, ServerError> {
    let db = {
        let guard = state.read().await;
        guard
            .db
            .as_ref()
            .ok_or(ServerError::DbNotInitialized)?
            .clone()
    };
    db.import_batch_status(&batch_id)
        .await?
        .map(Json)
        .ok_or_else(|| ServerError::NotFound(format!("unknown import batch '{batch_id}'")))
}

// ---------------------------------------------------------------------------
// GET /api/import/batches/active
// ---------------------------------------------------------------------------

/// GET /api/import/batches/active — most recently updated incomplete import
/// batches, newest first. Unscoped for the same reason as the status route.
pub async fn handle_active_import_batches(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<ActiveImportBatchesResponse>, ServerError> {
    let db = {
        let guard = state.read().await;
        guard
            .db
            .as_ref()
            .ok_or(ServerError::DbNotInitialized)?
            .clone()
    };
    let batches = db
        .active_import_batches(DEFAULT_ACTIVE_IMPORT_BATCH_LIMIT)
        .await?;
    Ok(Json(ActiveImportBatchesResponse { batches }))
}

#[cfg(test)]
mod chat_export_route_tests {
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    use crate::state::ServerState;
    use std::sync::Arc;
    use tokio::sync::{Notify, RwLock};

    struct BlockingProvider {
        calls: AtomicUsize,
        called: Notify,
        release: Notify,
    }

    #[async_trait]
    impl wenlan_core::llm_provider::LlmProvider for BlockingProvider {
        async fn generate(
            &self,
            _request: wenlan_core::llm_provider::LlmRequest,
        ) -> Result<String, wenlan_core::llm_provider::LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.called.notify_one();
            self.release.notified().await;
            Ok("{}".to_string())
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "import-handoff-test"
        }

        fn backend(&self) -> wenlan_core::llm_provider::LlmBackend {
            wenlan_core::llm_provider::LlmBackend::Api
        }

        fn kind(&self) -> &'static str {
            "mock"
        }
    }

    fn write_claude_export(path: &std::path::Path) {
        const CONVERSATIONS: &str = r#"[
          {
            "uuid": "route-conv-1",
            "name": "Scheduler handoff",
            "summary": "A short summary",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:05:00.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {
                "uuid": "msg-1",
                "text": "How should enrichment run?",
                "content": [{"type": "text", "text": "How should enrichment run?"}],
                "sender": "human",
                "created_at": "2026-04-01T10:00:00.000Z",
                "updated_at": "2026-04-01T10:00:00.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
              },
              {
                "uuid": "msg-2",
                "text": "Only through the ambient scheduler.",
                "content": [{"type": "text", "text": "Only through the ambient scheduler."}],
                "sender": "assistant",
                "created_at": "2026-04-01T10:00:05.000Z",
                "updated_at": "2026-04-01T10:00:05.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "msg-1"
              }
            ]
          }
        ]"#;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("users.json", options).unwrap();
        zip.write_all(b"[]").unwrap();
        zip.start_file("conversations.json", options).unwrap();
        zip.write_all(CONVERSATIONS.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    #[tokio::test]
    async fn post_chat_export_with_missing_file_returns_400() {
        let state = Arc::new(RwLock::new(ServerState::default()));
        let app = crate::router::build_router(state);
        let body = serde_json::json!({ "path": "/tmp/definitely-not-a-real-zip-xyz.zip" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/import/chat-export")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 400);
    }

    #[tokio::test]
    async fn chat_import_finishes_ingest_without_starting_enrichment() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("claude-export.zip");
        write_claude_export(&archive_path);
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(
                &dir.path().join("db"),
                Arc::new(wenlan_core::events::NoopEmitter),
            )
            .await
            .unwrap(),
        );
        let provider = Arc::new(BlockingProvider {
            calls: AtomicUsize::new(0),
            called: Notify::new(),
            release: Notify::new(),
        });
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            llm: Some(provider.clone()),
            ..Default::default()
        }));
        // Real startup finishes recovery before the routes serve.
        state.read().await.maintenance_coordinator.finish_recovery();

        let response = super::handle_chat_export_import(
            axum::extract::State(state),
            axum::Json(wenlan_types::import::ImportChatExportRequest {
                path: archive_path.to_string_lossy().into_owned(),
            }),
        )
        .await
        .unwrap()
        .0;

        let import = db
            .load_import_state(&response.import_id)
            .await
            .unwrap()
            .expect("import state");
        assert_eq!(
            import.stage,
            wenlan_core::chat_import::bulk_ingest::ImportStage::Done
        );
        assert!(db.list_pending_imports().await.unwrap().is_empty());
        // The HTTP result is vector-searchable at once: rows exist, and none of
        // them waits for the startup NULL-embedding recovery.
        let source_id = "import_claude_route-conv-1";
        let rows = db
            .get_memories_by_source_id("memory", source_id)
            .await
            .unwrap();
        assert_eq!(response.memories_stored, 1);
        assert_eq!(rows.len(), response.memories_stored);
        assert_eq!(
            db.count_unembedded_chunks("memory", source_id)
                .await
                .unwrap(),
            0,
            "chat-export rows must be embedded at write time"
        );
        assert!(db.get_classification_candidate(3).await.unwrap().is_some());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                provider.called.notified()
            )
            .await
            .is_err(),
            "chat import must leave enrichment to the ambient scheduler"
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    /// A Claude export whose one conversation made an HTML artifact.
    fn write_claude_export_with_artifact(path: &std::path::Path) {
        const CONVERSATIONS: &str = r#"[
          {
            "uuid": "route-conv-art",
            "name": "Launch site",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:05:00.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {"uuid": "m1", "text": "make a landing page",
               "content": [{"type": "text", "text": "make a landing page"}],
               "sender": "human", "created_at": "2026-04-01T10:00:00.000Z",
               "parent_message_uuid": "00000000-0000-4000-8000-000000000000"},
              {"uuid": "m2", "text": "", "sender": "assistant", "created_at": "2026-04-01T10:00:05.000Z",
               "content": [
                 {"type": "text", "text": "Here it is."},
                 {"type": "tool_use", "name": "artifacts",
                  "input": {"id": "landing", "type": "text/html", "title": "Landing",
                            "command": "create", "content": "<h1>Launch</h1>"}}
               ],
               "parent_message_uuid": "m1"}
            ]
          }
        ]"#;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("users.json", options).unwrap();
        zip.write_all(b"[]").unwrap();
        zip.start_file("conversations.json", options).unwrap();
        zip.write_all(CONVERSATIONS.as_bytes()).unwrap();
        zip.finish().unwrap();
    }

    /// Restores `WENLAN_DATA_DIR` on drop.
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
    async fn chat_import_keeps_artifacts_as_originals_and_the_page_names_its_chat() {
        let _lock = crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        std::fs::create_dir_all(&data_root).unwrap();
        let _env = ScratchDataDir(std::env::var_os("WENLAN_DATA_DIR"));
        std::env::set_var("WENLAN_DATA_DIR", &data_root);

        let archive_path = dir.path().join("claude-export.zip");
        write_claude_export_with_artifact(&archive_path);
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

        let response = chat_export_request(&state, &archive_path).await.unwrap().0;
        assert_eq!(response.memories_stored, 1);
        assert_eq!(response.originals_kept, 1);
        assert_eq!(
            std::fs::read_to_string(data_root.join("originals/claude/route-conv-art/landing.html"))
                .unwrap(),
            "<h1>Launch</h1>"
        );
        let record = db
            .originals_for_conversation("claude", "route-conv-art")
            .await
            .unwrap()
            .remove(0);

        let app = crate::router::build_router(state);
        let request = |uri: String| {
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("x-agent-name", "original-route-contract")
                .body(Body::empty())
                .unwrap()
        };
        let res = app
            .clone()
            .oneshot(request(format!("/api/pages/{}/original", record.page_id)))
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let original: wenlan_types::originals::PageOriginalResponse =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(original.vendor, "claude");
        assert_eq!(original.conversation_id, "route-conv-art");
        assert_eq!(
            original.conversation_url,
            "https://claude.ai/chat/route-conv-art"
        );
        assert_eq!(
            original.file_path,
            "originals/claude/route-conv-art/landing.html"
        );

        // A page not made from an original, and a missing page, read as absent.
        let plain = wenlan_core::post_write::create_page(
            &db,
            wenlan_types::requests::CreateConceptRequest {
                title: "Plain".into(),
                content: "A plain note.".into(),
                summary: None,
                entity_id: None,
                space: Option::<String>::None.into(),
                source_memory_ids: vec![],
                creation_kind: Some("authored".into()),
                workspace: None,
            },
            "test",
            None,
        )
        .await
        .unwrap();
        for id in [plain.id.as_str(), "page_does_not_exist"] {
            let res = app
                .clone()
                .oneshot(request(format!("/api/pages/{id}/original")))
                .await
                .unwrap();
            assert_eq!(res.status(), 404, "{id}");
        }
    }

    async fn chat_export_state() -> (
        Arc<RwLock<ServerState>>,
        Arc<wenlan_core::db::MemoryDB>,
        std::path::PathBuf,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("claude-export.zip");
        write_claude_export(&archive_path);
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
        // Real startup finishes recovery before the routes serve.
        state.read().await.maintenance_coordinator.finish_recovery();
        (state, db, archive_path, dir)
    }

    type ChatExportResult = Result<
        axum::Json<wenlan_types::import::ImportChatExportResponse>,
        crate::error::ServerError,
    >;

    fn chat_export_request(
        state: &Arc<RwLock<ServerState>>,
        archive_path: &std::path::Path,
    ) -> impl std::future::Future<Output = ChatExportResult> + Send + 'static {
        super::handle_chat_export_import(
            axum::extract::State(state.clone()),
            axum::Json(wenlan_types::import::ImportChatExportRequest {
                path: archive_path.to_string_lossy().into_owned(),
            }),
        )
    }

    /// Wait (bounded) until `db` shows `count` imports queued or running.
    async fn wait_for_pending_imports(db: &wenlan_core::db::MemoryDB, count: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while db.list_pending_imports().await.unwrap().len() < count {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("imports never reached the ingest lock");
    }

    #[tokio::test]
    async fn chat_import_overlapping_retry_stores_each_conversation_once() {
        let (state, db, archive_path, _dir) = chat_export_state().await;

        // Hold the ingest lock so both requests are in flight at once, the way
        // a client retry overlaps a timed-out import that is still running.
        let held = super::CHAT_EXPORT_INGEST_LOCK.lock().await;
        let first = tokio::spawn(chat_export_request(&state, &archive_path));
        wait_for_pending_imports(&db, 1).await;
        let retry = tokio::spawn(chat_export_request(&state, &archive_path));
        wait_for_pending_imports(&db, 2).await;
        drop(held);

        let first = first.await.unwrap().unwrap().0;
        let retry = retry.await.unwrap().unwrap().0;
        // Whichever request takes the lock first ingests; the other dedups.
        assert_eq!(first.conversations_new + retry.conversations_new, 1);
        assert_eq!(
            first.conversations_skipped_existing + retry.conversations_skipped_existing,
            1,
            "the overlapping request must dedup against the committed import"
        );
        let source_id = "import_claude_route-conv-1";
        let rows = db
            .get_memories_by_source_id("memory", source_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "overlapping imports must not duplicate rows");
        assert_eq!(
            db.count_unembedded_chunks("memory", source_id)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn chat_import_finishes_after_the_request_is_dropped() {
        let (state, db, archive_path, _dir) = chat_export_state().await;

        // Pin the import before its ingest, then drop the request future the
        // way a client timeout or disconnect does.
        let held = super::CHAT_EXPORT_INGEST_LOCK.lock().await;
        let mut request = Box::pin(chat_export_request(&state, &archive_path));
        let import_id = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let poll =
                    tokio::time::timeout(std::time::Duration::from_millis(10), request.as_mut())
                        .await;
                assert!(poll.is_err(), "request finished while the lock was held");
                if let Some(import) = db.list_pending_imports().await.unwrap().pop() {
                    break import.id;
                }
            }
        })
        .await
        .expect("import never started");
        drop(request);
        drop(held);

        let finished = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                let import = db.load_import_state(&import_id).await.unwrap().unwrap();
                use wenlan_core::chat_import::bulk_ingest::ImportStage;
                if matches!(import.stage, ImportStage::Done | ImportStage::Error) {
                    break import.stage;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a dropped request must not abandon the import");
        assert_eq!(
            finished,
            wenlan_core::chat_import::bulk_ingest::ImportStage::Done
        );
        let source_id = "import_claude_route-conv-1";
        let rows = db
            .get_memories_by_source_id("memory", source_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            db.count_unembedded_chunks("memory", source_id)
                .await
                .unwrap(),
            0
        );

        // The writer connection is not left inside a transaction: a later
        // import of the same export runs and dedups.
        let again = chat_export_request(&state, &archive_path).await.unwrap().0;
        assert_eq!(again.conversations_skipped_existing, 1);
    }

    #[tokio::test]
    async fn chat_import_stage_transition_error_does_not_leave_a_pending_row() {
        use wenlan_core::chat_import::bulk_ingest::ImportStage;
        let (state, db, archive_path, _dir) = chat_export_state().await;
        let path = archive_path.to_string_lossy().into_owned();
        super::ERROR_BEFORE_STAGE_A_PATHS
            .lock()
            .unwrap()
            .push(path.clone());
        let result = chat_export_request(&state, &archive_path).await;
        super::ERROR_BEFORE_STAGE_A_PATHS
            .lock()
            .unwrap()
            .retain(|armed| armed != &path);
        let Err(crate::error::ServerError::ChatImport(message)) = result else {
            panic!("the injected stage transition must fail");
        };
        let import_id = message
            .strip_prefix("injected stage transition failure: ")
            .unwrap();
        let row = db
            .load_import_state(import_id)
            .await
            .unwrap()
            .expect("row was created before failure");
        assert_eq!(row.stage, ImportStage::Error);
        assert!(row
            .error_message
            .unwrap_or_default()
            .contains("injected stage transition failure"));
        assert!(db.list_pending_imports().await.unwrap().is_empty());
        assert!(db
            .get_memories_by_source_id("memory", "import_claude_route-conv-1")
            .await
            .unwrap()
            .is_empty());
        let retried = chat_export_request(&state, &archive_path).await.unwrap().0;
        assert_eq!(retried.memories_stored, 1);
    }

    #[tokio::test]
    async fn chat_import_task_panic_ends_the_import_as_error() {
        use wenlan_core::chat_import::bulk_ingest::ImportStage;
        let (state, db, archive_path, _dir) = chat_export_state().await;
        let path = archive_path.to_string_lossy().into_owned();

        super::PANIC_IN_INGEST_PATHS
            .lock()
            .unwrap()
            .push(path.clone());
        let request = Request::builder()
            .method("POST")
            .uri("/api/import/chat-export")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "path": path })).unwrap(),
            ))
            .unwrap();
        let response = crate::router::build_router(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        super::PANIC_IN_INGEST_PATHS
            .lock()
            .unwrap()
            .retain(|armed| armed != &path);

        assert_eq!(response.status(), 500);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let message = body["error"].as_str().unwrap_or_default().to_string();
        let import_id = message
            .split_whitespace()
            .find(|word| word.starts_with("imp_"))
            .unwrap_or_else(|| panic!("error names no import id: {message}"));
        let import = db
            .load_import_state(import_id)
            .await
            .unwrap()
            .expect("the task created its import row before it panicked");
        assert_eq!(import.stage, ImportStage::Error);
        let reason = import.error_message.unwrap_or_default();
        assert!(
            reason.starts_with("import task failed: "),
            "unexpected reason: {reason}"
        );
        assert!(
            db.list_pending_imports().await.unwrap().is_empty(),
            "a panicked import must not stay pending"
        );
    }

    #[tokio::test]
    async fn chat_import_task_panic_after_the_request_is_dropped_ends_the_import() {
        use wenlan_core::chat_import::bulk_ingest::ImportStage;
        let (state, db, archive_path, _dir) = chat_export_state().await;
        let path = archive_path.to_string_lossy().into_owned();
        super::PANIC_IN_INGEST_PATHS
            .lock()
            .unwrap()
            .push(path.clone());

        // Hold the ingest lock so the import is queued, drop the request, then
        // let the ingest reach its panic with nobody awaiting the response.
        let held = super::CHAT_EXPORT_INGEST_LOCK.lock().await;
        let mut request = Box::pin(chat_export_request(&state, &archive_path));
        let import_id = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let poll =
                    tokio::time::timeout(std::time::Duration::from_millis(10), request.as_mut())
                        .await;
                assert!(poll.is_err(), "request finished while the lock was held");
                if let Some(import) = db.list_pending_imports().await.unwrap().pop() {
                    break import.id;
                }
            }
        })
        .await
        .expect("import never started");
        drop(request);
        drop(held);

        let import = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let import = db.load_import_state(&import_id).await.unwrap().unwrap();
                if matches!(import.stage, ImportStage::Done | ImportStage::Error) {
                    break import;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a panicked import must not stay pending after its request is dropped");
        super::PANIC_IN_INGEST_PATHS
            .lock()
            .unwrap()
            .retain(|armed| armed != &path);
        assert_eq!(import.stage, ImportStage::Error);
        assert!(
            import
                .error_message
                .as_deref()
                .is_some_and(|reason| reason.starts_with("import task failed: ")),
            "unexpected reason: {:?}",
            import.error_message
        );
    }
    fn is_fence_conflict(result: &ChatExportResult) -> bool {
        matches!(
            result,
            Err(crate::error::ServerError::Conflict(code)) if code == "repair_write_fence_conflict"
        )
    }

    #[tokio::test]
    async fn chat_import_is_refused_before_any_write_while_the_maintenance_fence_is_closed() {
        let (state, db, archive_path, _dir) = chat_export_state().await;
        let source_id = "import_claude_route-conv-1";

        // Startup recovery not finished: the default coordinator is sealed.
        let sealed = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        assert!(is_fence_conflict(
            &chat_export_request(&sealed, &archive_path).await
        ));

        // Recovery finished, but a repair holds the fence.
        let maintenance = state.read().await.maintenance_coordinator.clone();
        let repair = maintenance
            .acquire_repair("repair_during_import", std::time::Duration::from_secs(1))
            .await
            .expect("an idle coordinator grants the repair");
        assert!(is_fence_conflict(
            &chat_export_request(&state, &archive_path).await
        ));

        assert!(db.list_pending_imports().await.unwrap().is_empty());
        assert!(db
            .get_memories_by_source_id("memory", source_id)
            .await
            .unwrap()
            .is_empty());

        // The refusal was the fence: once the repair ends, the same import runs.
        drop(repair);
        let response = chat_export_request(&state, &archive_path).await.unwrap().0;
        assert_eq!(response.memories_stored, 1);
    }

    #[tokio::test]
    async fn chat_import_holds_the_repair_fence_through_ingest_and_cleanup_after_the_request_is_dropped(
    ) {
        use wenlan_core::chat_import::bulk_ingest::ImportStage;
        let (state, db, archive_path, _dir) = chat_export_state().await;
        let maintenance = state.read().await.maintenance_coordinator.clone();
        let path = archive_path.to_string_lossy().into_owned();
        super::PANIC_IN_INGEST_PATHS
            .lock()
            .unwrap()
            .push(path.clone());

        // Block the ingest at its lock, then drop the request.
        let held = super::CHAT_EXPORT_INGEST_LOCK.lock().await;
        let mut request = Box::pin(chat_export_request(&state, &archive_path));
        let import_id = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let poll =
                    tokio::time::timeout(std::time::Duration::from_millis(10), request.as_mut())
                        .await;
                assert!(poll.is_err(), "request finished while the lock was held");
                if let Some(import) = db.list_pending_imports().await.unwrap().pop() {
                    break import.id;
                }
            }
        })
        .await
        .expect("import never started");
        drop(request);

        assert!(
            matches!(
                maintenance
                    .acquire_repair("repair_during_import", std::time::Duration::from_millis(50))
                    .await,
                Err(crate::maintenance_coordinator::MaintenanceFenceError::Busy)
            ),
            "a dropped request must not release the import's fence"
        );

        // Let the ingest reach its panic; the supervisor ends the row, then
        // releases the fence.
        drop(held);
        let repair = maintenance
            .acquire_repair("repair_during_import", std::time::Duration::from_secs(30))
            .await
            .expect("the fence opens once the import and its cleanup finish");
        let import = db.load_import_state(&import_id).await.unwrap().unwrap();
        super::PANIC_IN_INGEST_PATHS
            .lock()
            .unwrap()
            .retain(|armed| armed != &path);
        assert_eq!(
            import.stage,
            ImportStage::Error,
            "the repair must not start before the cleanup ended the import"
        );
        drop(repair);
    }
}

#[cfg(test)]
mod import_batch_status_route_tests {
    use axum::extract::{Path, State};
    use axum::Json;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use wenlan_types::import::ImportPhase;
    use wenlan_types::WriteSpaceTarget;

    use crate::error::ServerError;
    use crate::state::ServerState;

    async fn test_state() -> (Arc<RwLock<ServerState>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(
                &dir.path().join("db"),
                Arc::new(wenlan_core::events::NoopEmitter),
            )
            .await
            .unwrap(),
        );
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db),
            ..Default::default()
        }));
        (state, dir)
    }

    fn import_request(
        content: &str,
        batch_id: &str,
        chunk_index: u32,
    ) -> wenlan_types::requests::ImportMemoriesRequest {
        wenlan_types::requests::ImportMemoriesRequest {
            source: "other".to_string(),
            content: content.to_string(),
            label: None,
            space: WriteSpaceTarget::Inherit,
            batch_id: Some(batch_id.to_string()),
            chunk_index: Some(chunk_index),
            chunk_total: Some(2),
        }
    }

    async fn post_chunk(
        state: &Arc<RwLock<ServerState>>,
        content: &str,
        batch_id: &str,
        chunk_index: u32,
    ) -> wenlan_types::responses::ImportMemoriesResponse {
        super::handle_import_memories(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Json(import_request(content, batch_id, chunk_index)),
        )
        .await
        .expect("import chunk")
        .0
    }

    #[tokio::test]
    async fn chunked_import_chunks_share_one_batch_id() {
        let (state, _dir) = test_state().await;
        let batch = "test-batch-chunks";
        for (chunk, content) in [
            (
                0,
                "- chunk zero alpha memory content here\n- chunk zero beta memory content here",
            ),
            (
                1,
                "- chunk one gamma memory content here\n- chunk one delta memory content here",
            ),
        ] {
            let resp = post_chunk(&state, content, batch, chunk).await;
            assert_eq!(resp.batch_id, batch);
            assert_eq!(resp.imported, 2);
            assert_eq!(resp.skipped, 0);
        }

        let db = state.read().await.db.clone().unwrap();
        let status = db
            .import_batch_status(batch)
            .await
            .expect("batch status")
            .expect("batch exists");
        assert_eq!(status.batch_id, batch);
        assert_eq!(status.memories_imported, 4);
        assert_eq!(status.chunks_received, 2);
        assert_eq!(status.memories_skipped, 0);
        assert_eq!(status.source, "other");
        assert!(!status.complete);
        // Ingest and Store settle inside the request.
        for want in [ImportPhase::Ingest, ImportPhase::Store] {
            let phase = status
                .phases
                .iter()
                .find(|p| p.phase == want)
                .expect("phase");
            assert_eq!(phase.done, 4);
            assert_eq!(phase.total, 4);
        }
    }

    #[tokio::test]
    async fn batch_status_unknown_id_is_none_and_404() {
        let (state, _dir) = test_state().await;
        let db = state.read().await.db.clone().unwrap();
        assert!(db
            .import_batch_status("no-such-batch")
            .await
            .expect("db status")
            .is_none());
        let err =
            super::handle_import_batch_status(State(state), Path("no-such-batch".to_string()))
                .await
                .expect_err("unknown batch must 404");
        assert!(
            matches!(err, ServerError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn active_import_batches_excludes_a_settled_batch() {
        let (state, _dir) = test_state().await;
        post_chunk(
            &state,
            "- restless batch alpha memory content\n- restless batch beta memory content",
            "restless-batch",
            0,
        )
        .await;
        let settled = post_chunk(
            &state,
            "- settled batch alpha memory content\n- settled batch beta memory content",
            "settled-batch",
            0,
        )
        .await;
        assert_eq!(settled.imported, 2);

        // Settle every background step row for the settled batch.
        let db = state.read().await.db.clone().unwrap();
        for i in 0..settled.imported {
            // Chunked form: `import_{batch}_{chunk}_{i}` with chunk 0.
            let source_id = format!("import_settled-batch_0_{i}");
            for step in [
                "entity_extract",
                "entity_link",
                "title_enrich",
                "page_growth",
            ] {
                db.record_enrichment_step(&source_id, step, "ok", None)
                    .await
                    .expect("record step");
            }
        }

        assert!(
            !db.import_batch_status("settled-batch")
                .await
                .unwrap()
                .unwrap()
                .complete,
            "enrichment alone must not finish a pending page turn"
        );
        db.set_app_metadata("import_priority_until_v1", "0")
            .await
            .unwrap();
        let done = db
            .import_batch_status("settled-batch")
            .await
            .expect("db status")
            .expect("settled batch exists");
        assert!(done.complete);

        let active = super::handle_active_import_batches(State(state))
            .await
            .expect("active batches")
            .0;
        let ids: Vec<&str> = active.batches.iter().map(|b| b.batch_id.as_str()).collect();
        assert!(ids.contains(&"restless-batch"), "active: {ids:?}");
        assert!(!ids.contains(&"settled-batch"), "active: {ids:?}");
    }
}
