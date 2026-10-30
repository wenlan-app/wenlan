// SPDX-License-Identifier: Apache-2.0

use super::super::tests::{test_db, test_db_at};
use super::{CreateEdgeOutcome, CreateNodeOutcome, EdgePatch, MemoryDB, NodeLayout, NodePatch};

async fn seed_page(db: &MemoryDB, page_id: &str) {
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO pages (id, title, content, created_at, last_compiled, last_modified) \
         VALUES (?1, 'Test Page', 'body', datetime('now'), datetime('now'), datetime('now'))",
        libsql::params![page_id],
    )
    .await
    .unwrap();
}

fn root_id(nodes: &[super::PageMapNode]) -> String {
    nodes
        .iter()
        .find(|n| n.parent_id.is_none())
        .expect("root node")
        .id
        .clone()
}

#[tokio::test]
async fn init_page_map_idempotent() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;

    let (first, created_first) = db.init_page_map("page-1").await.unwrap();
    assert!(created_first, "first call must report created = true");
    assert_eq!(first.map.revision, 1);
    assert_eq!(first.nodes.len(), 1);
    let root = &first.nodes[0];
    assert!(root.parent_id.is_none());
    assert_eq!(root.ref_kind, "page");
    assert_eq!(root.ref_id, "page-1");
    assert_eq!(root.fingerprint, "page\u{1f}page-1\u{1f}~");

    // Second call is a no-op: same revision, same single root, no new row.
    let (second, created_second) = db.init_page_map("page-1").await.unwrap();
    assert!(!created_second, "second call must report created = false");
    assert_eq!(second.map.revision, 1);
    assert_eq!(second.nodes.len(), 1);
    assert_eq!(second.nodes[0].id, root.id);
}

#[tokio::test]
async fn page_map_mutations_reject_non_active_pages_at_the_core_boundary() {
    let (db, _tmp) = test_db().await;
    let draft = db
        .create_page_draft("Draft", "Body", None, None)
        .await
        .unwrap();

    assert!(matches!(
        db.init_page_map(&draft.id).await,
        Err(crate::WenlanError::Validation(_))
    ));
    assert!(db.get_page_map(&draft.id, true).await.unwrap().is_none());

    seed_page(&db, "page-archived").await;
    let (map, _created) = db.init_page_map("page-archived").await.unwrap();
    let root = root_id(&map.nodes);
    db.archive_page("page-archived").await.unwrap();

    assert!(matches!(
        db.create_map_node(
            "page-archived",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await,
        Err(crate::WenlanError::Validation(_))
    ));
    assert!(matches!(
        db.reset_page_map("page-archived").await,
        Err(crate::WenlanError::Validation(_))
    ));
    assert!(db
        .get_page_map("page-archived", true)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn create_map_node_fingerprint_dedup() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let first = db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap();
    let created = match first {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    assert_eq!(created.fingerprint, "memory\u{1f}mem-1\u{1f}~");

    // Same (ref_kind, ref_id, parent) proposed again -> Duplicate, no revision bump.
    let revision_after_first = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let second = db
        .create_map_node(
            "page-1",
            revision_after_first,
            &root,
            "memory",
            "mem-1",
            None,
            1.0,
        )
        .await
        .unwrap();
    match second {
        CreateNodeOutcome::Duplicate(node) => assert_eq!(node.id, created.id),
        other => panic!("expected Duplicate, got {other:?}"),
    }
    let revision_after_second = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    assert_eq!(revision_after_second, revision_after_first);
}

// Mutation-proof note: this test only has teeth against the tombstone branch
// in `create_map_node` (`if existing.status == "dismissed" { Tombstoned }
// else { Duplicate(existing) }`). Flipping that branch to always return
// `Duplicate(existing)` makes this test fail on the `Tombstoned` assert
// below (the mutation was applied locally to confirm the failure, then
// reverted — it is not left in the shipped code).
#[tokio::test]
async fn dismissed_tombstone_blocks_reproposal() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let created = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-1", rev, &created.id)
        .await
        .unwrap();

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let outcome = db
        .create_map_node("page-1", rev, &root, "memory", "mem-1", None, 0.0)
        .await
        .unwrap();
    assert_eq!(outcome, CreateNodeOutcome::Tombstoned);

    // Tombstoned outcome must not have inserted a second row or bumped the revision.
    let after = db.get_page_map("page-1", true).await.unwrap().unwrap();
    assert_eq!(after.map.revision, rev);
    assert_eq!(
        after
            .nodes
            .iter()
            .filter(|n| n.fingerprint == "memory\u{1f}mem-1\u{1f}~")
            .count(),
        1
    );
}

#[tokio::test]
async fn revision_bumps_on_every_write_and_stale_base_rejected() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);
    let stale_revision = map.map.revision;

    db.create_map_node(
        "page-1",
        stale_revision,
        &root,
        "memory",
        "mem-1",
        None,
        0.0,
    )
    .await
    .unwrap();
    let bumped = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    assert_eq!(bumped, stale_revision + 1);

    // Reusing the now-stale base_revision must be rejected as a conflict.
    let err = db
        .create_map_node(
            "page-1",
            stale_revision,
            &root,
            "memory",
            "mem-2",
            None,
            0.0,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Conflict(_)));
}

#[tokio::test]
async fn root_invariants_reject_dismiss_delete_and_reparent() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let child = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db.delete_map_node("page-1", rev, &root).await.unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));

    let err = db
        .patch_map_node(
            "page-1",
            rev,
            &root,
            NodePatch {
                parent_id: Some(child.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

async fn page_content_and_version(db: &MemoryDB, page_id: &str) -> (String, i64) {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT content, version FROM pages WHERE id = ?1",
            libsql::params![page_id],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("seeded page");
    (row.get(0).unwrap(), row.get(1).unwrap())
}

async fn pragma_names(db: &MemoryDB, pragma: &str, column: i32) -> Vec<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(pragma, ()).await.unwrap();
    let mut values = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        values.push(row.get::<String>(column).unwrap());
    }
    values.sort();
    values
}

async fn db_user_version(db: &MemoryDB) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
    rows.next()
        .await
        .unwrap()
        .expect("user_version row")
        .get(0)
        .unwrap()
}

async fn db_foreign_keys_enabled(db: &MemoryDB) -> bool {
    let conn = db.conn.lock().await;
    let mut rows = conn.query("PRAGMA foreign_keys", ()).await.unwrap();
    rows.next()
        .await
        .unwrap()
        .expect("foreign_keys row")
        .get::<i64>(0)
        .unwrap()
        != 0
}

async fn page_history_has_title(db: &MemoryDB) -> bool {
    pragma_names(db, "PRAGMA table_info('page_history')", 1)
        .await
        .iter()
        .any(|column| column == "title")
}

async fn page_history_title(db: &MemoryDB, page_id: &str, version: i64) -> Option<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT title FROM page_history WHERE page_id=?1 AND version=?2",
            libsql::params![page_id, version],
        )
        .await
        .unwrap();
    rows.next()
        .await
        .unwrap()
        .expect("history row")
        .get::<Option<String>>(0)
        .unwrap()
}

async fn insert_history_without_title(
    db: &MemoryDB,
    page_id: &str,
    version: i64,
    content: &str,
    source_memory_ids: &str,
) {
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO page_history
             (page_id,version,content,source_memory_ids,edited_by,created_at)
         VALUES (?1,?2,?3,?4,'migration_134_fixture',1)",
        libsql::params![page_id, version, content, source_memory_ids],
    )
    .await
    .unwrap();
}

/// Build the private schema-133 map shape that shipped with the idea CHECK,
/// including one idea node. That lineage had no `page_history.title` column.
async fn install_private_133_page_map_ideas(db: &MemoryDB, page_id: &str, parent_id: &str) {
    let conn = db.conn.lock().await;
    conn.execute("PRAGMA foreign_keys = OFF", ()).await.unwrap();
    conn.execute_batch(
        "BEGIN IMMEDIATE;
        CREATE TABLE page_map_nodes__fixture (
            id TEXT PRIMARY KEY,
            page_id TEXT NOT NULL REFERENCES page_maps(page_id) ON DELETE CASCADE,
            parent_id TEXT REFERENCES page_map_nodes__fixture(id),
            rank REAL NOT NULL DEFAULT 0,
            ref_kind TEXT NOT NULL CHECK (ref_kind IN ('memory','entity','page','section','idea')),
            ref_id TEXT NOT NULL,
            label TEXT,
            status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('suggested','active','dismissed')),
            pinned INTEGER NOT NULL DEFAULT 0,
            placed INTEGER NOT NULL DEFAULT 0,
            collapsed INTEGER NOT NULL DEFAULT 0,
            x REAL, y REAL, width REAL, height REAL,
            fingerprint TEXT NOT NULL,
            provenance TEXT,
            created_at TEXT DEFAULT (datetime('now')),
            updated_at TEXT DEFAULT (datetime('now')),
            CHECK (ref_kind <> 'idea' OR (label IS NOT NULL AND length(trim(label)) > 0))
        );
        INSERT INTO page_map_nodes__fixture SELECT * FROM page_map_nodes;
        CREATE TABLE page_map_edges__fixture (
            id TEXT PRIMARY KEY,
            page_id TEXT NOT NULL REFERENCES page_maps(page_id) ON DELETE CASCADE,
            from_node TEXT NOT NULL REFERENCES page_map_nodes__fixture(id) ON DELETE CASCADE,
            to_node TEXT NOT NULL REFERENCES page_map_nodes__fixture(id) ON DELETE CASCADE,
            kind TEXT NOT NULL DEFAULT 'link' CHECK (kind IN ('link','suggested')),
            label TEXT,
            status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('suggested','active','dismissed')),
            provenance TEXT,
            created_at TEXT DEFAULT (datetime('now')),
            CHECK (from_node <> to_node),
            UNIQUE (page_id, from_node, to_node, kind)
        );
        INSERT INTO page_map_edges__fixture SELECT * FROM page_map_edges;
        DROP TABLE page_map_edges;
        DROP TABLE page_map_nodes;
        ALTER TABLE page_map_nodes__fixture RENAME TO page_map_nodes;
        ALTER TABLE page_map_edges__fixture RENAME TO page_map_edges;
        CREATE UNIQUE INDEX idx_pmn_fp ON page_map_nodes(page_id, fingerprint);
        CREATE INDEX idx_pmn_page ON page_map_nodes(page_id, status);
        PRAGMA user_version = 133;
        COMMIT;",
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO page_map_nodes (
             id,page_id,parent_id,rank,ref_kind,ref_id,label,status,pinned,placed,
             collapsed,x,y,width,height,fingerprint,provenance,created_at,updated_at
         ) VALUES ('private-idea',?1,?2,9,'idea',
                   '2a1d7bb6-4d1f-4e63-9c0a-a6d7fda5f8b2','Private idea','dismissed',
                   1,1,1,12,13,140,50,'idea-fingerprint','private-provenance','c','u')",
        libsql::params![page_id, parent_id],
    )
    .await
    .unwrap();
    conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
}

#[tokio::test]
async fn independent_idea_lifecycle_never_changes_page_content_or_version() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-idea").await;
    let (map, _) = db.init_page_map("page-idea").await.unwrap();
    let root = root_id(&map.nodes);
    let before = page_content_and_version(&db, "page-idea").await;
    let idea_id = uuid::Uuid::new_v4().to_string();

    let created = match db
        .create_map_node(
            "page-idea",
            map.map.revision,
            &root,
            "idea",
            &idea_id,
            Some("First idea"),
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    assert_eq!(created.ref_kind, "idea");
    assert_eq!(created.label.as_deref(), Some("First idea"));
    let after_create = db.get_page_map("page-idea", true).await.unwrap().unwrap();
    assert_eq!(after_create.map.map_schema, 2);
    assert_eq!(page_content_and_version(&db, "page-idea").await, before);

    let renamed = db
        .patch_map_node(
            "page-idea",
            after_create.map.revision,
            &created.id,
            NodePatch {
                label: Some(Some("Renamed idea".to_string())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.label.as_deref(), Some("Renamed idea"));
    let after_rename = db.get_page_map("page-idea", true).await.unwrap().unwrap();
    assert_eq!(page_content_and_version(&db, "page-idea").await, before);

    let empty_rename = db
        .patch_map_node(
            "page-idea",
            after_rename.map.revision,
            &created.id,
            NodePatch {
                label: Some(Some("  ".to_string())),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        empty_rename,
        Err(crate::WenlanError::Validation(_))
    ));

    let deleted = db
        .delete_map_node("page-idea", after_rename.map.revision, &created.id)
        .await
        .unwrap();
    assert_eq!(deleted.status, "dismissed");
    assert_eq!(page_content_and_version(&db, "page-idea").await, before);
    let deleted_map = db.get_page_map("page-idea", true).await.unwrap().unwrap();
    assert_eq!(
        db.create_map_node(
            "page-idea",
            deleted_map.map.revision,
            &root,
            "idea",
            &idea_id,
            Some("First idea"),
            1.0,
        )
        .await
        .unwrap(),
        CreateNodeOutcome::Tombstoned
    );
    assert_eq!(page_content_and_version(&db, "page-idea").await, before);
}

#[tokio::test]
async fn independent_idea_rejects_invalid_input_and_stale_cas() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-idea-invalid").await;
    let (map, _) = db.init_page_map("page-idea-invalid").await.unwrap();
    let root = root_id(&map.nodes);

    for (ref_kind, ref_id, label) in [
        ("unknown", "not-a-uuid", Some("label")),
        ("idea", "not-a-uuid", Some("label")),
        ("idea", "2a1d7bb6-4d1f-4e63-9c0a-a6d7fda5f8b2", None),
        ("idea", "2a1d7bb6-4d1f-4e63-9c0a-a6d7fda5f8b2", Some("   ")),
    ] {
        let result = db
            .create_map_node(
                "page-idea-invalid",
                map.map.revision,
                &root,
                ref_kind,
                ref_id,
                label,
                0.0,
            )
            .await;
        assert!(matches!(result, Err(crate::WenlanError::Validation(_))));
    }

    let stale = db
        .create_map_node(
            "page-idea-invalid",
            map.map.revision - 1,
            &root,
            "idea",
            "2a1d7bb6-4d1f-4e63-9c0a-a6d7fda5f8b2",
            Some("valid"),
            0.0,
        )
        .await;
    assert!(matches!(stale, Err(crate::WenlanError::Conflict(_))));
    let unchanged = db
        .get_page_map("page-idea-invalid", true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.map.map_schema, 1);
    assert_eq!(unchanged.map.revision, map.map.revision);
    assert_eq!(unchanged.nodes.len(), 1);
}

#[tokio::test]
async fn migration_134_public_133_backfills_only_exact_snapshot_and_reopens_idempotently() {
    let (db, dir) = test_db_at(133).await;
    assert!(page_history_has_title(&db).await);
    seed_page(&db, "public-133-history").await;
    let (map, _) = db.init_page_map("public-133-history").await.unwrap();
    let root = root_id(&map.nodes);
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET title='Current title', version=3, content='current body',
                 source_memory_ids='[\"current-source\"]' WHERE id='public-133-history'",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO page_history
                 (page_id,version,content,source_memory_ids,title,edited_by,created_at)
             VALUES ('public-133-history',1,'old body','[\"old-source\"]',
                     'Known historic title','fixture',1)",
            (),
        )
        .await
        .unwrap();
    }
    insert_history_without_title(
        &db,
        "public-133-history",
        2,
        "older body",
        "[\"older-source\"]",
    )
    .await;
    insert_history_without_title(
        &db,
        "public-133-history",
        3,
        "current body",
        "[\"current-source\"]",
    )
    .await;

    db.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(db_user_version(&db).await, 134);
    assert!(matches!(
        db.create_map_node(
            "public-133-history",
            map.map.revision,
            &root,
            "idea",
            "2a1d7bb6-4d1f-4e63-9c0a-a6d7fda5f8b2",
            Some("Public idea"),
            0.0,
        )
        .await
        .unwrap(),
        CreateNodeOutcome::Created(_)
    ));
    assert_eq!(
        page_history_title(&db, "public-133-history", 1)
            .await
            .as_deref(),
        Some("Known historic title")
    );
    assert_eq!(
        page_history_title(&db, "public-133-history", 2).await,
        None,
        "older history titles must not be inferred from the live page"
    );
    assert_eq!(
        page_history_title(&db, "public-133-history", 3)
            .await
            .as_deref(),
        Some("Current title")
    );
    db.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(db_user_version(&db).await, 134);

    let embedder = db.embedder.clone().expect("test database embedder");
    drop(db);
    let reopened = MemoryDB::new_with_shared_embedder(
        dir.path(),
        std::sync::Arc::new(crate::events::NoopEmitter),
        embedder,
    )
    .await
    .unwrap();
    reopened.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(
        db_user_version(&reopened).await,
        i64::from(crate::db::SCHEMA_VERSION)
    );
    assert_eq!(
        page_history_title(&reopened, "public-133-history", 1)
            .await
            .as_deref(),
        Some("Known historic title")
    );
    assert_eq!(
        page_history_title(&reopened, "public-133-history", 2).await,
        None
    );
    assert_eq!(
        page_history_title(&reopened, "public-133-history", 3)
            .await
            .as_deref(),
        Some("Current title")
    );
}

#[tokio::test]
async fn migration_134_backups_existing_133_before_ddl_and_preserves_it_on_retry() {
    let (db, dir) = test_db_at(133).await;
    // `test_db_at` models a fresh database boot by default. This fixture
    // represents an already-existing on-disk database reaching migration 134.
    db.skip_migration_backups
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let backup_path = dir.path().join("pre_migration_134_backup.db");
    assert!(
        !backup_path.exists(),
        "fixture must begin without a migration-134 backup"
    );

    // Fail after the node table was staged. Migration DDL must roll back, but
    // the pre-migration backup is intentionally outside that transaction.
    {
        let conn = db.conn.lock().await;
        conn.execute("CREATE TABLE page_map_edges__m134 (marker TEXT)", ())
            .await
            .unwrap();
    }
    assert!(db.migrate_134_page_map_ideas().await.is_err());
    assert_eq!(db_user_version(&db).await, 133);
    assert!(
        backup_path.exists(),
        "the existing database needs a restore point before migration DDL"
    );
    assert!(db
        .get_app_metadata("backup_before_migration_134")
        .await
        .unwrap()
        .is_some());

    let backup_bytes = std::fs::read(&backup_path).unwrap();
    let backup_db = libsql::Builder::new_local(backup_path.to_string_lossy().to_string())
        .build()
        .await
        .unwrap();
    let backup_conn = backup_db.connect().unwrap();
    let mut version_rows = backup_conn.query("PRAGMA user_version", ()).await.unwrap();
    let backup_version = version_rows
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(
        backup_version, 133,
        "the snapshot must predate migration 134"
    );
    let mut integrity_rows = backup_conn
        .query("PRAGMA integrity_check", ())
        .await
        .unwrap();
    assert_eq!(
        integrity_rows
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        "ok"
    );
    drop(backup_conn);
    drop(backup_db);

    {
        let conn = db.conn.lock().await;
        conn.execute("DROP TABLE page_map_edges__m134", ())
            .await
            .unwrap();
    }
    db.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(db_user_version(&db).await, 134);
    assert_eq!(
        std::fs::read(&backup_path).unwrap(),
        backup_bytes,
        "retry must preserve the original pre-134 restore point byte-for-byte"
    );
}

#[tokio::test]
async fn migration_134_skips_backup_when_fresh_store_backup_policy_is_set() {
    let (db, dir) = test_db_at(133).await;
    db.skip_migration_backups
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let backup_path = dir.path().join("pre_migration_134_backup.db");

    db.migrate_134_page_map_ideas().await.unwrap();

    assert_eq!(db_user_version(&db).await, 134);
    assert!(
        !backup_path.exists(),
        "migration 134 must retain the existing fresh-store backup skip behavior"
    );
    assert!(db
        .get_app_metadata("backup_before_migration_134")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn migration_134_private_133_adds_history_title_and_preserves_idea_rows() {
    let (db, _tmp) = test_db_at(132).await;
    seed_page(&db, "private-133-history").await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET title='Private current title', version=3, content='current body',
                 source_memory_ids='[\"current-source\"]' WHERE id='private-133-history'",
            (),
        )
        .await
        .unwrap();
    }
    insert_history_without_title(
        &db,
        "private-133-history",
        1,
        "old body",
        "[\"old-source\"]",
    )
    .await;
    insert_history_without_title(
        &db,
        "private-133-history",
        3,
        "current body",
        "[\"current-source\"]",
    )
    .await;
    let (map, _) = db.init_page_map("private-133-history").await.unwrap();
    let root = root_id(&map.nodes);
    install_private_133_page_map_ideas(&db, "private-133-history", &root).await;
    assert_eq!(db_user_version(&db).await, 133);
    assert!(!page_history_has_title(&db).await);

    db.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(db_user_version(&db).await, 134);
    assert!(page_history_has_title(&db).await);
    assert_eq!(
        page_history_title(&db, "private-133-history", 1).await,
        None,
        "the missing historical title must remain unknown"
    );
    assert_eq!(
        page_history_title(&db, "private-133-history", 3)
            .await
            .as_deref(),
        Some("Private current title")
    );
    {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT ref_kind,ref_id,label,status,pinned,placed,collapsed,x,y,width,height,provenance
                   FROM page_map_nodes WHERE id='private-idea'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("private idea row");
        assert_eq!(row.get::<String>(0).unwrap(), "idea");
        assert_eq!(
            row.get::<String>(1).unwrap(),
            "2a1d7bb6-4d1f-4e63-9c0a-a6d7fda5f8b2"
        );
        assert_eq!(row.get::<String>(2).unwrap(), "Private idea");
        assert_eq!(row.get::<String>(3).unwrap(), "dismissed");
        assert_eq!(row.get::<i64>(4).unwrap(), 1);
        assert_eq!(row.get::<i64>(5).unwrap(), 1);
        assert_eq!(row.get::<i64>(6).unwrap(), 1);
        assert_eq!(row.get::<f64>(7).unwrap(), 12.0);
        assert_eq!(row.get::<f64>(8).unwrap(), 13.0);
        assert_eq!(row.get::<f64>(9).unwrap(), 140.0);
        assert_eq!(row.get::<f64>(10).unwrap(), 50.0);
        assert_eq!(row.get::<String>(11).unwrap(), "private-provenance");
    }
    assert!(db_foreign_keys_enabled(&db).await);
    assert!(
        pragma_names(&db, "PRAGMA foreign_key_check(page_map_nodes)", 0)
            .await
            .is_empty()
    );
    assert!(
        pragma_names(&db, "PRAGMA foreign_key_check(page_map_edges)", 0)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn migration_134_from_132_preserves_rows_edges_layout_fks_indexes_and_rolls_back() {
    let (db, _tmp) = test_db_at(132).await;
    seed_page(&db, "page-map-migration").await;
    let (map, _) = db.init_page_map("page-map-migration").await.unwrap();
    let root = root_id(&map.nodes);
    let a = match db
        .create_map_node(
            "page-map-migration",
            map.map.revision,
            &root,
            "memory",
            "m-a",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let revision = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let b = match db
        .create_map_node(
            "page-map-migration",
            revision,
            &root,
            "entity",
            "e-b",
            None,
            1.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let revision = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let edge_a = match db
        .create_map_edge(
            "page-map-migration",
            revision,
            &root,
            &a.id,
            "link",
            Some("active"),
        )
        .await
        .unwrap()
    {
        CreateEdgeOutcome::Created(edge) => edge,
        other => panic!("expected Created, got {other:?}"),
    };
    let revision = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let edge_b = match db
        .create_map_edge(
            "page-map-migration",
            revision,
            &root,
            &b.id,
            "link",
            Some("dismissed"),
        )
        .await
        .unwrap()
    {
        CreateEdgeOutcome::Created(edge) => edge,
        other => panic!("expected Created, got {other:?}"),
    };
    let revision = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_edge("page-map-migration", revision, &edge_b.id)
        .await
        .unwrap();
    let revision = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-map-migration", revision, &b.id)
        .await
        .unwrap();
    let revision = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.put_page_map_layout(
        "page-map-migration",
        revision,
        Some(r#"{"x":12.0,"y":34.0,"zoom":1.5}"#),
        &[
            NodeLayout {
                node_id: root.clone(),
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 80.0,
                collapsed: false,
            },
            NodeLayout {
                node_id: a.id.clone(),
                x: 50.0,
                y: 60.0,
                width: 120.0,
                height: 48.0,
                collapsed: true,
            },
            NodeLayout {
                node_id: b.id.clone(),
                x: 90.0,
                y: 100.0,
                width: 110.0,
                height: 44.0,
                collapsed: false,
            },
        ],
    )
    .await
    .unwrap();
    let before = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap();
    assert!(before
        .edges
        .iter()
        .any(|edge| edge.id == edge_a.id && edge.status == "active"));
    assert!(before
        .edges
        .iter()
        .any(|edge| edge.id == edge_b.id && edge.status == "dismissed"));

    let node_indexes_before = pragma_names(&db, "PRAGMA index_list('page_map_nodes')", 1).await;
    let edge_indexes_before = pragma_names(&db, "PRAGMA index_list('page_map_edges')", 1).await;
    let node_fks_before = pragma_names(&db, "PRAGMA foreign_key_list('page_map_nodes')", 2).await;
    let edge_fks_before = pragma_names(&db, "PRAGMA foreign_key_list('page_map_edges')", 2).await;

    // Force failure after the staged node table has copied all rows.
    // Transactional DDL must leave the original map intact and restore FK mode.
    {
        let conn = db.conn.lock().await;
        conn.execute("CREATE TABLE page_map_edges__m134 (marker TEXT)", ())
            .await
            .unwrap();
    }
    assert!(db.migrate_134_page_map_ideas().await.is_err());
    assert_eq!(db_user_version(&db).await, 132);
    assert!(!page_history_has_title(&db).await);
    assert!(db_foreign_keys_enabled(&db).await);
    assert_eq!(
        db.get_page_map("page-map-migration", true)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    {
        let conn = db.conn.lock().await;
        conn.execute("DROP TABLE page_map_edges__m134", ())
            .await
            .unwrap();
    }

    db.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(db_user_version(&db).await, 134);
    assert!(db_foreign_keys_enabled(&db).await);
    let after = db
        .get_page_map("page-map-migration", true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        after.map.map_schema, 1,
        "migration alone must not opt maps in"
    );
    assert_eq!(
        pragma_names(&db, "PRAGMA index_list('page_map_nodes')", 1).await,
        node_indexes_before
    );
    assert_eq!(
        pragma_names(&db, "PRAGMA index_list('page_map_edges')", 1).await,
        edge_indexes_before
    );
    assert_eq!(
        pragma_names(&db, "PRAGMA foreign_key_list('page_map_nodes')", 2).await,
        node_fks_before
    );
    assert_eq!(
        pragma_names(&db, "PRAGMA foreign_key_list('page_map_edges')", 2).await,
        edge_fks_before
    );
    assert!(
        pragma_names(&db, "PRAGMA foreign_key_check(page_map_nodes)", 0)
            .await
            .is_empty()
    );
    assert!(
        pragma_names(&db, "PRAGMA foreign_key_check(page_map_edges)", 0)
            .await
            .is_empty()
    );
    db.migrate_134_page_map_ideas().await.unwrap();
    assert_eq!(
        db.get_page_map("page-map-migration", true)
            .await
            .unwrap()
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn reparent_rejects_cycle() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let a = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-a",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let b = match db
        .create_map_node("page-1", rev, &a.id, "memory", "mem-b", None, 0.0)
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    // a -> root, b -> a. Re-parenting a under b would make a its own ancestor.
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .patch_map_node(
            "page-1",
            rev,
            &a.id,
            NodePatch {
                parent_id: Some(b.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

#[tokio::test]
async fn reset_page_map_clears_tombstones() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let created = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-1", rev, &created.id)
        .await
        .unwrap();

    db.reset_page_map("page-1").await.unwrap();
    assert!(db.get_page_map("page-1", true).await.unwrap().is_none());

    // Fresh init + the same fingerprint must succeed as Created, not Tombstoned.
    let (map, created_after_reset) = db.init_page_map("page-1").await.unwrap();
    assert!(created_after_reset);
    let root = root_id(&map.nodes);
    let outcome = db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap();
    assert!(matches!(outcome, CreateNodeOutcome::Created(_)));
}

// Dismissed rows are terminal: no patch, of any shape, can touch them — not
// even one that only re-parents (which would otherwise recompute the
// fingerprint and free the old tombstone key, letting a dismissed suggestion
// resurface under a different parent).
#[tokio::test]
async fn patch_on_dismissed_node_rejected_and_tombstone_holds() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let other_parent = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-other-parent",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let created = match db
        .create_map_node("page-1", rev, &root, "memory", "mem-1", None, 0.0)
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-1", rev, &created.id)
        .await
        .unwrap();
    let fingerprint_before = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .nodes
        .into_iter()
        .find(|n| n.id == created.id)
        .unwrap()
        .fingerprint;

    // A patch touching only parent_id (no status change) must still be
    // rejected — dismissed is terminal regardless of patch contents.
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .patch_map_node(
            "page-1",
            rev,
            &created.id,
            NodePatch {
                parent_id: Some(other_parent.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));

    // Fingerprint must be unchanged in the DB.
    let after = db.get_page_map("page-1", true).await.unwrap().unwrap();
    let node_after = after.nodes.iter().find(|n| n.id == created.id).unwrap();
    assert_eq!(node_after.fingerprint, fingerprint_before);

    // A subsequent create under the original parent still tombstones.
    let rev = after.map.revision;
    let outcome = db
        .create_map_node("page-1", rev, &root, "memory", "mem-1", None, 0.0)
        .await
        .unwrap();
    assert_eq!(outcome, CreateNodeOutcome::Tombstoned);
}

#[tokio::test]
async fn patch_on_dismissed_edge_rejected() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let a = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-a",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let b = match db
        .create_map_node("page-1", rev, &root, "memory", "mem-b", None, 0.0)
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let edge = match db
        .create_map_edge("page-1", rev, &a.id, &b.id, "link", None)
        .await
        .unwrap()
    {
        CreateEdgeOutcome::Created(edge) => edge,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_edge("page-1", rev, &edge.id).await.unwrap();

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .patch_map_edge(
            "page-1",
            rev,
            &edge.id,
            EdgePatch {
                label: Some(Some("relabel".to_string())),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

// Dismissal cannot orphan: dismissing a node with a live child is rejected.
#[tokio::test]
async fn dismiss_with_live_child_rejected() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let parent = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-parent",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.create_map_node("page-1", rev, &parent.id, "memory", "mem-child", None, 0.0)
        .await
        .unwrap();

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .delete_map_node("page-1", rev, &parent.id)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

// Creating a node under a dismissed parent is rejected.
#[tokio::test]
async fn create_under_dismissed_parent_rejected() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let parent = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-parent",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-1", rev, &parent.id).await.unwrap();

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .create_map_node("page-1", rev, &parent.id, "memory", "mem-child", None, 0.0)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

// Creating an edge to a dismissed endpoint is rejected.
#[tokio::test]
async fn edge_to_dismissed_endpoint_rejected() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let a = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-a",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let b = match db
        .create_map_node("page-1", rev, &root, "memory", "mem-b", None, 0.0)
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-1", rev, &b.id).await.unwrap();

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .create_map_edge("page-1", rev, &a.id, &b.id, "link", None)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

#[tokio::test]
async fn fingerprint_uses_unit_separator_and_rejects_it_in_ref_components() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    // A ref_id containing the fingerprint separator itself is rejected.
    let err = db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "a\u{1f}b",
            None,
            0.0,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));

    // ref_ids that would have collided under the OLD "{kind}:{id}@{parent}"
    // scheme (colliding on '@'/':' in the ref_id) now insert as distinct rows.
    let first = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "a@b",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    assert_eq!(first.fingerprint, "memory\u{1f}a@b\u{1f}~");

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let second = match db
        .create_map_node("page-1", rev, &root, "memory", "a:b", None, 0.0)
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    assert_eq!(second.fingerprint, "memory\u{1f}a:b\u{1f}~");
    assert_ne!(first.fingerprint, second.fingerprint);
}

#[tokio::test]
async fn init_page_map_created_flag_reflects_atomic_insert() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;

    let (first, created_first) = db.init_page_map("page-1").await.unwrap();
    assert!(created_first, "first call must report created = true");
    assert_eq!(first.map.revision, 1);

    let (second, created_second) = db.init_page_map("page-1").await.unwrap();
    assert!(
        !created_second,
        "second call must report created = false (idempotent no-op)"
    );
    assert_eq!(second.map.revision, 1);
}

#[tokio::test]
async fn put_page_map_layout_round_trip_pins_and_places_positioned_nodes() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let child = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let viewport = r#"{"x":1.0,"y":2.0,"zoom":1.5}"#;
    let positions = vec![NodeLayout {
        node_id: child.id.clone(),
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 50.0,
        collapsed: true,
    }];
    let data = db
        .put_page_map_layout("page-1", rev, Some(viewport), &positions)
        .await
        .unwrap();

    assert_eq!(
        data.map.revision,
        rev + 1,
        "layout write must bump the revision"
    );
    assert_eq!(data.map.viewport.as_deref(), Some(viewport));

    let updated = data.nodes.iter().find(|n| n.id == child.id).unwrap();
    assert_eq!(updated.x, Some(10.0));
    assert_eq!(updated.y, Some(20.0));
    assert_eq!(updated.width, Some(100.0));
    assert_eq!(updated.height, Some(50.0));
    assert!(updated.placed, "positioned node must be marked placed");
    assert!(
        updated.pinned,
        "positioned node must be pinned (move with placement)"
    );
    assert!(updated.collapsed);

    // Stale base_revision is rejected.
    let err = db
        .put_page_map_layout("page-1", rev, None, &[])
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Conflict(_)));
}

// Re-parenting onto a dismissed target parent is rejected — the same
// cannot-orphan rule as create-under-dismissed-parent, on the patch path.
#[tokio::test]
async fn reparent_onto_dismissed_parent_rejected() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-1").await;
    let (map, _created) = db.init_page_map("page-1").await.unwrap();
    let root = root_id(&map.nodes);

    let a = match db
        .create_map_node(
            "page-1",
            map.map.revision,
            &root,
            "memory",
            "mem-a",
            None,
            0.0,
        )
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let b = match db
        .create_map_node("page-1", rev, &root, "memory", "mem-b", None, 1.0)
        .await
        .unwrap()
    {
        CreateNodeOutcome::Created(node) => node,
        other => panic!("expected Created, got {other:?}"),
    };
    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    db.delete_map_node("page-1", rev, &b.id).await.unwrap();

    let rev = db
        .get_page_map("page-1", true)
        .await
        .unwrap()
        .unwrap()
        .map
        .revision;
    let err = db
        .patch_map_node(
            "page-1",
            rev,
            &a.id,
            NodePatch {
                parent_id: Some(b.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::WenlanError::Validation(_)));
}

// `page_maps.page_id` references `pages(id)` without ON DELETE CASCADE, so a
// page whose map was ever opened could not be deleted ("FOREIGN KEY
// constraint failed"). Deleting the page takes its map with it.
#[tokio::test]
async fn delete_page_removes_its_map() {
    let (db, _tmp) = test_db().await;
    seed_page(&db, "page-with-map").await;
    let (map, _) = db.init_page_map("page-with-map").await.unwrap();
    let root = root_id(&map.nodes);
    let created = db
        .create_map_node(
            "page-with-map",
            map.map.revision,
            &root,
            "memory",
            "mem-1",
            None,
            0.0,
        )
        .await
        .unwrap();
    assert!(matches!(created, CreateNodeOutcome::Created(_)));

    db.delete_page("page-with-map").await.unwrap();

    let conn = db.conn.lock().await;
    for table in ["page_maps", "page_map_nodes", "page_map_edges"] {
        let mut rows = conn
            .query(
                &format!("SELECT COUNT(*) FROM {table} WHERE page_id = ?1"),
                libsql::params!["page-with-map"],
            )
            .await
            .unwrap();
        let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(count, 0, "{table} rows must go with the page");
    }
}
