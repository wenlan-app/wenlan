// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as RoutePath, State};
use tokio::sync::RwLock;
use wenlan_core::config::Config;
use wenlan_core::db::MemoryDB;
use wenlan_core::sources::{Source, SourceType, SyncStatus};

use crate::source_routes::tests::{loaded_source_status, new_test_db, DataDirGuard};
use crate::source_routes::{handle_sync_source, SyncStatsResponse};
use crate::state::ServerState;

const SOURCE_ID: &str = "obsidian-vault";

async fn data_dir_lock() -> tokio::sync::MutexGuard<'static, ()> {
    crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// A note with enough prose to pass the quality gate.
fn note(topic: &str) -> String {
    format!(
        "# {topic}\n\n{topic} is a note kept in the test vault. It explains one idea in plain \
         sentences so the sync keeps it. A second sentence adds detail about how {topic} \
         changes over time and why it matters to the person who wrote it.\n"
    )
}

/// A note long enough to split into several chunks, one per section.
fn long_note(topic: &str) -> String {
    let mut text = format!("# {topic}\n\n");
    for section in 0..4 {
        text.push_str(&format!("## Section {section}\n\n"));
        for line in 0..12 {
            text.push_str(&format!(
                "Section {section} line {line} of {topic} describes the vault sync in concrete \
                 detail, so the chunker has real prose to split into separate sections.\n"
            ));
        }
        text.push('\n');
    }
    text
}

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    // Push the mtime forward so a rewrite inside the same clock tick is
    // still seen as a change by the mtime fast path.
    let bump = std::time::SystemTime::now() + std::time::Duration::from_secs(next_tick());
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(bump)
        .unwrap();
    path
}

fn next_tick() -> u64 {
    static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn register_vault(root: &Path) -> Source {
    register_source(root, SourceType::Obsidian)
}

fn register_source(root: &Path, source_type: SourceType) -> Source {
    let source = Source {
        id: SOURCE_ID.to_string(),
        source_type,
        path: root.to_path_buf(),
        status: SyncStatus::Active,
        last_sync: None,
        file_count: 0,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: None,
        queued_files: 0,
        waiting_files: 0,
    };
    wenlan_core::config::save_config(&Config {
        sources: vec![source.clone()],
        ..Config::default()
    })
    .unwrap();
    source
}

fn loaded_memory_count() -> u64 {
    wenlan_core::config::load_config()
        .sources
        .into_iter()
        .find(|s| s.id == SOURCE_ID)
        .expect("source in config")
        .memory_count
}

fn server_state(db: &Arc<MemoryDB>) -> Arc<RwLock<ServerState>> {
    Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }))
}

async fn sync(db: &Arc<MemoryDB>) -> SyncStatsResponse {
    handle_sync_source(State(server_state(db)), RoutePath(SOURCE_ID.to_string()))
        .await
        .expect("vault sync succeeds")
        .0
}

/// Every stored chunk document id of the vault, sorted.
async fn stored_ids(db: &MemoryDB) -> Vec<String> {
    let mut ids = db
        .list_source_ids_with_prefix("memory", &format!("{SOURCE_ID}::"))
        .await
        .unwrap();
    ids.sort();
    ids
}

fn chunk_id(path: &Path, index: usize) -> String {
    format!("{SOURCE_ID}::{}::{index}", path.to_string_lossy())
}

async fn note_text(db: &MemoryDB, path: &Path) -> String {
    let mut text = String::new();
    for id in stored_ids(db).await {
        if id.starts_with(&format!("{SOURCE_ID}::{}::", path.to_string_lossy())) {
            for memory in db.get_memories_by_source_id("memory", &id).await.unwrap() {
                text.push_str(&memory.content);
            }
        }
    }
    text
}

#[tokio::test]
async fn vault_sync_follows_edits_new_notes_renames_and_deletions() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let (db, _db_dir) = new_test_db().await;
    let vault = tempfile::tempdir().unwrap();
    let alpha = write(vault.path(), "alpha.md", &note("Alpha"));
    let beta = write(vault.path(), "notes/beta.md", &note("Beta"));
    register_vault(vault.path());

    let first = sync(&db).await;
    assert_eq!(first.files_found, 2);
    assert_eq!(
        stored_ids(&db).await,
        vec![chunk_id(&alpha, 0), chunk_id(&beta, 0)]
    );

    // An unchanged vault does no work.
    let again = sync(&db).await;
    assert_eq!(again.ingested, 0);
    assert_eq!(again.skipped, 2);

    // Edit: the new text replaces the old one.
    write(vault.path(), "alpha.md", &note("Aleph"));
    let edited = sync(&db).await;
    assert_eq!(edited.ingested, 1);
    let alpha_text = note_text(&db, &alpha).await;
    assert!(alpha_text.contains("Aleph"), "{alpha_text}");
    assert!(!alpha_text.contains("Alpha"), "{alpha_text}");
    assert_eq!(stored_ids(&db).await.len(), 2);
    // The count shown for the vault is what it holds, not every edit so far.
    assert_eq!(loaded_memory_count(), 2);

    // New note.
    let gamma = write(vault.path(), "gamma.md", &note("Gamma"));
    sync(&db).await;
    assert!(stored_ids(&db).await.contains(&chunk_id(&gamma, 0)));

    // Rename: stored once, at the new path only.
    let renamed = vault.path().join("notes/beta-renamed.md");
    std::fs::rename(&beta, &renamed).unwrap();
    sync(&db).await;
    let ids = stored_ids(&db).await;
    assert!(ids.contains(&chunk_id(&renamed, 0)), "{ids:?}");
    assert!(!ids.contains(&chunk_id(&beta, 0)), "{ids:?}");
    assert_eq!(ids.len(), 3);

    // Deletion.
    std::fs::remove_file(&gamma).unwrap();
    sync(&db).await;
    let ids = stored_ids(&db).await;
    assert!(!ids.contains(&chunk_id(&gamma, 0)), "{ids:?}");
    assert_eq!(ids.len(), 2);
    let tracked = db.list_sync_state_paths(SOURCE_ID).await.unwrap();
    assert!(!tracked.contains(&gamma.to_string_lossy().to_string()));
}

#[tokio::test]
async fn shorter_note_leaves_no_stale_chunks() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let (db, _db_dir) = new_test_db().await;
    let vault = tempfile::tempdir().unwrap();
    let path = write(vault.path(), "long.md", &long_note("Delta"));
    register_vault(vault.path());

    sync(&db).await;
    let before = stored_ids(&db).await;
    assert!(before.len() > 1, "long note should split: {before:?}");

    write(vault.path(), "long.md", &note("Delta"));
    sync(&db).await;
    assert_eq!(stored_ids(&db).await, vec![chunk_id(&path, 0)]);
}

#[tokio::test]
async fn note_rewritten_to_boilerplate_drops_its_old_text() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let (db, _db_dir) = new_test_db().await;
    let vault = tempfile::tempdir().unwrap();
    let path = write(vault.path(), "draft.md", &note("Epsilon"));
    write(vault.path(), "keep.md", &note("Zeta"));
    register_vault(vault.path());

    sync(&db).await;
    assert!(stored_ids(&db).await.contains(&chunk_id(&path, 0)));

    write(vault.path(), "draft.md", "todo\n");
    sync(&db).await;
    let ids = stored_ids(&db).await;
    assert!(!ids.contains(&chunk_id(&path, 0)), "{ids:?}");
    assert_eq!(ids.len(), 1);
    // Not marked synced, so a later daemon reads it again.
    let tracked = db.list_sync_state_paths(SOURCE_ID).await.unwrap();
    assert!(!tracked.contains(&path.to_string_lossy().to_string()));
}

#[tokio::test]
async fn missing_vault_root_is_unavailable_and_deletes_nothing() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let (db, _db_dir) = new_test_db().await;
    let parent = tempfile::tempdir().unwrap();
    let vault = parent.path().join("vault");
    write(&vault, "alpha.md", &note("Alpha"));
    register_vault(&vault);

    sync(&db).await;
    let before = stored_ids(&db).await;
    assert_eq!(before.len(), 1);

    let moved = parent.path().join("vault-away");
    std::fs::rename(&vault, &moved).unwrap();
    let stats = sync(&db).await;
    assert_eq!(stats.files_found, 0);
    assert!(matches!(
        loaded_source_status(SOURCE_ID),
        SyncStatus::Unavailable(_)
    ));
    assert_eq!(stored_ids(&db).await, before);

    std::fs::rename(&moved, &vault).unwrap();
    sync(&db).await;
    assert!(matches!(
        loaded_source_status(SOURCE_ID),
        SyncStatus::Active
    ));
    assert_eq!(stored_ids(&db).await, before);
}

#[tokio::test]
async fn logseq_graph_sync_reads_pages_as_prose_and_follows_changes() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let (db, _db_dir) = new_test_db().await;
    let graph = tempfile::tempdir().unwrap();
    write(graph.path(), "logseq/config.edn", "{}");
    // Logseq's own backup copy must not be stored a second time.
    write(
        graph.path(),
        "logseq/bak/pages/atlas/2026-10-01.md",
        &outline("Atlas backup"),
    );
    let atlas = write(graph.path(), "pages/projects___atlas.md", &outline("Atlas"));
    let journal = write(graph.path(), "journals/2026_10_21.md", &outline("Today"));
    write(graph.path(), "journals/2026_10_22.md", "-\n");
    register_source(graph.path(), SourceType::Logseq);

    let first = sync(&db).await;
    assert_eq!(first.files_found, 3);
    assert_eq!(
        stored_ids(&db).await,
        vec![chunk_id(&journal, 0), chunk_id(&atlas, 0)]
    );
    assert_eq!(
        db.get_memory_source_agent(&chunk_id(&atlas, 0))
            .await
            .unwrap()
            .as_deref(),
        Some("logseq")
    );
    let text = note_text(&db, &atlas).await;
    assert!(text.contains("Atlas is a page kept"), "{text}");
    assert!(!text.contains("collapsed::"), "{text}");
    assert!(!text.contains("- Atlas is"), "{text}");

    // Edit, then delete: the graph follows both.
    write(graph.path(), "pages/projects___atlas.md", &outline("Aleph"));
    sync(&db).await;
    let text = note_text(&db, &atlas).await;
    assert!(text.contains("Aleph") && !text.contains("Atlas"), "{text}");
    std::fs::remove_file(&journal).unwrap();
    sync(&db).await;
    assert_eq!(stored_ids(&db).await, vec![chunk_id(&atlas, 0)]);
}

/// A Logseq page with enough prose to pass the quality gate.
fn outline(topic: &str) -> String {
    format!(
        "tags:: notes\n\n- {topic} is a page kept in the test graph. It explains one idea in \
         plain sentences so the sync keeps it.\n  collapsed:: true\n\t- A second sentence adds \
         detail about how {topic} changes over time and why it matters to the person who \
         wrote it.\n"
    )
}

#[tokio::test]
async fn background_pass_stores_a_bounded_number_of_notes() {
    use super::{try_sync_obsidian_source, OBSIDIAN_POLL_NOTE_LIMIT};

    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let (db, _db_dir) = new_test_db().await;
    let vault = tempfile::tempdir().unwrap();
    let total = OBSIDIAN_POLL_NOTE_LIMIT + 3;
    for i in 0..total {
        write(
            vault.path(),
            &format!("note-{i:03}.md"),
            &note(&format!("Topic{i}")),
        );
    }
    let source = register_vault(vault.path());
    let gate = ServerState::default().quality_gate;

    try_sync_obsidian_source(db.clone(), &gate, &source, OBSIDIAN_POLL_NOTE_LIMIT)
        .await
        .expect("no other sync is running")
        .expect("first pass succeeds");
    assert_eq!(stored_ids(&db).await.len(), OBSIDIAN_POLL_NOTE_LIMIT);

    try_sync_obsidian_source(db.clone(), &gate, &source, OBSIDIAN_POLL_NOTE_LIMIT)
        .await
        .expect("no other sync is running")
        .expect("second pass succeeds");
    assert_eq!(stored_ids(&db).await.len(), total);
}

/// Wenlan's pages folder moved into the vault: its pages and source stubs
/// are Wenlan's own output, never read back as the person's notes.
#[tokio::test]
async fn a_pages_folder_inside_the_vault_is_not_read_as_notes() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let vault = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(vault.path().join(".obsidian")).unwrap();
    let mine = write(vault.path(), "mine.md", &note("Mine"));
    let pages = vault.path().join("Wenlan");
    write(
        &pages,
        "_sources/mem_1.md",
        "---\ntype: source\ntitle: \"mem_1\"\norigin_stub: \"mem_1\"\n---\n\nA source stub.\n",
    );
    write(&pages, "page.md", &note("Page"));
    wenlan_core::config::save_config(&Config {
        knowledge_path: Some(pages.clone()),
        ..Config::default()
    })
    .unwrap();

    assert_eq!(
        super::scan_notes(&SourceType::Obsidian, vault.path()),
        vec![mine]
    );
}
