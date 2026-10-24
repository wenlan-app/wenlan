// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as RoutePath, State};
use tokio::sync::RwLock;
use wenlan_core::config::Config;
use wenlan_core::db::MemoryDB;
use wenlan_core::document_enrichment::source_page_id;
use wenlan_core::read_scope::ReadScope;
use wenlan_core::sources::directory::document_source_id;
use wenlan_core::sources::{Source, SourceType, SyncStatus};

use super::sync_okf_source_in_batches;
use crate::source_routes::tests::{loaded_source_status, mtime_ns, new_test_db, DataDirGuard};
use crate::source_routes::{handle_remove_source, handle_sync_source, SyncStatsResponse};
use crate::state::ServerState;

const SOURCE_ID: &str = "okf-wiki";

async fn data_dir_lock() -> tokio::sync::MutexGuard<'static, ()> {
    crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// A concept file with enough prose to pass the quality gate.
fn concept(title: &str, extra_frontmatter: &str, links: &str) -> String {
    format!(
        "---\ntype: concept\ntitle: {title}\ndescription: What {title} means in this bundle.\n\
         {extra_frontmatter}---\n\n# {title}\n\n{title} is a concept kept in the test bundle. \
         It explains one idea in plain sentences so the import keeps it. A second sentence \
         adds detail about how {title} behaves over time. {links}\n"
    )
}

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// Registers the bundle with pages written under `pages`, never the default
/// `~/.wenlan/pages`.
fn register_okf_source(root: &Path, space: Option<&str>, pages: &Path) -> Source {
    let source = Source {
        id: SOURCE_ID.to_string(),
        source_type: SourceType::Okf,
        path: root.to_path_buf(),
        status: SyncStatus::Active,
        last_sync: None,
        file_count: 0,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: space.map(str::to_string),
        queued_files: 0,
        waiting_files: 0,
    };
    wenlan_core::config::save_config(&Config {
        sources: vec![source.clone()],
        knowledge_path: Some(pages.to_path_buf()),
        ..Config::default()
    })
    .unwrap();
    source
}

fn loaded_source() -> Source {
    wenlan_core::config::load_config()
        .sources
        .into_iter()
        .find(|s| s.id == SOURCE_ID)
        .expect("source in config")
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
        .expect("OKF sync succeeds")
        .0
}

/// Run the document worker without a provider until nothing is claimable:
/// each queued concept is parsed, embedded, and gets its source page.
async fn drain(db: &Arc<MemoryDB>) -> usize {
    let prompts = wenlan_core::prompts::PromptRegistry::default();
    let knowledge_path = wenlan_core::config::load_config().knowledge_path_or_default();
    let mut processed = 0;
    while let Some(entry) = db.claim_next_pending_for_provider(false).await.unwrap() {
        wenlan_core::document_enrichment::run_document_enrichment_slice(
            db,
            &entry,
            Some(&knowledge_path),
            None,
            &prompts,
        )
        .await;
        processed += 1;
        assert!(processed < 50, "the worker must drain a small queue");
    }
    processed
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

async fn queued(db: &MemoryDB, path: &Path) -> bool {
    db.get_queue_entry(SOURCE_ID, &key(path))
        .await
        .unwrap()
        .is_some()
}

/// The page's outbound links as `(label, resolved target)`.
async fn outbound(db: &MemoryDB, page_id: &str) -> Vec<(String, Option<String>)> {
    let mut links: Vec<(String, Option<String>)> = db
        .get_page_outbound_links_scoped(page_id, &ReadScope::Global)
        .await
        .unwrap()
        .into_iter()
        .map(|link| (link.label, link.target_page_id))
        .collect();
    links.sort();
    links
}

/// The Space of every chunk of one concept file.
async fn chunk_spaces(db: &MemoryDB, path: &Path) -> Vec<Option<String>> {
    db.get_memories_by_source_ids(&[doc_id(path)])
        .await
        .unwrap()
        .into_iter()
        .map(|chunk| chunk.space)
        .collect()
}

fn doc_id(path: &Path) -> String {
    let knowledge_path = wenlan_core::config::load_config().knowledge_path_or_default();
    document_source_id(SOURCE_ID, path, Some(&knowledge_path))
}

#[tokio::test]
async fn okf_sync_queues_only_concepts_and_records_the_batch_position() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    write(root, "index.md", "---\nokf_version: \"0.2\"\n---\n# Wiki\n");
    write(root, "log.md", "# Log\n");
    write(root, "INSTRUCTIONS.md", "# Instructions\n");
    write(root, "concepts/index.md", "# Concepts\n");
    let alpha = write(root, "concepts/alpha.md", &concept("Alpha", "", ""));
    let beta = write(root, "concepts/beta.md", &concept("Beta", "", ""));
    write(root, "notes.txt", "A plain text note is not a concept.");
    write(
        root,
        "exported/.wenlan-okf-export.json",
        "{\"okf_version\":\"0.2\",\"generated_by\":\"wenlan/test\",\"files\":[]}",
    );
    let copied = write(root, "exported/pages/copied.md", &concept("Copied", "", ""));
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;

    let stats = sync(&db).await;

    assert_eq!(stats.files_found, 2, "only concept files count: {stats:?}");
    assert_eq!(stats.ingested, 2);
    assert_eq!(stats.queued_files, Some(2));
    assert_eq!(stats.waiting_files, Some(0));
    assert!(queued(&db, &alpha).await && queued(&db, &beta).await);
    assert!(
        !queued(&db, &copied).await,
        "a Wenlan export subtree is pruned"
    );
    for reserved in ["index.md", "log.md", "INSTRUCTIONS.md", "concepts/index.md"] {
        assert!(
            !queued(&db, &root.join(reserved)).await,
            "{reserved} is reserved"
        );
    }
    let source = loaded_source();
    assert_eq!((source.queued_files, source.waiting_files), (2, 0));
    assert!(matches!(source.status, SyncStatus::Active));
}

#[tokio::test]
async fn okf_sync_refuses_a_wenlan_export_root_and_a_deleted_space_until_fixed() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let (db, _db_dir) = new_test_db().await;

    let export = tempfile::tempdir().unwrap();
    write(export.path(), ".wenlan-okf-export.json", "{}");
    let exported = write(export.path(), "pages/alpha.md", &concept("Alpha", "", ""));
    register_okf_source(export.path(), None, pages.path());
    let stats = sync(&db).await;
    assert_eq!(stats.files_found, 0);
    assert!(!queued(&db, &exported).await);
    assert!(matches!(
        loaded_source_status(SOURCE_ID),
        SyncStatus::Unavailable(reason) if reason.contains("Wenlan OKF export")
    ));

    let bundle = tempfile::tempdir().unwrap();
    let alpha = write(bundle.path(), "alpha.md", &concept("Alpha", "", ""));
    register_okf_source(bundle.path(), Some("Research"), pages.path());
    sync(&db).await;
    assert!(!queued(&db, &alpha).await, "no import into a missing Space");
    assert!(matches!(
        loaded_source_status(SOURCE_ID),
        SyncStatus::Unavailable(reason) if reason.contains("Research")
    ));

    db.create_space("Research", None, false).await.unwrap();
    sync(&db).await;
    assert!(queued(&db, &alpha).await);
    assert!(matches!(
        loaded_source_status(SOURCE_ID),
        SyncStatus::Active
    ));
}

#[tokio::test]
async fn okf_sync_hands_over_the_next_batch_only_once_the_last_one_is_prepared() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = (1..=5)
        .map(|i| {
            write(
                bundle.path(),
                &format!("c{i}.md"),
                &concept(&format!("Concept {i}"), "", ""),
            )
        })
        .collect();
    let source = register_okf_source(bundle.path(), None, pages.path());
    let config = wenlan_core::config::load_config();
    let (db, _db_dir) = new_test_db().await;

    let first = sync_okf_source_in_batches(db.clone(), &source, &config, 2)
        .await
        .unwrap();
    assert_eq!(first.newly_queued, 2);
    assert_eq!(first.stats.waiting_files, Some(3));
    assert!(queued(&db, &paths[0]).await && queued(&db, &paths[1]).await);
    assert!(
        !queued(&db, &paths[2]).await,
        "handed over in concept id order"
    );

    let held = sync_okf_source_in_batches(db.clone(), &source, &config, 2)
        .await
        .unwrap();
    assert_eq!(held.newly_queued, 0, "an unprepared batch holds the gate");
    assert_eq!(held.stats.queued_files, Some(2));
    assert_eq!(held.stats.waiting_files, Some(3));
    assert!(!queued(&db, &paths[2]).await);

    // One row finished, the other paused past its retry cap: neither holds
    // the gate.
    db.mark_done(SOURCE_ID, &key(&paths[0])).await.unwrap();
    for _ in 0..5 {
        db.mark_paused(SOURCE_ID, &key(&paths[1]), "provider failed", None)
            .await
            .unwrap();
    }
    let second = sync_okf_source_in_batches(db.clone(), &source, &config, 2)
        .await
        .unwrap();
    assert_eq!(second.newly_queued, 2);
    assert_eq!(second.stats.waiting_files, Some(1));
    assert!(queued(&db, &paths[2]).await && queued(&db, &paths[3]).await);
    // The exhausted row read fine, so it is reported as a worker failure
    // rather than disappearing into a clean `errors: 0`.
    assert_eq!(first.stats.errors, 0);
    assert_eq!(second.stats.errors, 1);
    assert_eq!(
        second.stats.error_detail.as_deref(),
        Some("document_enrichment_failed")
    );

    // Prepared rows park as waiting_for_provider, which opens the gate too.
    assert_eq!(drain(&db).await, 2);
    let last = sync_okf_source_in_batches(db.clone(), &source, &config, 2)
        .await
        .unwrap();
    assert_eq!(last.newly_queued, 1);
    assert_eq!(last.stats.waiting_files, Some(0));
    assert!(queued(&db, &paths[4]).await);
    assert_eq!(loaded_source().waiting_files, 0);
}

#[tokio::test]
async fn okf_deprecation_removes_the_concept_flags_its_page_and_retires_links_to_it() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    let alpha = write(
        root,
        "concepts/alpha.md",
        &concept("Alpha", "", "It builds on [Beta](beta.md)."),
    );
    let beta_text = concept("Beta", "", "");
    let beta = write(root, "concepts/beta.md", &beta_text);
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;

    sync(&db).await;
    assert_eq!(drain(&db).await, 2);
    let alpha_page = source_page_id(SOURCE_ID, &key(&alpha));
    let beta_page = source_page_id(SOURCE_ID, &key(&beta));
    assert_eq!(
        outbound(&db, &alpha_page).await,
        vec![("concepts/beta".to_string(), Some(beta_page.clone()))]
    );
    let alpha_bytes = std::fs::read(&alpha).unwrap();
    let alpha_mtime = mtime_ns(&alpha);

    std::fs::write(
        &beta,
        beta_text.replace("type: concept\n", "type: concept\nstatus: deprecated\n"),
    )
    .unwrap();
    let stats = sync(&db).await;

    assert_eq!(stats.ingested, 0, "a deprecated concept is not queued");
    assert!(db
        .get_memories_by_source_id("memory", &doc_id(&beta))
        .await
        .unwrap()
        .is_empty());
    let beta_row = db.get_page(&beta_page).await.unwrap().expect("page kept");
    assert_eq!(beta_row.stale_reason.as_deref(), Some("source_removed"));
    assert!(db.get_okf_concept(&beta_page).await.unwrap().is_none());
    assert_eq!(
        outbound(&db, &alpha_page).await,
        vec![("concepts/beta".to_string(), None)]
    );
    assert_eq!(std::fs::read(&alpha).unwrap(), alpha_bytes);
    assert_eq!(mtime_ns(&alpha), alpha_mtime);

    // The deprecated file's hash is recorded: the next sync does not read it.
    let again = sync(&db).await;
    assert_eq!((again.ingested, again.skipped), (0, 2));
}

#[tokio::test]
async fn okf_links_resolve_when_the_target_arrives_and_retire_when_it_vanishes() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    let alpha = write(
        root,
        "concepts/alpha.md",
        &concept(
            "Alpha",
            "",
            "See [Beta](/concepts/beta.md) and [Gone](gone.md).",
        ),
    );
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;
    sync(&db).await;
    drain(&db).await;
    let alpha_page = source_page_id(SOURCE_ID, &key(&alpha));
    assert_eq!(
        outbound(&db, &alpha_page).await,
        vec![
            ("concepts/beta".to_string(), None),
            ("concepts/gone".to_string(), None)
        ]
    );

    let beta = write(root, "concepts/beta.md", &concept("Beta", "", ""));
    sync(&db).await;
    drain(&db).await;
    let beta_page = source_page_id(SOURCE_ID, &key(&beta));
    assert_eq!(
        outbound(&db, &alpha_page).await,
        vec![
            ("concepts/beta".to_string(), Some(beta_page.clone())),
            ("concepts/gone".to_string(), None)
        ]
    );

    std::fs::remove_file(&beta).unwrap();
    sync(&db).await;
    assert!(db.get_okf_concept(&beta_page).await.unwrap().is_none());
    assert_eq!(
        outbound(&db, &alpha_page).await,
        vec![
            ("concepts/beta".to_string(), None),
            ("concepts/gone".to_string(), None)
        ]
    );
}

/// A typed `INSTRUCTIONS.md` that is momentarily malformed mid-save is still on
/// disk. The scan cannot tell whether it is typed, so sync keeps what it
/// imported, reports the file as an error, and re-imports it once it parses.
#[tokio::test]
async fn okf_sync_keeps_a_typed_instructions_file_while_it_is_malformed() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    let instructions = write(root, "INSTRUCTIONS.md", &concept("Runbook", "", ""));
    write(root, "concepts/alpha.md", &concept("Alpha", "", ""));
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;
    let page = source_page_id(SOURCE_ID, &key(&instructions));

    let stats = sync(&db).await;
    assert_eq!((stats.files_found, stats.ingested, stats.errors), (2, 2, 0));
    assert_eq!(drain(&db).await, 2);
    let imported = db
        .get_memories_by_source_id("memory", &doc_id(&instructions))
        .await
        .unwrap();
    assert!(!imported.is_empty());
    assert!(db.get_okf_concept(&page).await.unwrap().is_some());
    assert!(db
        .get_sync_state(SOURCE_ID, &key(&instructions))
        .await
        .unwrap()
        .is_some());

    // Mid-save: the frontmatter no longer parses.
    let valid = std::fs::read_to_string(&instructions).unwrap();
    std::fs::write(
        &instructions,
        valid.replacen("type: concept", "type: [concept", 1),
    )
    .unwrap();
    for _ in 0..2 {
        let stats = sync(&db).await;
        assert_eq!(stats.errors, 1, "the failure is reported: {stats:?}");
        assert_eq!(stats.error_detail.as_deref(), Some("file_read_errors"));
        assert_eq!(stats.files_found, 1, "only the alpha concept is scanned");
        assert_eq!(loaded_source().last_sync_errors, 1);
        assert_eq!(
            db.get_memories_by_source_id("memory", &doc_id(&instructions))
                .await
                .unwrap()
                .len(),
            imported.len(),
            "the last good import is kept"
        );
        assert!(db.get_okf_concept(&page).await.unwrap().is_some());
        assert!(db
            .get_sync_state(SOURCE_ID, &key(&instructions))
            .await
            .unwrap()
            .is_some());
        assert!(
            db.get_page(&page)
                .await
                .unwrap()
                .is_some_and(|row| row.stale_reason.as_deref() != Some("source_removed")),
            "the page is not flagged as removed"
        );
    }

    // Saved again, with new content: re-imported normally, no error left.
    std::fs::write(&instructions, concept("Runbook Revised", "", "")).unwrap();
    let stats = sync(&db).await;
    assert_eq!((stats.errors, stats.error_detail), (0, None));
    assert_eq!(stats.ingested, 1, "the revised file is handed over");
    assert!(queued(&db, &instructions).await);
    drain(&db).await;
    let revised = db
        .get_memories_by_source_id("memory", &doc_id(&instructions))
        .await
        .unwrap();
    assert!(revised
        .iter()
        .any(|chunk| chunk.content.contains("Runbook Revised")));
    assert_eq!(loaded_source().last_sync_errors, 0);
}

/// Keeping an unclassifiable file is not a blanket exemption: deleting the
/// file, or removing its `type` so that it is confirmed untyped, still removes
/// what it imported.
#[tokio::test]
async fn okf_sync_still_removes_an_instructions_file_that_is_deleted_or_becomes_untyped() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    let top = write(root, "INSTRUCTIONS.md", &concept("Runbook", "", ""));
    let nested = write(root, "ops/INSTRUCTIONS.md", &concept("Ops", "", ""));
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;
    sync(&db).await;
    assert_eq!(drain(&db).await, 2);
    for path in [&top, &nested] {
        assert!(db
            .get_okf_concept(&source_page_id(SOURCE_ID, &key(path)))
            .await
            .unwrap()
            .is_some());
    }

    std::fs::remove_file(&top).unwrap();
    std::fs::write(&nested, "# How to edit this folder\n").unwrap();
    let stats = sync(&db).await;

    assert_eq!(stats.errors, 0, "{stats:?}");
    for path in [&top, &nested] {
        assert!(db
            .get_memories_by_source_id("memory", &doc_id(path))
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .get_okf_concept(&source_page_id(SOURCE_ID, &key(path)))
            .await
            .unwrap()
            .is_none());
        assert!(db
            .get_sync_state(SOURCE_ID, &key(path))
            .await
            .unwrap()
            .is_none());
    }
}

/// Only a file that imported before has state to keep, so only it is a file
/// error. A malformed `INSTRUCTIONS.md` that never imported is skipped quietly,
/// as it was before, and imports once it parses.
#[tokio::test]
async fn okf_sync_skips_a_never_imported_malformed_instructions_file_quietly() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    let valid = concept("Runbook", "", "");
    let instructions = write(
        root,
        "INSTRUCTIONS.md",
        &valid.replacen("type: concept", "type: [concept", 1),
    );
    write(root, "concepts/alpha.md", &concept("Alpha", "", ""));
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;

    let stats = sync(&db).await;
    assert_eq!((stats.files_found, stats.errors), (1, 0), "{stats:?}");
    assert_eq!(loaded_source().last_sync_errors, 0);
    assert!(db
        .get_sync_state(SOURCE_ID, &key(&instructions))
        .await
        .unwrap()
        .is_none());
    drain(&db).await;

    std::fs::write(&instructions, &valid).unwrap();
    let stats = sync(&db).await;
    assert_eq!((stats.ingested, stats.errors), (1, 0), "{stats:?}");
    assert!(queued(&db, &instructions).await);
}

#[tokio::test]
async fn okf_same_bytes_rename_rekeys_the_concept_and_its_linkers_re_resolve() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let root = bundle.path();
    let alpha = write(
        root,
        "concepts/alpha.md",
        &concept("Alpha", "", "It builds on [Beta](beta.md)."),
    );
    let beta = write(root, "concepts/beta.md", &concept("Beta", "", ""));
    register_okf_source(root, None, pages.path());
    let (db, _db_dir) = new_test_db().await;
    sync(&db).await;
    drain(&db).await;
    let alpha_page = source_page_id(SOURCE_ID, &key(&alpha));
    let beta_page = source_page_id(SOURCE_ID, &key(&beta));

    let gamma = root.join("concepts/gamma.md");
    std::fs::rename(&beta, &gamma).unwrap();
    let stats = sync(&db).await;

    assert_eq!(
        stats.ingested, 0,
        "a same-bytes rename is rebound, not queued"
    );
    let gamma_page = source_page_id(SOURCE_ID, &key(&gamma));
    let record = db
        .get_okf_concept(&gamma_page)
        .await
        .unwrap()
        .expect("row follows the page");
    assert_eq!(record.concept_id, "concepts/gamma");
    assert!(db.get_okf_concept(&beta_page).await.unwrap().is_none());
    assert_eq!(
        outbound(&db, &alpha_page).await,
        vec![("concepts/beta".to_string(), None)],
        "alpha still links to the old path"
    );
}

#[tokio::test]
async fn okf_concepts_land_in_the_source_space_and_a_moved_page_keeps_its_space() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let text = concept("Alpha", "", "");
    let alpha = write(bundle.path(), "alpha.md", &text);
    let (db, _db_dir) = new_test_db().await;
    db.create_space("Research", None, false).await.unwrap();
    db.create_space("Archive", None, false).await.unwrap();
    register_okf_source(bundle.path(), Some("Research"), pages.path());

    sync(&db).await;
    drain(&db).await;
    let page_id = source_page_id(SOURCE_ID, &key(&alpha));
    let page = db.get_page(&page_id).await.unwrap().expect("source page");
    assert_eq!(page.space.as_deref(), Some("Research"));
    let spaces = chunk_spaces(&db, &alpha).await;
    assert!(!spaces.is_empty());
    assert!(spaces.iter().all(|s| s.as_deref() == Some("Research")));
    let record = db
        .get_okf_concept(&page_id)
        .await
        .unwrap()
        .expect("provenance");
    assert_eq!(record.frontmatter["type"], "concept");

    db.set_page_workspace(&page_id, Some("Archive"))
        .await
        .unwrap();
    std::fs::write(&alpha, text.replace("over time.", "over time and space.")).unwrap();
    sync(&db).await;
    drain(&db).await;

    let page = db.get_page(&page_id).await.unwrap().expect("source page");
    assert_eq!(page.space.as_deref(), Some("Archive"));
    let spaces = chunk_spaces(&db, &alpha).await;
    assert!(!spaces.is_empty());
    assert!(spaces.iter().all(|s| s.as_deref() == Some("Archive")));
}

#[tokio::test]
async fn removing_an_okf_source_cancels_its_queue_and_drops_its_concepts() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let alpha = write(bundle.path(), "alpha.md", &concept("Alpha", "", ""));
    let beta = write(bundle.path(), "beta.md", &concept("Beta", "", ""));
    register_okf_source(bundle.path(), None, pages.path());
    let (db, _db_dir) = new_test_db().await;
    sync(&db).await;
    // Prepare one concept; leave the other queued.
    let prompts = wenlan_core::prompts::PromptRegistry::default();
    let knowledge_path = wenlan_core::config::load_config().knowledge_path_or_default();
    let entry = db
        .claim_next_pending_for_provider(false)
        .await
        .unwrap()
        .unwrap();
    let prepared_page = source_page_id(SOURCE_ID, &entry.file_path);
    wenlan_core::document_enrichment::run_document_enrichment_slice(
        &db,
        &entry,
        Some(&knowledge_path),
        None,
        &prompts,
    )
    .await;
    assert!(db.get_okf_concept(&prepared_page).await.unwrap().is_some());

    handle_remove_source(State(server_state(&db)), RoutePath(SOURCE_ID.to_string()))
        .await
        .unwrap();

    assert!(db.get_okf_concept(&prepared_page).await.unwrap().is_none());
    assert!(!queued(&db, &alpha).await && !queued(&db, &beta).await);
    assert!(db
        .claim_next_pending_for_provider(true)
        .await
        .unwrap()
        .is_none());
}

/// The bundle's own frontmatter is kept for display, so page detail has to hand
/// it back. Without this the provenance would be write-only and the spec's
/// "a concept page returns its frontmatter" would be false. Driven through the
/// real router, because a store that nothing reads proves nothing.
#[tokio::test]
async fn page_detail_returns_the_concept_frontmatter_and_omits_it_for_other_pages() {
    use tower::ServiceExt;

    let _guard = data_dir_lock().await;
    let _config_root = DataDirGuard::new();
    let (db, _tmp) = new_test_db().await;
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..Default::default()
    }));

    // `create_page_draft_with_id` only accepts the `page_<uuid-v4>` shape. The
    // subject here is the route, not how an import mints its id.
    let page_id = "page_3f2b1c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    let plain_id = "page_9a8b7c6d-5e4f-4321-9876-0a1b2c3d4e5f";
    let frontmatter = serde_json::json!({
        "type": "concept",
        "status": "stale",
        "stale_after": "2026-12-01",
        "generated": {"by": "openwiki", "at": "2026-09-01"},
        "verified": [{"by": "a-human", "at": "2026-09-02"}],
        "sources": [{"resource": "repo://acme/lib#L10-L20"}],
    });
    for id in [page_id, plain_id] {
        db.create_page_draft_with_id(id, "Alpha", "Alpha body.", None, None)
            .await
            .unwrap();
    }
    db.upsert_okf_concept(page_id, SOURCE_ID, "concepts/alpha", &frontmatter, &[])
        .await
        .unwrap();

    async fn detail(
        state: &Arc<RwLock<ServerState>>,
        id: &str,
    ) -> (axum::http::StatusCode, serde_json::Value) {
        let response = crate::router::build_router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/api/pages/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    let (status, body) = detail(&state, page_id).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        body["okf"]["frontmatter"], frontmatter,
        "the frontmatter comes back structurally equal to the file's: {body}"
    );
    assert_eq!(body["okf"]["concept_id"], "concepts/alpha");
    assert_eq!(body["okf"]["source_id"], SOURCE_ID);
    assert!(
        body["okf"]["updated_at"].is_number(),
        "the record carries when the concept last changed: {body}"
    );
    assert_eq!(body["page"]["id"], page_id, "the page itself still opens");

    let (status, body) = detail(&state, plain_id).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body["okf"].is_null(),
        "a page that is not an imported concept keeps today's shape: {body}"
    );
}
