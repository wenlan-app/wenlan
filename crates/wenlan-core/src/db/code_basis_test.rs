// SPDX-License-Identifier: Apache-2.0
//! DB-level and file-level tests for a note relying on lines of code.
//!
//! The numbered tests follow the acceptance list of the code-basis spec
//! (`2026-10-20-code-basis-spec.md`, 验收) and the PR 2a brief.

use super::{build_chunker, BasisWrite, MemoryDB, FTS_SCHEMA, FTS_TRIGGERS, SCHEMA};
use crate::cache::EmbeddingCache;
use crate::code_anchor::{self, LineSpan};
use crate::code_basis::{
    brief_code_basis_pending, confirm_code_basis, git_blob_id, prepare_code_basis,
    recheck_code_basis, recheck_code_basis_in_scope, resolve_code_path, CodeBasisError,
    CODE_BASIS_STATE_MOVED, CODE_BASIS_STATE_OK, CODE_BASIS_STATE_PENDING,
    CODE_BASIS_STATE_UNAVAILABLE, MAX_CODE_FILE_BYTES,
};
use crate::read_scope::ReadScope;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use wenlan_types::{CodeBasisInput, CodeBasisLocator, CodeBasisRef};

/// A migrated scratch database with no embedder: nothing here embeds text, so
/// the code-basis tests run without the ONNX model the shared fixture loads.
/// `ceiling` stops the migration chain at a historical schema state.
async fn build_db(ceiling: Option<i64>) -> (MemoryDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_file = dir.path().join("origin_memory.db");
    let db = libsql::Builder::new_local(db_file.to_str().unwrap())
        .build()
        .await
        .expect("libsql open");
    let conn = db.connect().expect("libsql connect");
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .await
        .unwrap();
    conn.execute_batch(SCHEMA).await.unwrap();
    let _ = conn.execute_batch(FTS_SCHEMA).await;
    let _ = conn.execute_batch(FTS_TRIGGERS).await;
    let lint_freshness = Arc::new(
        crate::lint::snapshot::LintFreshnessClock::new(&db).expect("lint freshness observer"),
    );
    let memory_db = MemoryDB {
        _db: db,
        conn: Arc::new(tokio::sync::Mutex::new(conn)),
        entity_resolution_lock: tokio::sync::Mutex::new(()),
        space_write_lock: tokio::sync::Mutex::new(()),
        lint_freshness,
        page_projection_tracker: crate::page_projection_tracker::PageProjectionTracker::new(),
        derived_artifact_state: crate::derived_artifact_state::DerivedArtifactState::new(),
        embedder: None,
        chunker: build_chunker(Path::new(".nonexistent")),
        embedding_cache: std::sync::Mutex::new(EmbeddingCache::new(200)),
        community_dirty_nodes: std::sync::Mutex::new(BTreeMap::new()),
        community_grouping_runtime: std::sync::Mutex::new(
            crate::community_grouping::CommunityGroupingRuntime::default(),
        ),
        opened_fresh_file: std::sync::atomic::AtomicBool::new(true),
        skip_migration_backups: std::sync::atomic::AtomicBool::new(false),
    };
    match ceiling {
        Some(ceiling) => memory_db
            .run_migrations_up_to(&crate::events::NoopEmitter, ceiling)
            .await
            .unwrap(),
        None => memory_db
            .run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap(),
    }
    (memory_db, dir)
}

/// [`build_db`] at the current schema; shared with the lint and watcher
/// tests of the same feature.
pub(crate) async fn test_db() -> (MemoryDB, tempfile::TempDir) {
    build_db(None).await
}

async fn test_db_at(ceiling: i64) -> (MemoryDB, tempfile::TempDir) {
    build_db(Some(ceiling)).await
}

const T0: &str = "2026-01-01T00:00:00Z";
const SPACE: &str = "sp";

async fn page(db: &MemoryDB, id: &str, title: &str, space: &str) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        "The note body.",
        None,
        Some(space),
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
}

/// `n` numbered lines: `line 1`, `line 2`, ...
fn numbered(n: u32) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn root(&self) -> String {
        self.dir.path().to_str().unwrap().to_string()
    }
    fn write(&self, path: &str, text: &str) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    fn input(&self, path: &str, start: u32, end: u32) -> CodeBasisInput {
        CodeBasisInput {
            repo_root: self.root(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }
    fn locator(&self, path: &str, start: u32, end: u32) -> CodeBasisLocator {
        CodeBasisLocator {
            repo_root: self.root(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }
}

/// Write one code basis for `page_id` the way the daemon does for a create.
async fn rely(db: &MemoryDB, page_id: &str, inputs: &[CodeBasisInput]) {
    let prepared = prepare_code_basis(inputs).await.unwrap();
    db.upsert_page_code_basis(page_id, &prepared, "agent")
        .await
        .unwrap();
}

async fn rows(db: &MemoryDB, page_id: &str) -> Vec<CodeBasisRef> {
    db.list_page_code_basis(page_id).await.unwrap()
}

/// The only row of the note.
async fn only(db: &MemoryDB, page_id: &str) -> CodeBasisRef {
    let mut all = rows(db, page_id).await;
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

fn invalid(error: CodeBasisError) -> String {
    match error {
        CodeBasisError::Invalid(message) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Migration 139
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_139_adds_the_table_and_index_idempotently() {
    let (db, _dir) = test_db_at(138).await;
    {
        let conn = db.conn.lock().await;
        let mut found = conn
            .query(
                "SELECT name FROM sqlite_master WHERE name = 'page_code_basis'",
                (),
            )
            .await
            .unwrap();
        assert!(
            found.next().await.unwrap().is_none(),
            "the table must not exist before migration 139"
        );
    }
    for _ in 0..2 {
        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        let conn = db.conn.lock().await;
        let mut found = conn
            .query(
                "SELECT name FROM sqlite_master \
                 WHERE name IN ('page_code_basis','page_code_basis_by_page')",
                (),
            )
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(row) = found.next().await.unwrap() {
            names.push(row.get::<String>(0).unwrap());
        }
        names.sort();
        assert_eq!(names, vec!["page_code_basis", "page_code_basis_by_page"]);
        let mut version = conn.query("PRAGMA user_version", ()).await.unwrap();
        let user_version: i64 = version.next().await.unwrap().unwrap().get(0).unwrap();
        assert!(user_version >= 139, "user_version was {user_version}");
    }
}

#[tokio::test]
async fn migration_142_adds_cited_blob_to_a_table_that_lacks_it_and_is_a_no_op_otherwise() {
    // A database that stopped at 139 holds the table without the column.
    let (db, _dir) = test_db_at(139).await;
    let has_column = |db: &MemoryDB| {
        let conn = db.conn.clone();
        async move {
            let conn = conn.lock().await;
            let mut found = conn
                .query(
                    "SELECT COUNT(*) FROM pragma_table_info('page_code_basis') \
                     WHERE name = 'cited_blob'",
                    (),
                )
                .await
                .unwrap();
            found.next().await.unwrap().unwrap().get::<i64>(0).unwrap() > 0
        }
    };
    {
        // Put the table back in its 139 shape: 139 itself now creates it with
        // the column, so rebuild it without.
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "DROP TABLE page_code_basis; \
             CREATE TABLE page_code_basis ( \
                page_id TEXT NOT NULL, repo_root TEXT NOT NULL, path TEXT NOT NULL, \
                line_start INTEGER NOT NULL, line_end INTEGER NOT NULL, \
                anchor_json TEXT NOT NULL, confirmed_digest TEXT NOT NULL, \
                confirmed_by TEXT NOT NULL, confirmed_at INTEGER NOT NULL, \
                state TEXT NOT NULL CHECK(state IN ('ok','moved','pending','unavailable')), \
                unavailable_reason TEXT, changed_at INTEGER, file_len INTEGER, \
                file_mtime_ns INTEGER, added_by TEXT NOT NULL, added_at INTEGER NOT NULL, \
                PRIMARY KEY (page_id, repo_root, path, line_start, line_end)); \
             INSERT INTO page_code_basis VALUES \
                ('n1','/r','a.rs',1,2,'{}','d','agent',5,'ok',NULL,NULL,NULL,NULL,'agent',5);",
        )
        .await
        .unwrap();
    }
    assert!(!has_column(&db).await);
    for _ in 0..2 {
        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        assert!(has_column(&db).await);
    }
    let conn = db.conn.lock().await;
    let mut kept = conn
        .query("SELECT cited_blob FROM page_code_basis", ())
        .await
        .unwrap();
    let row = kept.next().await.unwrap().unwrap();
    assert_eq!(
        row.get::<Option<String>>(0).unwrap(),
        None,
        "old rows stay NULL"
    );
    let mut version = conn.query("PRAGMA user_version", ()).await.unwrap();
    let user_version: i64 = version.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(user_version, i64::from(crate::db::SCHEMA_VERSION));
}

#[tokio::test]
async fn migration_143_adds_the_recheck_columns_to_a_table_that_lacks_them_and_is_a_no_op_otherwise(
) {
    // A database that stopped at 142 holds the table without the four columns.
    let (db, _dir) = test_db_at(142).await;
    let columns = |db: &MemoryDB| {
        let conn = db.conn.clone();
        async move {
            let conn = conn.lock().await;
            let mut found = conn
                .query(
                    "SELECT name FROM pragma_table_info('page_code_basis') \
                     WHERE name IN ('recheck_note','recheck_verdict','recheck_digest', \
                                   'recheck_attempts') \
                     ORDER BY name",
                    (),
                )
                .await
                .unwrap();
            let mut names = Vec::new();
            while let Some(row) = found.next().await.unwrap() {
                names.push(row.get::<String>(0).unwrap());
            }
            names
        }
    };
    {
        // Put the table back in its 142 shape: 139 now creates it with the
        // columns, so rebuild it without them.
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "DROP TABLE page_code_basis; \
             CREATE TABLE page_code_basis ( \
                page_id TEXT NOT NULL, repo_root TEXT NOT NULL, path TEXT NOT NULL, \
                line_start INTEGER NOT NULL, line_end INTEGER NOT NULL, \
                anchor_json TEXT NOT NULL, confirmed_digest TEXT NOT NULL, \
                confirmed_by TEXT NOT NULL, confirmed_at INTEGER NOT NULL, \
                state TEXT NOT NULL CHECK(state IN ('ok','moved','pending','unavailable')), \
                unavailable_reason TEXT, changed_at INTEGER, file_len INTEGER, \
                file_mtime_ns INTEGER, cited_blob TEXT, added_by TEXT NOT NULL, \
                added_at INTEGER NOT NULL, \
                PRIMARY KEY (page_id, repo_root, path, line_start, line_end)); \
             INSERT INTO page_code_basis VALUES \
                ('n1','/r','a.rs',1,2,'{}','d','agent',5,'pending',NULL,5,NULL,NULL,NULL,'agent',5);",
        )
        .await
        .unwrap();
    }
    assert!(columns(&db).await.is_empty());
    for _ in 0..2 {
        db.run_migrations(&crate::events::NoopEmitter)
            .await
            .unwrap();
        assert_eq!(
            columns(&db).await,
            vec![
                "recheck_attempts",
                "recheck_digest",
                "recheck_note",
                "recheck_verdict"
            ]
        );
    }
    let conn = db.conn.lock().await;
    let mut kept = conn
        .query(
            "SELECT recheck_note, recheck_digest, recheck_attempts, state, recheck_verdict \
             FROM page_code_basis",
            (),
        )
        .await
        .unwrap();
    let row = kept.next().await.unwrap().unwrap();
    assert_eq!(row.get::<Option<String>>(0).unwrap(), None);
    assert_eq!(row.get::<Option<String>>(1).unwrap(), None);
    assert_eq!(
        row.get::<i64>(2).unwrap(),
        0,
        "old rows start at zero attempts"
    );
    assert_eq!(
        row.get::<String>(3).unwrap(),
        "pending",
        "the row itself is kept"
    );
    assert_eq!(row.get::<Option<String>>(4).unwrap(), None);
    let mut version = conn.query("PRAGMA user_version", ()).await.unwrap();
    let user_version: i64 = version.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(user_version, i64::from(crate::db::SCHEMA_VERSION));
    assert_eq!(user_version, 143);
}

#[tokio::test]
async fn a_fresh_database_has_the_recheck_columns_from_the_ddl() {
    let (db, _dir) = test_db().await;
    let conn = db.conn.lock().await;
    let mut found = conn
        .query(
            "SELECT COUNT(*) FROM pragma_table_info('page_code_basis') \
             WHERE name IN ('recheck_note','recheck_verdict','recheck_digest', \
                              'recheck_attempts')",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        found.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        4
    );
}

#[tokio::test]
async fn a_fresh_database_has_cited_blob_from_the_ddl() {
    let (db, _dir) = test_db().await;
    let conn = db.conn.lock().await;
    let mut found = conn
        .query(
            "SELECT COUNT(*) FROM pragma_table_info('page_code_basis') WHERE name = 'cited_blob'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        found.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
}

#[tokio::test]
async fn the_state_check_allows_the_four_states_and_nothing_else() {
    let (db, _dir) = test_db().await;
    let conn = db.conn.lock().await;
    for (state, allowed) in [
        ("ok", true),
        ("moved", true),
        ("pending", true),
        ("unavailable", true),
        ("archived", false),
        ("deleted", false),
    ] {
        let result = conn
            .execute(
                "INSERT OR REPLACE INTO page_code_basis \
                   (page_id,repo_root,path,line_start,line_end,anchor_json,confirmed_digest,\
                    confirmed_by,confirmed_at,state,added_by,added_at) \
                 VALUES ('x','/r','a.rs',1,2,'{}','d','human',1,?1,'human',1)",
                libsql::params![state],
            )
            .await;
        assert_eq!(result.is_ok(), allowed, "{state}");
    }
}

// ---------------------------------------------------------------------------
// Path safety (acceptance 6)
// ---------------------------------------------------------------------------

#[test]
fn resolve_code_path_accepts_a_plain_relative_path_under_an_absolute_root() {
    let resolved = resolve_code_path("/repo", "src/lib.rs").unwrap();
    assert_eq!(resolved, Path::new("/repo/src/lib.rs"));
    // A trailing slash on the root and a dotted file name are ordinary.
    assert!(resolve_code_path("/repo/", "a/.hidden.rs").is_ok());
    assert!(resolve_code_path("/repo", "a/b..c/d").is_ok());
}

#[test]
fn resolve_code_path_refuses_every_way_out_of_the_repository() {
    for (root, path) in [
        ("/repo", "../etc/passwd"),
        ("/repo", "src/../../etc/passwd"),
        ("/repo", "src/.."),
        ("/repo", "./src/lib.rs"),
        ("/repo", "src/./lib.rs"),
        ("/repo", "/etc/passwd"),
        ("/repo", "src\\lib.rs"),
        ("/repo", "..\\x"),
        ("/repo", "src//lib.rs"),
        ("/repo", "src/lib.rs/"),
        ("/repo", ""),
        ("/repo", "a\0b"),
        ("repo", "src/lib.rs"),
        ("", "src/lib.rs"),
        ("/repo/../etc", "passwd"),
        ("/repo/./x", "passwd"),
    ] {
        assert!(
            resolve_code_path(root, path).is_err(),
            "{root:?} + {path:?} must be refused"
        );
    }
}

#[tokio::test]
async fn a_write_with_a_bad_path_or_span_is_refused_with_the_reason() {
    let repo = Repo::new();
    repo.write("src/lib.rs", &numbered(30));

    for path in ["../x.rs", "/etc/passwd", "src\\lib.rs", "src/./lib.rs"] {
        let error = prepare_code_basis(&[repo.input(path, 1, 2)])
            .await
            .unwrap_err();
        assert!(invalid(error).contains("path"), "{path}");
    }

    // line_end before line_start, and line 0 (lines are 1-based).
    for (start, end) in [(20, 10), (0, 5)] {
        let message = invalid(
            prepare_code_basis(&[repo.input("src/lib.rs", start, end)])
                .await
                .unwrap_err(),
        );
        assert!(message.contains("selects no lines"), "{message}");
    }

    // Past the end of the file: the anchor's own message, with the line count.
    let message = invalid(
        prepare_code_basis(&[repo.input("src/lib.rs", 25, 40)])
            .await
            .unwrap_err(),
    );
    assert!(message.contains("outside the file (30 lines)"), "{message}");

    // A file that is not there fails the request; it never becomes a row.
    let message = invalid(
        prepare_code_basis(&[repo.input("src/gone.rs", 1, 2)])
            .await
            .unwrap_err(),
    );
    assert!(message.contains("file_missing"), "{message}");
    let relative = CodeBasisInput {
        repo_root: "relative/root".into(),
        path: "a.rs".into(),
        line_start: 1,
        line_end: 1,
    };
    assert!(prepare_code_basis(&[relative]).await.is_err());
}

#[tokio::test]
async fn a_write_may_carry_a_bounded_number_of_entries_and_duplicates_collapse() {
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    let same = repo.input("a.rs", 2, 4);
    let prepared = prepare_code_basis(&[same.clone(), same.clone(), repo.input("a.rs", 5, 6)])
        .await
        .unwrap();
    assert_eq!(prepared.len(), 2);

    let too_many: Vec<CodeBasisInput> = (0..=crate::code_basis::MAX_CODE_BASIS_PER_WRITE)
        .map(|_| repo.input("a.rs", 1, 2))
        .collect();
    // The cap counts what was sent, before duplicates collapse.
    assert!(prepare_code_basis(&too_many).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_may_not_lead_out_of_the_repository() {
    let repo = Repo::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "top secret\nline two\n").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        repo.dir.path().join("link.txt"),
    )
    .unwrap();
    let message = invalid(
        prepare_code_basis(&[repo.input("link.txt", 1, 1)])
            .await
            .unwrap_err(),
    );
    assert!(message.contains("outside_repo"), "{message}");

    // A link that stays inside is fine.
    repo.write("real.rs", &numbered(3));
    std::os::unix::fs::symlink(
        repo.dir.path().join("real.rs"),
        repo.dir.path().join("alias.rs"),
    )
    .unwrap();
    assert!(prepare_code_basis(&[repo.input("alias.rs", 1, 2)])
        .await
        .is_ok());
}

#[tokio::test]
async fn a_file_over_the_size_cap_is_too_large() {
    let repo = Repo::new();
    repo.write("big.txt", &"x".repeat(MAX_CODE_FILE_BYTES as usize + 1));
    let message = invalid(
        prepare_code_basis(&[repo.input("big.txt", 1, 1)])
            .await
            .unwrap_err(),
    );
    assert!(message.contains("too_large"), "{message}");
}

// ---------------------------------------------------------------------------
// Acceptance 1-5, 7, 8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accept_1_a_new_code_basis_reads_ok() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("src/lib.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("src/lib.rs", 10, 20)]).await;

    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!((row.line_start, row.line_end), (10, 20));
    assert_eq!(row.path, "src/lib.rs");
    assert_eq!(row.repo_root, repo.root());
    assert_eq!((row.changed_at, row.reason), (None, None));
    // Nothing to look at: the list/recall view is empty.
    assert!(db
        .list_non_ok_code_basis(&["n1".to_string()])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn accept_2_a_line_inserted_above_moves_the_span_and_the_brief_stays_quiet() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("src/lib.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("src/lib.rs", 10, 20)]).await;

    repo.write("src/lib.rs", &format!("// a new comment\n{}", numbered(40)));
    assert_eq!(
        brief_code_basis_pending(&db, SPACE).await.unwrap(),
        Some(0),
        "a move is not a notice"
    );

    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_MOVED);
    assert_eq!((row.line_start, row.line_end), (11, 21));
    assert_eq!(row.changed_at, None);

    // The row keeps its new place; it is not quietly turned `ok` by the next
    // check, and a person-facing recheck report does not list it.
    assert!(
        recheck_code_basis_in_scope(&db, &ReadScope::Space(SPACE.into()))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_MOVED);
    // Agents see it in the non-ok view.
    let non_ok = db
        .list_non_ok_code_basis(&["n1".to_string()])
        .await
        .unwrap();
    assert_eq!(non_ok["n1"][0].state, CODE_BASIS_STATE_MOVED);
}

#[tokio::test]
async fn a_moved_row_is_followed_to_its_next_place_and_confirm_makes_it_ok() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    repo.write("a.rs", &format!("// one\n{}", numbered(40)));
    recheck_code_basis(&db, None).await.unwrap();
    repo.write("a.rs", &format!("// one\n// two\n{}", numbered(40)));
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(
        (row.state.as_str(), row.line_start, row.line_end),
        (CODE_BASIS_STATE_MOVED, 12, 22)
    );

    let confirmed = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 12, 22), "agent")
        .await
        .unwrap();
    assert_eq!(confirmed.state, CODE_BASIS_STATE_OK);
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn accept_3_an_edited_line_makes_the_note_pending_and_the_lists_carry_it() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    page(&db, "n2", "Quiet", SPACE).await;
    let repo = Repo::new();
    repo.write("src/lib.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("src/lib.rs", 10, 20)]).await;
    rely(&db, "n2", &[repo.input("src/lib.rs", 30, 35)]).await;

    repo.write(
        "src/lib.rs",
        &numbered(40).replace("line 15\n", "line fifteen, rewritten\n"),
    );
    assert_eq!(brief_code_basis_pending(&db, SPACE).await.unwrap(), Some(1));

    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert!(row.changed_at.is_some());
    // The span it was confirmed with is kept, so confirm can still find it.
    assert_eq!((row.line_start, row.line_end), (10, 20));
    assert_eq!(only(&db, "n2").await.state, CODE_BASIS_STATE_OK);

    let non_ok = db
        .list_non_ok_code_basis(&["n1".to_string(), "n2".to_string()])
        .await
        .unwrap();
    assert_eq!(non_ok.len(), 1);
    assert_eq!(non_ok["n1"][0].state, CODE_BASIS_STATE_PENDING);

    let report = recheck_code_basis_in_scope(&db, &ReadScope::Space(SPACE.into()))
        .await
        .unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].page_id, "n1");
    assert_eq!(report[0].title, "Note");
    assert_eq!(report[0].pending.len(), 1);
}

#[tokio::test]
async fn a_pending_row_keeps_its_first_changed_at_and_returns_to_ok_when_the_file_is_restored() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    repo.write("a.rs", &numbered(40).replace("line 15\n", "other\n"));
    recheck_code_basis(&db, None).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE page_code_basis SET changed_at = 5", ())
            .await
            .unwrap();
    }
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(row.changed_at, Some(5), "the first sighting is kept");

    repo.write("a.rs", &numbered(40));
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(row.changed_at, None);
}

#[tokio::test]
async fn accept_4_confirm_returns_to_ok_with_the_anchor_of_the_new_content() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("src/lib.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("src/lib.rs", 10, 20)]).await;
    let before = db
        .get_code_basis_row("n1", &repo.locator("src/lib.rs", 10, 20))
        .await
        .unwrap()
        .unwrap();

    let edited = numbered(40).replace("line 15\n", "line fifteen, rewritten\n");
    repo.write("src/lib.rs", &edited);
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);

    let confirmed = confirm_code_basis(&db, "n1", &repo.locator("src/lib.rs", 10, 20), "agent")
        .await
        .unwrap();
    assert_eq!(confirmed.state, CODE_BASIS_STATE_OK);
    assert_eq!((confirmed.line_start, confirmed.line_end), (10, 20));

    let after = db
        .get_code_basis_row("n1", &repo.locator("src/lib.rs", 10, 20))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(after.anchor.digest, before.anchor.digest);
    let expected = code_anchor::anchor(&edited, LineSpan { start: 10, end: 20 }).unwrap();
    assert_eq!(
        after.anchor, expected,
        "anchored on the file as it reads now"
    );
    assert_eq!(after.reference.state, CODE_BASIS_STATE_OK);

    // And a later check agrees.
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn confirm_follows_a_changed_block_that_also_moved() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    // Two lines inserted above, one line inside the block edited.
    let edited = format!(
        "// x\n// y\n{}",
        numbered(40).replace("line 15\n", "line fifteen\n")
    );
    repo.write("a.rs", &edited);
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);

    let confirmed = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 10, 20), "agent")
        .await
        .unwrap();
    assert_eq!((confirmed.line_start, confirmed.line_end), (12, 22));
    assert_eq!(only(&db, "n1").await.line_start, 12);
}

#[tokio::test]
async fn confirm_refuses_an_unknown_row_and_an_unreadable_file() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    let unknown = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 1, 2), "agent").await;
    assert!(
        matches!(unknown, Err(CodeBasisError::NotFound)),
        "{unknown:?}"
    );
    let other_note = confirm_code_basis(&db, "nope", &repo.locator("a.rs", 10, 20), "agent").await;
    assert!(matches!(other_note, Err(CodeBasisError::NotFound)));

    std::fs::remove_file(repo.dir.path().join("a.rs")).unwrap();
    let gone = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 10, 20), "agent").await;
    assert!(matches!(gone, Err(CodeBasisError::Conflict(_))), "{gone:?}");
    // The row was not touched by the failed confirm.
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn confirm_of_lines_that_cannot_be_found_conflicts_and_changes_nothing() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    let before = db
        .get_code_basis_row("n1", &repo.locator("a.rs", 10, 20))
        .await
        .unwrap()
        .unwrap();

    // The whole file is rewritten: nothing fences the block any more, and the
    // stored line numbers now hold unrelated text. Confirming must not bless it.
    let rewritten: String = (1..=40).map(|i| format!("fresh {i}\n")).collect();
    repo.write("a.rs", &rewritten);
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);
    let conflict = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 10, 20), "agent").await;
    match conflict {
        Err(CodeBasisError::Conflict(message)) => assert!(
            message.contains("lines cannot be located; rewrite the note or remove this reliance"),
            "{message}"
        ),
        other => panic!("expected a conflict, got {other:?}"),
    }
    let after = db
        .get_code_basis_row("n1", &repo.locator("a.rs", 10, 20))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.reference.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(after.anchor, before.anchor, "the anchor was not replaced");

    // Now the file shrinks below the span: also a conflict.
    repo.write("a.rs", "only\ntwo\n");
    recheck_code_basis(&db, None).await.unwrap();
    let conflict = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 10, 20), "agent").await;
    assert!(
        matches!(conflict, Err(CodeBasisError::Conflict(_))),
        "{conflict:?}"
    );
}

#[tokio::test]
async fn confirm_does_not_delete_another_row_at_the_span_it_moves_to() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(
        &db,
        "n1",
        &[repo.input("a.rs", 10, 20), repo.input("a.rs", 12, 22)],
    )
    .await;

    // Two lines go in above. Confirming the first relocates it onto 12-22,
    // where the second still sits (no check has run to move that one on).
    repo.write("a.rs", &format!("// x\n// y\n{}", numbered(40)));
    let conflict = confirm_code_basis(&db, "n1", &repo.locator("a.rs", 10, 20), "agent").await;
    match conflict {
        Err(CodeBasisError::Conflict(message)) => {
            assert!(message.contains("already covers"), "{message}")
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    let mut spans: Vec<(u32, u32)> = rows(&db, "n1")
        .await
        .iter()
        .map(|r| (r.line_start, r.line_end))
        .collect();
    spans.sort_unstable();
    assert_eq!(spans, vec![(10, 20), (12, 22)], "both reliances survive");
}

#[tokio::test]
async fn accept_5_a_deleted_file_is_unavailable_and_counted_in_the_brief() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("src/lib.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("src/lib.rs", 10, 20)]).await;

    std::fs::remove_file(repo.dir.path().join("src/lib.rs")).unwrap();
    assert_eq!(brief_code_basis_pending(&db, SPACE).await.unwrap(), Some(1));
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_UNAVAILABLE);
    assert_eq!(row.reason.as_deref(), Some("file_missing"));

    // The whole repository gone reads as such.
    let root = repo.root();
    drop(repo);
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_UNAVAILABLE);
    assert_eq!(row.reason.as_deref(), Some("repo_missing"), "{root}");
}

#[tokio::test]
async fn an_unavailable_row_heals_when_the_file_comes_back() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    std::fs::remove_file(repo.dir.path().join("a.rs")).unwrap();
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_UNAVAILABLE);

    repo.write("a.rs", &numbered(40));
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(row.reason, None);
}

#[tokio::test]
async fn accept_7_crlf_and_trailing_spaces_alone_change_nothing() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    let crlf: String = numbered(40)
        .lines()
        .map(|line| format!("{line}   \r\n"))
        .collect();
    repo.write("a.rs", &crlf);
    assert_eq!(brief_code_basis_pending(&db, SPACE).await.unwrap(), Some(0));
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!((row.line_start, row.line_end), (10, 20));
}

#[tokio::test]
async fn a_whitespace_only_edit_leaves_the_row_ok() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    // Re-indent the whole file, add trailing spaces, respace inside a line.
    let edited: String = numbered(40)
        .lines()
        .map(|line| format!("    {}  \n", line.replace(' ', "   ")))
        .collect();
    repo.write("a.rs", &edited);
    assert_eq!(brief_code_basis_pending(&db, SPACE).await.unwrap(), Some(0));
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(row.reason, None);
    assert_eq!((row.line_start, row.line_end), (10, 20));

    // A real edit in the same re-indented file is still pending.
    repo.write("a.rs", &edited.replace("line   15", "line   fifteen"));
    assert_eq!(brief_code_basis_pending(&db, SPACE).await.unwrap(), Some(1));
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);
}

#[tokio::test]
async fn accept_8_twenty_notes_on_one_span_are_all_pending_after_one_recheck() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    for i in 0..20 {
        let id = format!("n{i}");
        page(&db, &id, &format!("Note {i}"), SPACE).await;
        rely(&db, &id, &[repo.input("a.rs", 10, 20)]).await;
    }
    repo.write("a.rs", &numbered(40).replace("line 12\n", "changed\n"));

    let report = recheck_code_basis_in_scope(&db, &ReadScope::Space(SPACE.into()))
        .await
        .unwrap();
    assert_eq!(report.len(), 20);
    for entry in &report {
        assert_eq!(entry.pending.len(), 1);
        assert_eq!(entry.pending[0].state, CODE_BASIS_STATE_PENDING);
    }
    assert_eq!(
        db.count_pages_with_code_basis_pending(SPACE).await.unwrap(),
        20
    );
}

// ---------------------------------------------------------------------------
// Scope, lifecycle and write semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_check_covers_the_notes_of_its_space_only() {
    let (db, _dir) = test_db().await;
    page(&db, "in", "In", SPACE).await;
    page(&db, "out", "Out", "elsewhere").await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "in", &[repo.input("a.rs", 10, 20)]).await;
    rely(&db, "out", &[repo.input("a.rs", 10, 20)]).await;
    repo.write("a.rs", &numbered(40).replace("line 12\n", "changed\n"));

    let report = recheck_code_basis_in_scope(&db, &ReadScope::Space(SPACE.into()))
        .await
        .unwrap();
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].page_id, "in");
    assert_eq!(
        only(&db, "out").await.state,
        CODE_BASIS_STATE_OK,
        "untouched"
    );
    assert!(db.space_has_code_basis(SPACE).await.unwrap());
    assert!(!db.space_has_code_basis("empty-space").await.unwrap());
    assert_eq!(
        brief_code_basis_pending(&db, "empty-space").await.unwrap(),
        None,
        "a Space with no code bases reads nothing"
    );
}

#[tokio::test]
async fn a_write_adds_and_re_anchors_but_never_drops_the_spans_it_does_not_name() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(60));
    rely(
        &db,
        "n1",
        &[repo.input("a.rs", 1, 5), repo.input("a.rs", 10, 20)],
    )
    .await;

    // The first span's content changes; a later write that names only the
    // second span leaves the first as it is (pending).
    repo.write("a.rs", &numbered(60).replace("line 3\n", "changed\n"));
    recheck_code_basis(&db, None).await.unwrap();
    rely(&db, "n1", &[repo.input("a.rs", 30, 40)]).await;
    let all = rows(&db, "n1").await;
    assert_eq!(all.len(), 3);
    let by_start = |start: u32| {
        all.iter()
            .find(|r| r.line_start == start)
            .unwrap()
            .state
            .clone()
    };
    assert_eq!(by_start(1), CODE_BASIS_STATE_PENDING);
    assert_eq!(by_start(10), CODE_BASIS_STATE_OK);
    assert_eq!(by_start(30), CODE_BASIS_STATE_OK);

    // Naming the pending span again re-anchors it and returns it to ok.
    rely(&db, "n1", &[repo.input("a.rs", 1, 5)]).await;
    assert_eq!(
        rows(&db, "n1")
            .await
            .iter()
            .find(|r| r.line_start == 1)
            .unwrap()
            .state,
        CODE_BASIS_STATE_OK
    );
}

/// A note citing `a.rs` 10-20 whose file then gained a line, so the row is
/// `pending`. The file now has 41 lines.
async fn a_pending_row_at_10_20(db: &MemoryDB, repo: &Repo) {
    repo.write("a.rs", &numbered(40));
    rely(db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    repo.write(
        "a.rs",
        &numbered(40).replace("line 15\n", "line 15\nline 15b\n"),
    );
    recheck_code_basis(db, None).await.unwrap();
    assert_eq!(only(db, "n1").await.state, CODE_BASIS_STATE_PENDING);
}

#[tokio::test]
async fn a_write_naming_a_changed_span_replaces_the_stale_row_it_overlaps() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    a_pending_row_at_10_20(&db, &repo).await;

    rely(&db, "n1", &[repo.input("a.rs", 10, 21)]).await;
    let row = only(&db, "n1").await;
    assert_eq!((row.line_start, row.line_end), (10, 21));
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(brief_code_basis_pending(&db, SPACE).await.unwrap(), Some(0));
}

#[tokio::test]
async fn a_write_naming_a_span_that_does_not_overlap_leaves_the_stale_row() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    a_pending_row_at_10_20(&db, &repo).await;

    rely(&db, "n1", &[repo.input("a.rs", 30, 40)]).await;
    let all = rows(&db, "n1").await;
    assert_eq!(all.len(), 2, "{all:?}");
    let state_at = |start: u32| {
        all.iter()
            .find(|r| r.line_start == start)
            .unwrap()
            .state
            .clone()
    };
    assert_eq!(state_at(10), CODE_BASIS_STATE_PENDING);
    assert_eq!(state_at(30), CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn a_write_never_replaces_an_ok_row_even_when_the_spans_overlap() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    rely(&db, "n1", &[repo.input("a.rs", 10, 21)]).await;
    let all = rows(&db, "n1").await;
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(
        all.iter().all(|r| r.state == CODE_BASIS_STATE_OK),
        "{all:?}"
    );
}

#[tokio::test]
async fn a_write_replaces_an_unavailable_row_once_the_file_is_back() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    std::fs::remove_file(repo.dir.path().join("a.rs")).unwrap();
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_UNAVAILABLE);

    repo.write("a.rs", &numbered(41));
    rely(&db, "n1", &[repo.input("a.rs", 10, 21)]).await;
    let row = only(&db, "n1").await;
    assert_eq!((row.line_start, row.line_end), (10, 21));
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn only_an_active_note_can_rely_on_code() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(10));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 1, 2)])
        .await
        .unwrap();

    let missing = db.upsert_page_code_basis("ghost", &prepared, "agent").await;
    assert!(
        matches!(missing, Err(CodeBasisError::Invalid(_))),
        "{missing:?}"
    );

    page(&db, "archived", "Old", SPACE).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET status = 'archived' WHERE id = 'archived'",
            (),
        )
        .await
        .unwrap();
    }
    let archived = db
        .upsert_page_code_basis("archived", &prepared, "agent")
        .await;
    assert!(matches!(archived, Err(CodeBasisError::Invalid(_))));
    assert!(rows(&db, "archived").await.is_empty());
}

#[tokio::test]
async fn remove_deletes_the_row_and_says_whether_there_was_one() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    assert!(!db
        .remove_code_basis("n1", &repo.locator("a.rs", 1, 2))
        .await
        .unwrap());
    assert!(db
        .remove_code_basis("n1", &repo.locator("a.rs", 10, 20))
        .await
        .unwrap());
    assert!(rows(&db, "n1").await.is_empty());
    assert!(!db
        .remove_code_basis("n1", &repo.locator("a.rs", 10, 20))
        .await
        .unwrap());
}

#[tokio::test]
async fn accept_9_deleting_a_page_removes_its_code_bases() {
    let (db, _dir) = test_db().await;
    page(&db, "gone", "Gone", SPACE).await;
    page(&db, "kept", "Kept", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "gone", &[repo.input("a.rs", 10, 20)]).await;
    rely(&db, "kept", &[repo.input("a.rs", 10, 20)]).await;

    db.delete_page("gone").await.unwrap();
    assert!(rows(&db, "gone").await.is_empty());
    assert_eq!(rows(&db, "kept").await.len(), 1);
    let conn = db.conn.lock().await;
    let mut count = conn
        .query(
            "SELECT COUNT(*) FROM page_code_basis WHERE page_id = 'gone'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        count.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

#[tokio::test]
async fn a_check_that_raced_a_write_does_not_undo_it() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    let stale = db.list_code_basis_rows(None).await.unwrap();

    // The note is written again against changed lines (new anchor, `ok`) ...
    repo.write("a.rs", &numbered(40).replace("line 12\n", "changed\n"));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    // ... and only then does the verdict computed from the old anchor arrive.
    let verdicts: Vec<_> = stale
        .iter()
        .map(|row| (row, crate::db::CodeBasisVerdict::Pending))
        .collect();
    db.apply_code_basis_verdicts(&verdicts, 99).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn a_moved_row_that_lands_on_a_span_the_note_already_has_replaces_it() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    // Two entries; the first is about to move onto the place the second holds,
    // and the second moves on in the same check.
    rely(
        &db,
        "n1",
        &[repo.input("a.rs", 10, 20), repo.input("a.rs", 11, 21)],
    )
    .await;
    repo.write("a.rs", &format!("// new\n{}", numbered(40)));
    recheck_code_basis(&db, None).await.unwrap();
    // 11-21 moved to 12-22 first, which freed the place 10-20 moved into.
    let all = rows(&db, "n1").await;
    assert!(
        all.iter().all(|r| r.state == CODE_BASIS_STATE_MOVED),
        "{all:?}"
    );
    let mut spans: Vec<(u32, u32)> = all.iter().map(|r| (r.line_start, r.line_end)).collect();
    spans.sort_unstable();
    spans.dedup();
    assert_eq!(spans, vec![(11, 21), (12, 22)], "both reliances survive");
}

// ---------------------------------------------------------------------------
// Riding a page write
// ---------------------------------------------------------------------------

async fn edit_with(
    db: &MemoryDB,
    id: &str,
    content: &str,
    basis: &BasisWrite,
    version: i64,
) -> bool {
    db.try_update_page_content_with_changelog_at_version_and_basis(
        id,
        content,
        &[],
        "agent_refresh",
        "",
        None,
        version,
        None,
        basis,
    )
    .await
    .unwrap()
    .is_some()
}

#[tokio::test]
async fn code_bases_land_in_the_page_writes_own_transaction() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 10, 20)])
        .await
        .unwrap();
    let basis = BasisWrite {
        actor: "agent".into(),
        code_basis: prepared,
        ..BasisWrite::default()
    };

    // A write against a version the note is no longer at lands nothing, and
    // neither does its code basis.
    assert!(!edit_with(&db, "n1", "New body, stale.", &basis, 99).await);
    assert!(rows(&db, "n1").await.is_empty());

    // The write that lands carries it.
    assert!(edit_with(&db, "n1", "New body.", &basis, 1).await);
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!((row.line_start, row.line_end), (10, 20));
}

// ---------------------------------------------------------------------------
// Wire/old-data safety
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_row_with_an_unreadable_anchor_is_left_out_of_checks_not_mislabelled() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO page_code_basis \
               (page_id,repo_root,path,line_start,line_end,anchor_json,confirmed_digest,\
                confirmed_by,confirmed_at,state,added_by,added_at) \
             VALUES ('n1','/r','a.rs',1,2,'not json','d','human',1,'ok','human',1)",
            (),
        )
        .await
        .unwrap();
    }
    assert!(db.list_code_basis_rows(None).await.unwrap().is_empty());
    // It still reads as the row it is.
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn a_merge_carries_the_absorbed_notes_code_bases_to_the_survivor() {
    let (db, _dir) = test_db().await;
    page(&db, "survivor", "Survivor", SPACE).await;
    page(&db, "absorbed", "Absorbed", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    repo.write("b.rs", &numbered(40));
    rely(
        &db,
        "absorbed",
        &[repo.input("a.rs", 1, 5), repo.input("b.rs", 3, 9)],
    )
    .await;
    // The survivor already relies on one of the same spans, confirmed by it.
    rely(&db, "survivor", &[repo.input("a.rs", 1, 5)]).await;
    assert_eq!(rows(&db, "survivor").await.len(), 1);

    {
        let conn = db.conn.lock().await;
        super::code_basis::carry_code_basis_into_survivor(&conn, "absorbed", "survivor")
            .await
            .unwrap();
    }

    let mut carried = rows(&db, "survivor").await;
    carried.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(carried.len(), 2, "{carried:?}");
    assert_eq!(carried[0].path, "a.rs");
    assert_eq!(carried[1].path, "b.rs");
    // The absorbed note keeps its own rows; it is archived by the merge.
    assert_eq!(rows(&db, "absorbed").await.len(), 2);
}

#[tokio::test]
async fn a_source_note_that_changes_id_takes_its_code_bases_along() {
    let (db, _dir) = test_db().await;
    page(&db, "old", "Old id", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(
        &db,
        "old",
        &[repo.input("a.rs", 2, 6), repo.input("a.rs", 10, 12)],
    )
    .await;
    {
        let conn = db.conn.lock().await;
        super::code_basis::retarget_code_basis(&conn, "old", "new")
            .await
            .unwrap();
    }
    assert!(rows(&db, "old").await.is_empty());
    assert_eq!(rows(&db, "new").await.len(), 2);
}

// ---------------------------------------------------------------------------
// Review fixes: path-resolution reasons, and the stat short-circuit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invalid_repo_root_reads_repo_missing_and_only_an_escaping_path_reads_outside_repo() {
    use crate::code_basis::{read_code_file, CodeFileProblem};
    let repo = Repo::new();
    repo.write("a.rs", &numbered(3));
    for root in ["relative/root", "", "/tmp/../etc"] {
        assert_eq!(
            read_code_file(root, "a.rs").await.unwrap_err(),
            CodeFileProblem::RepoMissing,
            "{root:?}"
        );
    }
    assert_eq!(
        read_code_file(&repo.root(), "../a.rs").await.unwrap_err(),
        CodeFileProblem::OutsideRepo
    );
    assert_eq!(
        read_code_file(&repo.root(), "nope.rs").await.unwrap_err(),
        CodeFileProblem::FileMissing
    );
    assert!(read_code_file(&repo.root(), "a.rs").await.is_ok());
}

/// Pin `path`'s modification time to `time`.
fn set_mtime(repo: &Repo, path: &str, time: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(repo.dir.path().join(path))
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn mtime(repo: &Repo, path: &str) -> std::time::SystemTime {
    std::fs::metadata(repo.dir.path().join(path))
        .unwrap()
        .modified()
        .unwrap()
}

#[tokio::test]
async fn a_check_skips_a_file_whose_length_and_mtime_are_unchanged() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    let stored = db
        .get_code_basis_row("n1", &repo.locator("a.rs", 10, 20))
        .await
        .unwrap()
        .unwrap();
    assert!(stored.stamp.is_some(), "a write records the file's stamp");
    let original = mtime(&repo, "a.rs");

    // Same length, same mtime, different text: only a skipped read can leave
    // the row `ok`.
    repo.write("a.rs", &numbered(40).replace("line 15\n", "lXne 15\n"));
    set_mtime(&repo, "a.rs", original);
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);

    // A new mtime reads the file again and sees the edit.
    set_mtime(&repo, "a.rs", original + std::time::Duration::from_secs(10));
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);

    // A different length is read too, even with the old mtime.
    let (db, _dir) = test_db().await;
    page(&db, "n2", "Note", SPACE).await;
    rely(&db, "n2", &[repo.input("a.rs", 10, 20)]).await;
    let stamp = mtime(&repo, "a.rs");
    repo.write("a.rs", &numbered(40).replace("line 15\n", "line fifteen\n"));
    set_mtime(&repo, "a.rs", stamp);
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n2").await.state, CODE_BASIS_STATE_PENDING);
}

#[tokio::test]
async fn a_recheck_refreshes_the_stamp_so_the_next_one_can_skip() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;

    // Touched but identical: read once, still `ok`, stamp now the new one.
    let later = mtime(&repo, "a.rs") + std::time::Duration::from_secs(30);
    set_mtime(&repo, "a.rs", later);
    recheck_code_basis(&db, None).await.unwrap();
    let row = db
        .get_code_basis_row("n1", &repo.locator("a.rs", 10, 20))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.reference.state, CODE_BASIS_STATE_OK);
    let expected = {
        let m = std::fs::metadata(repo.dir.path().join("a.rs")).unwrap();
        m.modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64
    };
    assert_eq!(row.stamp.unwrap().mtime_ns, expected);
}

#[tokio::test]
async fn a_pending_row_is_always_read_even_when_the_file_looks_untouched() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    let original = mtime(&repo, "a.rs");
    repo.write("a.rs", &numbered(40).replace("line 15\n", "lXne 15\n"));
    set_mtime(&repo, "a.rs", original + std::time::Duration::from_secs(5));
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);

    // Reverted with the stamp the pending check saw: the row must not be stuck.
    repo.write("a.rs", &numbered(40));
    set_mtime(&repo, "a.rs", original + std::time::Duration::from_secs(5));
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

/// The `.md` projection never lists a note's code bases; only the OKF bundle
/// export does. The rows change outside page writes (a create stores them after
/// the projection, confirm and remove edit them, a recheck moves or deletes
/// them), so a `repo://` line in the projected file would go stale and the file
/// would read as hand-edited, which refuses the next rename.
#[tokio::test]
async fn the_md_projection_of_a_note_with_a_code_basis_has_no_repo_line() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 20)]).await;
    let note = db.get_page("n1").await.unwrap().unwrap();

    let root = tempfile::tempdir().unwrap();
    let writer = crate::export::knowledge::KnowledgeWriter::new(root.path().to_path_buf(), &db);
    let guard = db.begin_page_projection_write();
    let file = writer.write_page(&guard, &note).unwrap();
    let projected = std::fs::read_to_string(&file).unwrap();
    assert!(!projected.contains("repo://"), "{projected}");
    assert!(!projected.contains(&repo.root()), "{projected}");
    // Byte-identical to what a page with no code basis renders.
    assert_eq!(
        projected,
        crate::export::knowledge::render_markdown_for(&note)
    );
}

// ---------------------------------------------------------------------------
// Cited blob
// ---------------------------------------------------------------------------

async fn cited(db: &MemoryDB, page_id: &str, locator: &CodeBasisLocator) -> Option<String> {
    db.get_code_basis_cited(page_id, locator)
        .await
        .unwrap()
        .expect("the row exists")
        .cited_blob
}

#[tokio::test]
async fn a_write_keeps_the_blob_id_of_the_file_and_a_confirm_after_an_edit_replaces_it() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    let original = numbered(40);
    repo.write("src/lib.rs", &original);
    rely(&db, "n1", &[repo.input("src/lib.rs", 10, 12)]).await;
    let locator = repo.locator("src/lib.rs", 10, 12);

    let stored = db
        .get_code_basis_cited("n1", &locator)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.cited_blob.as_deref(),
        Some(git_blob_id(original.as_bytes()).as_str()),
        "the whole file, not just the cited lines"
    );
    assert_eq!(stored.state, CODE_BASIS_STATE_OK);
    assert!(stored.confirmed_at > 0);

    let edited = original.replace("line 11\n", "line eleven, rewritten\n");
    repo.write("src/lib.rs", &edited);
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(
        cited(&db, "n1", &locator).await,
        stored.cited_blob,
        "a pending row still points at the version it was confirmed against"
    );

    confirm_code_basis(&db, "n1", &locator, "agent")
        .await
        .unwrap();
    let after = db
        .get_code_basis_cited("n1", &locator)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.cited_blob.as_deref(),
        Some(git_blob_id(edited.as_bytes()).as_str())
    );
    assert_ne!(after.cited_blob, stored.cited_blob);
    assert_eq!(after.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn the_blob_id_is_taken_over_the_raw_bytes_not_the_lossy_text() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    let mut raw = numbered(5).into_bytes();
    raw.extend_from_slice(&[0xff, 0xfe, b'\n']);
    std::fs::write(Path::new(&repo.root()).join("bin.rs"), &raw).unwrap();
    rely(&db, "n1", &[repo.input("bin.rs", 1, 2)]).await;
    assert_eq!(
        cited(&db, "n1", &repo.locator("bin.rs", 1, 2))
            .await
            .as_deref(),
        Some(git_blob_id(&raw).as_str())
    );
}

#[tokio::test]
async fn a_recheck_that_marks_a_row_pending_or_unavailable_leaves_the_blob_id() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 12)]).await;
    let locator = repo.locator("a.rs", 10, 12);
    let before = cited(&db, "n1", &locator).await;
    assert!(before.is_some());

    repo.write("a.rs", &numbered(40).replace("line 11\n", "changed\n"));
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(cited(&db, "n1", &locator).await, before);

    std::fs::remove_file(Path::new(&repo.root()).join("a.rs")).unwrap();
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_UNAVAILABLE);
    assert_eq!(cited(&db, "n1", &locator).await, before);
}

#[tokio::test]
async fn a_moved_row_keeps_its_blob_id() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 12)]).await;
    let before = cited(&db, "n1", &repo.locator("a.rs", 10, 12)).await;

    repo.write("a.rs", &format!("// x\n// y\n{}", numbered(40)));
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_MOVED);
    assert_eq!(
        cited(&db, "n1", &repo.locator("a.rs", 12, 14)).await,
        before
    );
}

#[tokio::test]
async fn cited_is_absent_for_an_unknown_row_and_null_for_an_old_one() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "n1", &[repo.input("a.rs", 10, 12)]).await;

    assert!(db
        .get_code_basis_cited("n1", &repo.locator("a.rs", 1, 2))
        .await
        .unwrap()
        .is_none());
    assert!(db
        .get_code_basis_cited("nope", &repo.locator("a.rs", 10, 12))
        .await
        .unwrap()
        .is_none());

    // A row written before the blob id was kept reads as null, not as an error.
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE page_code_basis SET cited_blob = NULL", ())
            .await
            .unwrap();
    }
    let old = db
        .get_code_basis_cited("n1", &repo.locator("a.rs", 10, 12))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.cited_blob, None);
}

#[tokio::test]
async fn a_merge_carries_the_blob_id_to_the_survivor() {
    let (db, _dir) = test_db().await;
    page(&db, "survivor", "Survivor", SPACE).await;
    page(&db, "absorbed", "Absorbed", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    rely(&db, "absorbed", &[repo.input("a.rs", 3, 4)]).await;
    let expected = cited(&db, "absorbed", &repo.locator("a.rs", 3, 4)).await;
    assert!(expected.is_some());
    {
        let conn = db.conn.lock().await;
        super::code_basis::carry_code_basis_into_survivor(&conn, "absorbed", "survivor")
            .await
            .unwrap();
    }
    assert_eq!(
        cited(&db, "survivor", &repo.locator("a.rs", 3, 4)).await,
        expected
    );
}

async fn version_of(db: &MemoryDB, page_id: &str) -> i64 {
    db.get_page(page_id).await.unwrap().unwrap().version
}

/// A write that changed nothing records the lines it relies on in the same
/// transaction as its note marks, against the version it compared itself to.
#[tokio::test]
async fn an_unchanged_write_records_its_code_basis_with_its_marks() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", SPACE).await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 10, 20)])
        .await
        .unwrap();
    let at = version_of(&db, "n1").await;

    db.mark_page_bases("n1", at, &[("a".into(), 1)], &prepared, "agent", None)
        .await
        .unwrap();

    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(db.list_page_basis("n1").await.unwrap().len(), 1);
}

/// An edit that landed after the unchanged write compared itself refuses the
/// code basis exactly as it refuses the marks.
#[tokio::test]
async fn an_unchanged_write_records_no_code_basis_when_the_note_changed_since() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 10, 20)])
        .await
        .unwrap();
    let compared = version_of(&db, "n1").await;
    db.conn
        .lock()
        .await
        .execute("UPDATE pages SET version = version + 1 WHERE id = 'n1'", ())
        .await
        .unwrap();

    let error = db
        .mark_page_bases("n1", compared, &[], &prepared, "agent", None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, super::BasisError::CiterMovedOn { current } if current == compared + 1),
        "{error:?}"
    );
    assert!(rows(&db, "n1").await.is_empty());
}

/// One transaction: a refused note mark leaves no code basis behind.
#[tokio::test]
async fn a_refused_mark_on_an_unchanged_write_records_no_code_basis() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 10, 20)])
        .await
        .unwrap();
    let at = version_of(&db, "n1").await;

    let error = db
        .mark_page_bases("n1", at, &[("missing".into(), 1)], &prepared, "agent", None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, super::BasisError::BasisNotFound),
        "{error:?}"
    );
    assert!(rows(&db, "n1").await.is_empty());
}

/// A Space-locked agent's unchanged write records no code basis on a note that
/// left its Space.
#[tokio::test]
async fn an_unchanged_write_records_no_code_basis_on_a_note_outside_the_space() {
    let (db, _dir) = test_db().await;
    page(&db, "n1", "Note", SPACE).await;
    let repo = Repo::new();
    repo.write("a.rs", &numbered(40));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 10, 20)])
        .await
        .unwrap();
    let at = version_of(&db, "n1").await;
    let elsewhere = ReadScope::Space("elsewhere".into());

    let error = db
        .mark_page_bases("n1", at, &[], &prepared, "agent", Some(&elsewhere))
        .await
        .unwrap_err();
    assert!(
        matches!(error, super::BasisError::CiterNotFound),
        "{error:?}"
    );
    assert!(rows(&db, "n1").await.is_empty());
}
