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
    let (db, _dir) = fixture().await;
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().expect("the test database has a model");
    let memories_before = db.export_memory_rows().await.unwrap();

    let run = run_once(&db, &pages(), data_root.path(), &mirror, embedder, true)
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
    let (db, _dir) = fixture().await;
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().unwrap();
    run_once(
        &db,
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

    let run = run_once(&db, &pages(), data_root.path(), &mirror, embedder, false)
        .await
        .unwrap();
    assert_eq!(run.check_clean, None);
    assert!(!stale.exists(), "the new files replace the old folder");
    assert_eq!(leftovers(&mirror), [FILES_DIR]);
}

#[tokio::test]
async fn a_failed_run_keeps_the_previous_files() {
    let (db, _dir) = fixture().await;
    let data_root = tempfile::tempdir().unwrap();
    let mirror = mirror_root(data_root.path());
    let embedder = db.shared_embedder().unwrap();
    run_once(
        &db,
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
    // A file where the run keeps its snapshot folder: the run stops.
    std::fs::write(mirror.join("snapshot"), "in the way").unwrap();

    let failed = run_once(&db, &pages(), data_root.path(), &mirror, embedder, false).await;
    assert!(failed.is_err());
    assert_eq!(std::fs::read(&manifest).unwrap(), before);
}

#[test]
fn the_status_counts_checks_and_keeps_the_last_good_files_on_failure() {
    let run = MirrorRun {
        files: 12,
        write_ms: 30,
        check_clean: Some(true),
        check_ms: 40,
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
