// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::store::export::tests::fixture;

fn pages() -> PagesFolder {
    PagesFolder::default()
}

fn leftovers(mirror: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(mirror)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_run_writes_the_files_from_a_snapshot_and_checks_them() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().expect("the test database has a model");
    let memories_before = db.export_memory_rows().await.unwrap();

    let run = run_once(
        &db_file,
        &pages(),
        data_root.path(),
        &mirror,
        embedder,
        true,
    )
    .await
    .unwrap();

    assert!(run.files > 0);
    assert_eq!(run.check_clean, Some(true), "{}", {
        std::fs::read_to_string(mirror.join(REPORT_TEXT)).unwrap_or_default()
    });
    assert!(mirror
        .join(FILES_DIR)
        .join("export-manifest.json")
        .is_file());
    assert!(std::fs::read_to_string(mirror.join(REPORT_TEXT))
        .unwrap()
        .starts_with("CLEAN"));
    // The snapshot and the rebuilt database are gone; only the files and
    // the reports stay.
    assert_eq!(leftovers(&mirror), [REPORT_JSON, REPORT_TEXT, FILES_DIR]);
    // The database itself is only read.
    assert_eq!(db.export_memory_rows().await.unwrap(), memories_before);
}

#[tokio::test]
async fn the_next_run_replaces_the_files_whole() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().unwrap();
    run_once(
        &db_file,
        &pages(),
        data_root.path(),
        &mirror,
        embedder.clone(),
        false,
    )
    .await
    .unwrap();
    let stale = mirror.join(FILES_DIR).join("stale.md");
    std::fs::write(&stale, "from an older run").unwrap();
    // A run that stopped part way left these behind.
    std::fs::create_dir_all(mirror.join("snapshot").join("memorydb")).unwrap();
    std::fs::create_dir_all(mirror.join("files.next")).unwrap();

    let run = run_once(
        &db_file,
        &pages(),
        data_root.path(),
        &mirror,
        embedder,
        false,
    )
    .await
    .unwrap();
    assert_eq!(run.check_clean, None);
    assert!(!stale.exists(), "the new files replace the old folder");
    assert_eq!(leftovers(&mirror), [FILES_DIR]);
}

#[tokio::test]
async fn a_failed_run_keeps_the_previous_files() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().unwrap();
    run_once(
        &db_file,
        &pages(),
        data_root.path(),
        &mirror,
        embedder.clone(),
        false,
    )
    .await
    .unwrap();
    let manifest = mirror.join(FILES_DIR).join("export-manifest.json");
    let before = std::fs::read(&manifest).unwrap();
    // The database cannot be copied: the run stops.
    let missing = dir.path().join("gone.db");
    let failed = run_once(
        &missing,
        &pages(),
        data_root.path(),
        &mirror,
        embedder,
        false,
    )
    .await;
    assert!(failed.is_err());
    assert!(!missing.exists(), "a missing database is not created");
    assert_eq!(std::fs::read(&manifest).unwrap(), before);
}

#[test]
fn the_status_counts_checks_and_keeps_the_last_good_files_on_failure() {
    let run = MirrorRun {
        files: 12,
        written: 3,
        removed: 1,
        write_ms: 30,
        check_clean: Some(true),
        check_ms: 40,
        cleanup: None,
    };
    let status = after_run(FilesMirrorStatus::default(), &run, 100);
    assert_eq!(status.state, FilesMirrorState::Ok);
    assert_eq!((status.checks, status.clean_checks), (1, 1));
    let differs = MirrorRun {
        check_clean: Some(false),
        ..run.clone()
    };
    let status = after_run(status, &differs, 200);
    assert_eq!(status.state, FilesMirrorState::Differences);
    assert_eq!((status.checks, status.clean_checks), (2, 1));
    // A write without a check keeps the last check's verdict.
    let status = after_run(
        status,
        &MirrorRun {
            check_clean: None,
            ..run.clone()
        },
        300,
    );
    assert_eq!(
        (status.state.clone(), status.checked_at, status.written_at),
        (FilesMirrorState::Differences, Some(200), Some(300))
    );
    let failed = after_failure(status, &WenlanError::Validation("disk full".into()));
    assert_eq!(failed.state, FilesMirrorState::Failed);
    assert_eq!(failed.written_at, Some(300));
    assert_eq!(failed.note.as_deref(), Some("Validation error: disk full"));

    let dir = tempfile::tempdir().unwrap();
    write_status(dir.path(), &failed).unwrap();
    assert_eq!(read_status(dir.path()), Some(failed));
}

#[tokio::test]
async fn files_left_half_swapped_are_put_back_first() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().unwrap();
    run_once(
        &db_file,
        &pages(),
        data_root.path(),
        &mirror,
        embedder.clone(),
        false,
    )
    .await
    .unwrap();
    let manifest = std::fs::read(mirror.join(FILES_DIR).join("export-manifest.json")).unwrap();
    // A run stopped after `files` became `files.old` and before the new
    // files took its place; then the next run fails early.
    std::fs::rename(mirror.join(FILES_DIR), mirror.join("files.old")).unwrap();

    let missing = dir.path().join("gone.db");
    let failed = run_once(
        &missing,
        &pages(),
        data_root.path(),
        &mirror,
        embedder,
        false,
    )
    .await;
    assert!(failed.is_err());
    assert_eq!(
        std::fs::read(mirror.join(FILES_DIR).join("export-manifest.json")).unwrap(),
        manifest,
        "the last good files are back in place"
    );
}

fn manifest_paths(files: &Path) -> Vec<String> {
    crate::store::export::read_manifest(files)
        .unwrap()
        .files
        .into_iter()
        .map(|file| file.path)
        .collect()
}

#[tokio::test]
async fn a_refresh_writes_only_what_changed_and_removes_what_is_gone() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());

    let first = refresh(&db_file, &pages(), data_root.path(), &mirror)
        .await
        .unwrap();
    assert!(first.files > 0);
    assert_eq!(
        first.written, first.files,
        "a first refresh writes every file"
    );
    let files = mirror.join(FILES_DIR);
    let before = manifest_paths(&files);

    // Nothing changed: nothing is written or removed, also a second later
    // (an entry with no stored time once took the export's own time).
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let again = refresh(&db_file, &pages(), data_root.path(), &mirror)
        .await
        .unwrap();
    assert_eq!((again.written, again.removed), (0, 0));
    assert_eq!(manifest_paths(&files), before);

    // One page changes, one is deleted, and a refresh that stopped part
    // way left a temporary file behind.
    db.export_fixture_sql(
        "UPDATE pages SET content = 'The launch moved to 2026-04-01.' WHERE id = ?1",
        "a",
    )
    .await;
    db.export_fixture_sql("DELETE FROM pages WHERE id = ?1", "x")
        .await;
    let leftover = files.join("visible").join("half.md.wenlan-tmp");
    std::fs::write(&leftover, "half").unwrap();
    let changed = refresh(&db_file, &pages(), data_root.path(), &mirror)
        .await
        .unwrap();
    assert!(changed.written >= 1, "{changed:?}");
    assert!(changed.written < first.written, "{changed:?}");
    assert!(changed.removed >= 1, "{changed:?}");
    assert!(!leftover.exists());
    let after = manifest_paths(&files);
    assert_eq!(after.len(), before.len() - 1);
    let alpha = std::fs::read_to_string(files.join("visible").join("Alpha.md")).unwrap();
    assert!(alpha.contains("2026-04-01"), "{alpha}");
    // Every file the manifest lists is there; nothing else is.
    for path in &after {
        assert!(files.join(path).is_file(), "{path}");
    }

    // The refreshed files are a lossless export of the database now.
    let work = tempfile::tempdir().unwrap();
    let report = check_export_with(&db, &files, &work.path().join("check"), None)
        .await
        .unwrap();
    assert!(report.is_clean(), "{}", report.summary());
}

#[tokio::test]
async fn a_read_snapshot_sees_one_moment_while_the_database_changes() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let snapshot = MemoryDB::open_read_snapshot(&db_file).await.unwrap();
    let titles = |rows: Vec<crate::db::export_reads::ExportPageRow>| {
        rows.into_iter()
            .map(|row| row.title)
            .collect::<std::collections::BTreeSet<_>>()
    };
    let seen = titles(snapshot.export_page_rows().await.unwrap());
    db.export_fixture_sql("UPDATE pages SET title = 'Alpha two' WHERE id = ?1", "a")
        .await;
    assert_eq!(titles(snapshot.export_page_rows().await.unwrap()), seen);
    assert!(titles(db.export_page_rows().await.unwrap()).contains("Alpha two"));
    // It writes nothing.
    assert!(snapshot
        .export_fixture_sql_result("UPDATE pages SET title = 'no' WHERE id = ?1", "a")
        .await
        .is_err());
    // A missing file is refused, not created.
    let missing = dir.path().join("gone.db");
    assert!(MemoryDB::open_read_snapshot(&missing).await.is_err());
    assert!(!missing.exists());
}

#[test]
fn a_refresh_keeps_the_last_check_and_clears_a_failure() {
    let refreshed = MirrorRun {
        files: 5,
        written: 1,
        write_ms: 9,
        ..MirrorRun::default()
    };
    let checked = FilesMirrorStatus {
        state: FilesMirrorState::Differences,
        check_clean: Some(false),
        checked_at: Some(100),
        note: Some("waiting for the search model to load".into()),
        ..FilesMirrorStatus::default()
    };
    let status = after_refresh(checked, &refreshed, 200);
    assert_eq!(status.state, FilesMirrorState::Differences);
    assert_eq!(
        (status.written_at, status.checked_at),
        (Some(200), Some(100))
    );
    assert!(status.note.is_some(), "a note on a waiting check stays");

    let failed = after_failure(
        FilesMirrorStatus::default(),
        &WenlanError::Validation("disk full".into()),
    );
    let status = after_refresh(failed, &refreshed, 300);
    assert_eq!(status.state, FilesMirrorState::Ok);
    assert_eq!(status.note, None);
}

#[tokio::test]
async fn an_emptied_database_does_not_empty_the_files() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    refresh(&db_file, &pages(), data_root.path(), &mirror)
        .await
        .unwrap();
    let files = mirror.join(FILES_DIR);
    let mut manifest = crate::store::export::read_manifest(&files).unwrap();
    let content = db.export_content_count().await.unwrap();
    assert!(content > 0);
    // Files that list far more memories than the database now holds, as
    // after the database came back empty.
    let listed: Vec<_> = (0..(2 * content + COLLAPSE_FLOOR))
        .map(|n| crate::store::export::ExportedFile {
            id: format!("gone-{n}"),
            file_type: "memory".to_string(),
            path: format!("backstage/memories/gone-{n}.md"),
            sha256: String::new(),
        })
        .collect();
    manifest.files.extend(listed);
    std::fs::write(
        files.join(crate::store::export::MANIFEST_FILE),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();
    let kept = std::fs::read(files.join(crate::store::export::MANIFEST_FILE)).unwrap();

    let refused = refresh(&db_file, &pages(), data_root.path(), &mirror).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("not refreshed")),
        "{refused:?}"
    );
    assert_eq!(
        std::fs::read(files.join(crate::store::export::MANIFEST_FILE)).unwrap(),
        kept,
        "nothing written"
    );
    // A small copy is not compared: a new database grows past it.
    assert!(refuse_a_collapse(&db, &mirror.join("nothing-here"))
        .await
        .is_ok());
}

#[tokio::test]
async fn a_file_the_manifest_lists_but_is_gone_is_a_problem_not_an_error() {
    let (db, dir) = fixture().await;
    let db_file = dir.path().join("origin_memory.db");
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    refresh(&db_file, &pages(), data_root.path(), &mirror)
        .await
        .unwrap();
    let files = mirror.join(FILES_DIR);
    let manifest = crate::store::export::read_manifest(&files).unwrap();
    // A refresh stopped after removing these and before its manifest.
    let mut gone = Vec::new();
    for kind in ["page", "memory"] {
        let file = manifest
            .files
            .iter()
            .find(|file| file.file_type == kind)
            .unwrap();
        std::fs::remove_file(files.join(&file.path)).unwrap();
        gone.push(file.path.clone());
    }
    drop(db);

    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = MemoryDB::new(fresh_dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let rebuilt = crate::store::rebuild::rebuild(&files, &fresh)
        .await
        .unwrap();
    for path in gone {
        assert!(
            rebuilt
                .problems
                .contains(&format!("{path}: listed but missing")),
            "{:?}",
            rebuilt.problems
        );
    }
    assert!(rebuilt.tables["pages"] > 0);
}
