// SPDX-License-Identifier: Apache-2.0

use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use tokio::sync::RwLock;
use wenlan_core::config::Config;
use wenlan_core::sources::{Source, SourceType, SyncStatus};

use crate::error::ServerError;
use crate::source_routes::tests::{new_test_db, DataDirGuard};
use crate::source_routes::{handle_add_source, AddSourceRequest};
use crate::space_header::SpaceHeader;
use crate::state::ServerState;

async fn data_dir_lock() -> tokio::sync::MutexGuard<'static, ()> {
    crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn save_config(pages: &Path, sources: Vec<Source>) {
    wenlan_core::config::save_config(&Config {
        sources,
        knowledge_path: Some(pages.to_path_buf()),
        ..Config::default()
    })
    .unwrap();
}

fn registered(id: &str, source_type: SourceType, path: &Path) -> Source {
    Source {
        id: id.to_string(),
        source_type,
        path: path.to_path_buf(),
        status: SyncStatus::Active,
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

fn bundle_in(parent: &Path, name: &str) -> std::path::PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(root.join("concepts")).unwrap();
    std::fs::write(
        root.join("index.md"),
        "---\nokf_version: \"0.2\"\n---\n# Wiki\n",
    )
    .unwrap();
    root
}

async fn add(
    state: &Arc<RwLock<ServerState>>,
    source_type: &str,
    path: &Path,
    space: Option<&str>,
) -> Result<Source, ServerError> {
    handle_add_source(
        State(state.clone()),
        SpaceHeader(space.map(str::to_string)),
        Json(AddSourceRequest {
            source_type: source_type.to_string(),
            path: path.to_string_lossy().to_string(),
            follow_changes: None,
        }),
    )
    .await
    .map(|Json(source)| source)
}

fn validation_message(result: Result<Source, ServerError>) -> String {
    match result {
        Err(ServerError::ValidationError(message)) => message,
        other => panic!("expected a ValidationError, got {other:?}"),
    }
}

#[tokio::test]
async fn okf_registration_stores_the_header_space_or_the_default_space() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    save_config(pages.path(), Vec::new());
    let (db, _db_dir) = new_test_db().await;
    db.create_space("Research", None, false).await.unwrap();
    let home = db.create_space("Home", None, false).await.unwrap();
    db.set_default_space(&home.id).await.unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }));

    let wiki = bundle_in(parent.path(), "Wiki");
    let source = add(&state, "okf", &wiki, Some("Research")).await.unwrap();
    assert_eq!(source.id, "okf-wiki");
    assert_eq!(source.source_type, SourceType::Okf);
    assert_eq!(source.space.as_deref(), Some("Research"));

    // A plain markdown file with a `type` field is enough of a signal, and a
    // second folder of the same name gets its own id.
    let other_parent = parent.path().join("other");
    let second = other_parent.join("Wiki");
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(second.join("idea.md"), "---\ntype: concept\n---\n# Idea\n").unwrap();
    let source = add(&state, "okf", &second, None).await.unwrap();
    assert_eq!(source.id, "okf-wiki-2");
    assert_eq!(source.space.as_deref(), Some("Home"));

    // The stored Space is the one used later, whatever the default becomes.
    db.set_default_space(
        &db.get_space("Research")
            .await
            .unwrap()
            .expect("Research")
            .id,
    )
    .await
    .unwrap();
    let stored = wenlan_core::config::load_config()
        .sources
        .into_iter()
        .find(|s| s.id == "okf-wiki-2")
        .unwrap();
    assert_eq!(stored.space.as_deref(), Some("Home"));

    let message = validation_message(
        add(
            &state,
            "okf",
            &bundle_in(parent.path(), "Third"),
            Some("Missing"),
        )
        .await,
    );
    assert!(message.contains("Missing"), "{message}");
}

#[tokio::test]
async fn okf_registration_refuses_folders_that_are_not_importable_bundles() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    save_config(pages.path(), Vec::new());
    let (db, _db_dir) = new_test_db().await;
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }));

    let file = parent.path().join("note.md");
    std::fs::write(&file, "---\ntype: concept\n---\n# Note\n").unwrap();
    let message = validation_message(add(&state, "okf", &file, None).await);
    assert!(message.contains("not a directory"), "{message}");

    let export = bundle_in(parent.path(), "export");
    std::fs::write(export.join(".wenlan-okf-export.json"), "{}").unwrap();
    let message = validation_message(add(&state, "okf", &export, None).await);
    assert!(message.contains("Wenlan OKF export"), "{message}");

    let plain = parent.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(
        plain.join("note.md"),
        "# Just notes\n\nNo frontmatter here.\n",
    )
    .unwrap();
    let message = validation_message(add(&state, "okf", &plain, None).await);
    assert!(message.contains("No OKF bundle"), "{message}");

    assert!(wenlan_core::config::load_config().sources.is_empty());
}

#[tokio::test]
async fn okf_registration_refuses_overlap_with_pages_and_other_sources() {
    let _lock = data_dir_lock().await;
    let _env = DataDirGuard::new();
    let pages = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (db, _db_dir) = new_test_db().await;
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }));

    // Inside the pages folder, and holding it.
    save_config(pages.path(), Vec::new());
    let inside_pages = bundle_in(pages.path(), "wiki");
    let message = validation_message(add(&state, "okf", &inside_pages, None).await);
    assert!(message.contains("pages folder"), "{message}");
    let holder = bundle_in(parent.path(), "holder");
    save_config(&holder.join("pages"), Vec::new());
    std::fs::create_dir_all(holder.join("pages")).unwrap();
    let message = validation_message(add(&state, "okf", &holder, None).await);
    assert!(message.contains("pages folder"), "{message}");

    // Inside a registered folder source, reached through `..`.
    let notes = parent.path().join("notes");
    let nested = bundle_in(&notes, "wiki");
    save_config(
        pages.path(),
        vec![registered("directory-notes", SourceType::Directory, &notes)],
    );
    let dotted = notes.join("wiki").join("..").join("wiki");
    let message = validation_message(add(&state, "okf", &dotted, None).await);
    assert!(message.contains("directory-notes"), "{message}");
    assert!(nested.is_dir());

    // A plain folder holding a registered bundle, reached through a symlink.
    let bundle = bundle_in(parent.path(), "bundle");
    save_config(
        pages.path(),
        vec![registered("okf-bundle", SourceType::Okf, &bundle)],
    );
    #[cfg(unix)]
    {
        let link = parent.path().join("bundle-link");
        std::os::unix::fs::symlink(&bundle, &link).unwrap();
        let message = validation_message(add(&state, "okf", &link, None).await);
        assert!(message.contains("okf-bundle"), "{message}");
    }
    let message = validation_message(add(&state, "directory", parent.path(), None).await);
    assert!(message.contains("okf-bundle"), "{message}");

    // Two plain folders may still nest, as before.
    let deeper = parent.path().join("deeper");
    std::fs::create_dir_all(deeper.join("inner")).unwrap();
    save_config(
        pages.path(),
        vec![registered(
            "directory-deeper",
            SourceType::Directory,
            &deeper,
        )],
    );
    add(&state, "directory", &deeper.join("inner"), None)
        .await
        .expect("plain folders keep nesting");
}
