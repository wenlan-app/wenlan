// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::store::export::run::PagesFolder;
use crate::store::export::tests::fixture;
use crate::store::mirror::run_once;
use crate::store::rebuild::compare::{compare_keyed, compare_memories};

/// A data root whose file copy holds the fixture database.
async fn files_of_fixture() -> (MemoryDB, tempfile::TempDir, tempfile::TempDir) {
    let (db, dir) = fixture().await;
    let data_root = tempfile::tempdir().unwrap();
    let embedder = db.shared_embedder().expect("the test database has a model");
    run_once(
        &dir.path().join(DATABASE_FILE),
        &PagesFolder::default(),
        data_root.path(),
        &mirror_root(data_root.path()),
        embedder,
        false,
    )
    .await
    .unwrap();
    (db, dir, data_root)
}

#[tokio::test]
async fn a_missing_database_is_built_again_from_the_files() {
    let (source, _source_dir, data_root) = files_of_fixture().await;
    let db_dir = data_root.path().join("memorydb");
    std::fs::create_dir_all(&db_dir).unwrap();
    // Left by the database that is gone.
    std::fs::write(db_dir.join(format!("{DATABASE_FILE}-wal")), b"old log").unwrap();

    let outcome = restore_missing_database(data_root.path(), &db_dir)
        .await
        .unwrap();
    let Restore::Restored { rows, problems, .. } = outcome else {
        panic!("expected a restore, got {outcome:?}");
    };
    assert!(rows > 0);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(db_dir.join(DATABASE_FILE).is_file());
    assert!(!db_dir.join(format!("{DATABASE_FILE}-wal")).exists());
    let mirror = mirror_root(data_root.path());
    assert!(mirror.join(RESTORE_REPORT).is_file());
    assert!(mirror.join(RESTORE_DIR).join("stray-wal").is_file());
    // The files are only read.
    assert!(mirror.join(FILES_DIR).join(MANIFEST_FILE).is_file());

    let restored = MemoryDB::new_with_embedding_recovery(
        &db_dir,
        Arc::new(NoopEmitter),
        MemoryEmbeddingRecovery::Deferred,
    )
    .await
    .unwrap();
    let pages = compare_keyed(&source, &restored, "pages", &["id"], &[])
        .await
        .unwrap();
    assert!(pages.rows > 0);
    assert!(pages.only_in_source.is_empty(), "{pages:#?}");
    assert!(pages.only_in_rebuilt.is_empty(), "{pages:#?}");
    // Memories: one row per file (chunks are cache), the same decisions.
    let manifest: crate::store::export::Manifest = serde_json::from_str(
        &std::fs::read_to_string(mirror.join(FILES_DIR).join(MANIFEST_FILE)).unwrap(),
    )
    .unwrap();
    let written = manifest
        .files
        .iter()
        .filter(|file| file.file_type == "memory" || file.file_type == "original")
        .map(|file| file.id.clone())
        .collect();
    let memories = compare_memories(&source, &restored, &written)
        .await
        .unwrap();
    assert!(memories.rows > 0);
    assert!(memories.is_empty(), "{memories:#?}");
    drop(restored);

    // Now the database exists: a second start leaves it alone.
    let before = std::fs::metadata(db_dir.join(DATABASE_FILE))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        restore_missing_database(data_root.path(), &db_dir)
            .await
            .unwrap(),
        Restore::NotNeeded
    );
    assert_eq!(
        std::fs::metadata(db_dir.join(DATABASE_FILE))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn with_no_files_the_daemon_starts_empty_as_before() {
    let data_root = tempfile::tempdir().unwrap();
    let db_dir = data_root.path().join("memorydb");
    assert_eq!(
        restore_missing_database(data_root.path(), &db_dir)
            .await
            .unwrap(),
        Restore::NoFiles
    );
    assert!(!db_dir.exists());
    assert!(!mirror_root(data_root.path()).exists());
}

#[tokio::test]
async fn a_failed_restore_keeps_the_files_aside() {
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let files = mirror.join(FILES_DIR);
    std::fs::create_dir_all(&files).unwrap();
    // A manifest that cannot be read: the rebuild stops.
    std::fs::write(files.join(MANIFEST_FILE), "not json").unwrap();
    let db_dir = data_root.path().join("memorydb");

    assert!(restore_missing_database(data_root.path(), &db_dir)
        .await
        .is_err());
    assert!(!db_dir.join(DATABASE_FILE).exists());

    let kept = keep_files_after_failure(data_root.path(), 1_700_000_000).unwrap();
    assert_eq!(kept, mirror.join(format!("{KEPT_PREFIX}1700000000")));
    assert!(kept.join(MANIFEST_FILE).is_file());
    assert!(!files.exists());
    // Nothing left to restore from: the next start begins empty.
    assert_eq!(restore_source(data_root.path()), None);
}
