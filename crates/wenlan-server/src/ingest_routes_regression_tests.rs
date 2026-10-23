// SPDX-License-Identifier: Apache-2.0
use super::*;
use sha2::{Digest, Sha256};
use std::path::Path;
use wenlan_core::config::Config;
use wenlan_types::sources::{Source, SourceType, SyncStatus};

struct DataDirGuard {
    previous: Option<std::ffi::OsString>,
}

impl DataDirGuard {
    fn new(path: &Path) -> Self {
        let previous = std::env::var_os("WENLAN_DATA_DIR");
        std::env::set_var("WENLAN_DATA_DIR", path);
        Self { previous }
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

fn directory_source(id: &str, path: &Path, status: SyncStatus) -> Source {
    Source {
        id: id.to_string(),
        source_type: SourceType::Directory,
        path: path.to_path_buf(),
        status,
        last_sync: None,
        file_count: 0,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: None,
        queued_files: 0,
        waiting_files: 0,
    }
}

fn write_config(sources: Vec<Source>, knowledge_path: &Path) {
    wenlan_core::config::save_config(&Config {
        sources,
        knowledge_path: Some(knowledge_path.to_path_buf()),
        ..Config::default()
    })
    .unwrap();
}

async fn test_db(root: &Path) -> Arc<wenlan_core::db::MemoryDB> {
    Arc::new(
        wenlan_core::db::MemoryDB::new(root, Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    )
}

fn request(source_id: &str) -> IngestTextRequest {
    IngestTextRequest {
        source: "local_files".to_string(),
        source_id: source_id.to_string(),
        title: "Desktop snapshot".to_string(),
        content: "This desktop snapshot should be ignored in favor of the registered source scan."
            .to_string(),
        url: Some(format!("file://{source_id}")),
        metadata: None,
    }
}

async fn fixture(
    root: &Path,
    sources: Vec<Source>,
) -> (Arc<RwLock<ServerState>>, Arc<wenlan_core::db::MemoryDB>) {
    let db = test_db(&root.join("db")).await;
    write_config(sources, &root.join("pages"));
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }));
    (state, db)
}

#[tokio::test]
async fn active_local_files_request_uses_registered_directory_sync() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let file_path = source_root.join("note.txt");
    std::fs::write(
        &file_path,
        "Registered directory content that should be queued for ingestion.",
    )
    .unwrap();
    let other_file = source_root.join("other.txt");
    std::fs::write(
        &other_file,
        "Another eligible document must remain outside this single-file request.",
    )
    .unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory-1",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;

    let Json(response) =
        handle_ingest_text(State(state), Json(request(&file_path.to_string_lossy())))
            .await
            .unwrap();

    assert_eq!(
        response.document_id,
        wenlan_core::sources::directory::document_source_id(
            "directory-1",
            &file_path,
            Some(&root.path().join("pages")),
        )
    );
    assert_eq!(response.chunks_created, 0);
    assert_eq!(response.space, None);
    let queued = db
        .get_queue_entry("directory-1", &file_path.to_string_lossy())
        .await
        .unwrap()
        .expect("the canonical directory file should be queued");
    let expected_hash = format!("{:x}", Sha256::digest(std::fs::read(&file_path).unwrap()));
    assert_eq!(queued.content_hash.as_deref(), Some(expected_hash.as_str()));
    assert!(db
        .get_queue_entry("local_files", &file_path.to_string_lossy())
        .await
        .unwrap()
        .is_none());
    assert!(!db
        .source_id_exists(&file_path.to_string_lossy())
        .await
        .unwrap());
    assert!(db
        .get_queue_entry("directory-1", &other_file.to_string_lossy())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn short_registered_directory_note_rejects_without_queue_or_rows() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let file_path = source_root.join("note.txt");
    std::fs::write(&file_path, "Hello, world!").unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "short-note-directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;

    let result =
        handle_ingest_text(State(state), Json(request(&file_path.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("cannot be indexed"))
    );
    assert!(db
        .get_queue_entry("short-note-directory", &file_path.to_string_lossy())
        .await
        .unwrap()
        .is_none());
    assert!(!db
        .source_id_exists(&file_path.to_string_lossy())
        .await
        .unwrap());
}

#[tokio::test]
async fn paused_directory_owner_rejects_without_mutation() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let file_path = source_root.join("note.txt");
    std::fs::write(
        &file_path,
        "A paused directory file must not be directly ingested.",
    )
    .unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "paused-directory",
            &source_root,
            SyncStatus::Paused,
        )],
    )
    .await;

    let result =
        handle_ingest_text(State(state), Json(request(&file_path.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("not active"))
    );
    assert!(db
        .get_queue_entry("paused-directory", &file_path.to_string_lossy())
        .await
        .unwrap()
        .is_none());
    assert!(!db
        .source_id_exists(&file_path.to_string_lossy())
        .await
        .unwrap());
}

#[tokio::test]
async fn ambiguous_directory_owners_reject_without_mutation() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let outer = root.path().join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let file_path = inner.join("note.txt");
    std::fs::write(
        &file_path,
        "A file inside overlapping registered roots is ambiguous.",
    )
    .unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![
            directory_source("outer", &outer, SyncStatus::Active),
            directory_source("inner", &inner, SyncStatus::Active),
        ],
    )
    .await;

    let result =
        handle_ingest_text(State(state), Json(request(&file_path.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("multiple"))
    );
    assert!(!db
        .source_id_exists(&file_path.to_string_lossy())
        .await
        .unwrap());
}

#[tokio::test]
async fn owned_missing_file_rejects_without_mutation() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let missing = source_root.join("missing.txt");
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;

    let result = handle_ingest_text(State(state), Json(request(&missing.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("does not exist"))
    );
    assert!(!db
        .source_id_exists(&missing.to_string_lossy())
        .await
        .unwrap());
}

#[tokio::test]
async fn missing_registered_single_file_root_rejects_without_raw_fallback() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let missing_file = root.path().join("registered-single-file.txt");
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "missing-file-source",
            &missing_file,
            SyncStatus::Active,
        )],
    )
    .await;

    let result =
        handle_ingest_text(State(state), Json(request(&missing_file.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("missing or unreadable"))
    );
    assert!(!db
        .source_id_exists(&missing_file.to_string_lossy())
        .await
        .unwrap());
}

#[tokio::test]
async fn parent_component_path_rejects_before_raw_fallback() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let outside = root.path().join("outside.txt");
    std::fs::write(
        &outside,
        "A traversing path must not create a bare local row.",
    )
    .unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;
    let traversal = source_root.join("..").join("outside.txt");

    let result =
        handle_ingest_text(State(state), Json(request(&traversal.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("traversal"))
    );
    assert!(!db
        .source_id_exists(&outside.to_string_lossy())
        .await
        .unwrap());
    assert!(!db
        .source_id_exists(&traversal.to_string_lossy())
        .await
        .unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_rejects_without_mutation() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let outside = root.path().join("outside.txt");
    std::fs::write(
        &outside,
        "A symlink must not escape its registered directory.",
    )
    .unwrap();
    let link = source_root.join("escape.txt");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;

    let result = handle_ingest_text(State(state), Json(request(&link.to_string_lossy()))).await;

    assert!(
        matches!(result, Err(ServerError::ValidationError(message)) if message.contains("outside"))
    );
    assert!(!db.source_id_exists(&link.to_string_lossy()).await.unwrap());
}

#[tokio::test]
async fn prefix_sibling_and_non_local_requests_keep_raw_ingest_behavior() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    let sibling = root.path().join("watched-sibling");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let sibling_file = sibling.join("note.txt");
    std::fs::write(
        &sibling_file,
        "A prefix sibling path is unrelated to the registered root.",
    )
    .unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;

    let Json(sibling_response) = handle_ingest_text(
        State(state.clone()),
        Json(request(&sibling_file.to_string_lossy())),
    )
    .await
    .unwrap();
    assert_eq!(sibling_response.document_id, sibling_file.to_string_lossy());
    assert!(db
        .source_id_exists(&sibling_file.to_string_lossy())
        .await
        .unwrap());

    let mut other_source = request(&sibling_file.to_string_lossy());
    other_source.source = "manual_capture".to_string();
    other_source.source_id = "unchanged-non-local-id".to_string();
    let Json(other_response) = handle_ingest_text(State(state), Json(other_source))
        .await
        .unwrap();
    assert_eq!(other_response.document_id, "unchanged-non-local-id");
    assert!(db.source_id_exists("unchanged-non-local-id").await.unwrap());
}

#[tokio::test]
async fn unsupported_directory_extension_keeps_legacy_raw_ingest() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let file_path = source_root.join("snippet.rs");
    std::fs::write(&file_path, "fn main() {}\n").unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;

    let Json(response) =
        handle_ingest_text(State(state), Json(request(&file_path.to_string_lossy())))
            .await
            .expect("unsupported Directory extensions retain the released raw-ingest behavior");

    assert_eq!(response.document_id, file_path.to_string_lossy());
    assert!(db
        .source_id_exists(&file_path.to_string_lossy())
        .await
        .unwrap());
    assert!(db
        .get_queue_entry("directory", &file_path.to_string_lossy())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn same_mtime_content_change_enqueues_new_actual_hash() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = tempfile::tempdir().unwrap();
    let _env = DataDirGuard::new(root.path());
    let source_root = root.path().join("watched");
    std::fs::create_dir_all(&source_root).unwrap();
    let file_path = source_root.join("note.txt");
    std::fs::write(
        &file_path,
        "Initial on-disk bytes are queued with their content hash.",
    )
    .unwrap();
    let (state, db) = fixture(
        root.path(),
        vec![directory_source(
            "directory",
            &source_root,
            SyncStatus::Active,
        )],
    )
    .await;
    let _ = handle_ingest_text(
        State(state.clone()),
        Json(request(&file_path.to_string_lossy())),
    )
    .await
    .unwrap();
    let original_mtime = std::fs::metadata(&file_path).unwrap().modified().unwrap();
    let first_hash = db
        .get_queue_entry("directory", &file_path.to_string_lossy())
        .await
        .unwrap()
        .unwrap()
        .content_hash
        .unwrap();

    let changed_content = "Changed on-disk bytes with the exact previous timestamp are queued too.";
    std::fs::write(&file_path, changed_content).unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&file_path)
        .unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    let second_hash = format!("{:x}", Sha256::digest(changed_content.as_bytes()));
    let _ = handle_ingest_text(State(state), Json(request(&file_path.to_string_lossy())))
        .await
        .unwrap();

    let queued_hash = db
        .get_queue_entry("directory", &file_path.to_string_lossy())
        .await
        .unwrap()
        .unwrap()
        .content_hash
        .unwrap();
    assert_ne!(first_hash, second_hash);
    assert_eq!(queued_hash, second_hash);
}
