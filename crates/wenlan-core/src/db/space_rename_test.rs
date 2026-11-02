//! Teeth for S0-162's rename closure — mutation-catalog gates `G8.11a`-`c`.
//!
//! One scenario asserted several ways: rename a space that carries community
//! substrate, then check that identity is unchanged, that the new
//! name resolves, and that no row still names the old space.

use super::space_rename::closed_tables;
use crate::db::tests::test_db;
use crate::db::MemoryDB;

const OLD: &str = "Research Notes";
const NEW: &str = "Field Notes";

/// Tables that carry a `space` column and are closed over by the cascade that
/// already existed before S0-162 (`update_space`, `db.rs:19327`-`:19360`).
const PRE_EXISTING_CASCADE: &[&str] = &[
    "memories",
    "entities",
    "pages",
    "agent_sessions",
    "session_reads",
];

/// Tables that carry a `space` column and are deliberately left alone.
///
/// `page_draft_create_requests` is S0-162's one recorded exclusion: its stored
/// name is replay history, not stale data (`db.rs:7771`).
const DELIBERATE_EXCLUSIONS: &[&str] = &["page_draft_create_requests"];

async fn seed_substrate(conn: &libsql::Connection, space: &str) {
    let stmts: Vec<(&str, Vec<libsql::Value>)> = vec![
        // G6 Stage 3: the `entities` row 'ent-1' that used to head this list
        // is gone with the table (migration 123). Its space column was one of
        // the rename targets; the shadow page below now carries the entity's
        // space, and it is already in the list.
        //
        // G6 Stage 2 item 1 (db.rs:9219): the ported `edges_space_fence`/
        // `edges_space_fence_update` triggers resolve an entity endpoint's
        // space via its shadow page, not `entities.space` directly, and
        // fail closed (abort) when no shadow page exists. The non-legacy
        // edge seeded below needs one for 'ent-1' before it can insert.
        // Same shape as `insert_entity_shadow_page`/`m4_community_gates.rs`'s
        // `seed_entity_shadow_pages`.
        (
            "INSERT INTO pages (
                 id, title, summary, content, kind, entity_type, confidence, entity_confirmed,
                 embedding, space, workspace, source_memory_ids, version, status,
                 created_at, last_compiled, last_modified, creation_kind, review_status
             )
             VALUES ('shadow-ent-1', 'Anchor', NULL, '', 'entity', 'concept', NULL, 0,
                     NULL, ?1, ?1, '[]', 1, 'active',
                     '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z',
                     'entity', 'unconfirmed')",
            vec![space.into()],
        ),
        (
            "INSERT INTO entity_page_map (entity_id, page_id, created_at)
             VALUES ('ent-1', 'shadow-ent-1', '2024-01-01T00:00:00Z')",
            vec![],
        ),
        // `m4_page_community_page_insert_input` (db.rs:11483) fires on every
        // `pages` insert and mints a `page_community_route_inputs` row keyed
        // by page_id, so the shadow page above added a second one this
        // closure never expected -- 'page-1' is the fixture's one
        // community-routable page. Undo the trigger's side effect for the
        // shadow page only; it carries no separate routing meaning here.
        (
            "DELETE FROM page_community_route_inputs WHERE page_id = 'shadow-ent-1'",
            vec![],
        ),
        (
            "INSERT INTO pages (id, title, content, space, created_at, last_compiled, last_modified)
             VALUES ('page-1', 'Anchor', 'body', ?1, '2024-01-01T00:00:00Z',
                     '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            vec![space.into()],
        ),
        (
            "INSERT INTO communities
                 (community_id, space, algo_version, projection_version, created_at, updated_at)
             VALUES ('com-1', ?1, 'leiden-v1', 'proj-v1', 1, 1)",
            vec![space.into()],
        ),
        (
            "INSERT OR REPLACE INTO community_members
                 (space, node_id, node_kind, community_id, published_generation, attachment)
             VALUES (?1, 'ent-1', 'entity', 'com-1', 1, 'core')",
            vec![space.into()],
        ),
        (
            "INSERT INTO page_community_assignments
                 (page_id, space, community_id, state, score, page_version,
                  routing_input_generation, routing_space_generation,
                  community_published_generation, route_version, updated_at)
             VALUES ('page-1', ?1, 'com-1', 'assigned', 0.9, 1, 0, 0, 1, 'route-v1', 1)",
            vec![space.into()],
        ),
        (
            "INSERT OR REPLACE INTO page_community_route_inputs (page_id, space, generation)
             VALUES ('page-1', ?1, 0)",
            vec![space.into()],
        ),
        (
            "INSERT OR REPLACE INTO community_route_space_inputs (space, generation) VALUES (?1, 0)",
            vec![space.into()],
        ),
        (
            "INSERT OR REPLACE INTO space_graph_state (space, dirty) VALUES (?1, 1)",
            vec![space.into()],
        ),
        (
            "INSERT OR REPLACE INTO grouping_leases (phase, space, input_generation, token, expires_at)
             VALUES ('grouping', ?1, 1, 'tok-1', 9999999999)",
            vec![space.into()],
        ),
        // No `community_reader_parity` row: the table is dropped by the
        // "retire local-only community parity model" migration (`db.rs:11659`),
        // which is why it is not in the closure either.
        (
            "INSERT OR REPLACE INTO community_reader_space_proof
                 (consumer, space, proven_published_generation, proven_membership_digest)
             VALUES ('scoped_pages', ?1, 1, 'digest-1')",
            vec![space.into()],
        ),
        (
            "INSERT OR REPLACE INTO community_publication_receipts
                 (space, published_generation, membership_digest, algo_version,
                  projection_version, published_at)
             VALUES (?1, 1, 'digest-1', 'leiden-v1', 'proj-v1', 1)",
            vec![space.into()],
        ),
        // A NON-legacy edge, so `edges_space_fence_update` (db.rs:8932) is live
        // on the rewrite. Its endpoint is the seeded entity, which the
        // pre-existing cascade renames first — this row is what proves the
        // ordering rather than merely assuming it.
        (
            "INSERT INTO edges
                 (edge_id, src_id, src_kind, dst_id, dst_kind, edge_type, lineage,
                  grounded, space, created_at)
             VALUES ('edge-1', 'ent-1', 'entity', 'ent-1', 'entity', 'relates',
                     'assertion', 0, ?1, 1)",
            vec![space.into()],
        ),
    ];
    for (sql, params) in stmts {
        conn.execute(sql, params)
            .await
            .unwrap_or_else(|e| panic!("seed failed: {sql}\n{e}"));
    }
}

async fn rows_naming(conn: &libsql::Connection, table: &str, space: &str) -> i64 {
    let mut rows = conn
        .query(
            &format!("SELECT COUNT(*) FROM {table} WHERE space = ?1"),
            libsql::params![space],
        )
        .await
        .unwrap_or_else(|e| panic!("count {table}: {e}"));
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

async fn seeded_db() -> (MemoryDB, tempfile::TempDir) {
    let (db, dir) = test_db().await;
    db.create_space(OLD, None, false).await.unwrap();
    {
        let conn = db.conn.lock().await;
        seed_substrate(&conn, OLD).await;
    }
    (db, dir)
}

#[tokio::test]
async fn every_space_keyed_row_follows_the_rename() {
    let (db, _dir) = seeded_db().await;
    db.update_space(OLD, NEW, None).await.unwrap();

    let conn = db.conn.lock().await;
    for table in closed_tables() {
        assert_eq!(
            rows_naming(&conn, table, NEW).await,
            1,
            "{table} should carry exactly one row under the new name"
        );
    }
}

#[tokio::test]
async fn no_substrate_row_still_names_the_old_space() {
    // G8.11c stated once over the whole closure: a row keyed to the old name
    // must not survive the rename and stay claimable.
    let (db, _dir) = seeded_db().await;
    db.update_space(OLD, NEW, None).await.unwrap();

    let conn = db.conn.lock().await;
    for table in closed_tables() {
        assert_eq!(
            rows_naming(&conn, table, OLD).await,
            0,
            "{table} still names the old space after the rename"
        );
    }
}

#[tokio::test]
async fn the_page_draft_ledger_keeps_the_old_name() {
    // The one deliberate exclusion. Rewriting it would break delayed-create
    // replay, which is the failure the ledger exists to prevent.
    let (db, _dir) = seeded_db().await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO page_draft_create_requests (page_id, title, content, space)
             VALUES ('draft-1', 'Draft', 'body', ?1)",
            libsql::params![OLD],
        )
        .await
        .unwrap();
    }

    db.update_space(OLD, NEW, None).await.unwrap();

    let conn = db.conn.lock().await;
    assert_eq!(
        rows_naming(&conn, "page_draft_create_requests", OLD).await,
        1,
        "the create-request ledger must keep the name the request was made under"
    );
}

#[tokio::test]
async fn an_orphan_row_under_the_new_name_is_retired() {
    // `spaces.name` is UNIQUE, so a row already keyed to the destination name
    // names no live space. It must not block the rename.
    let (db, _dir) = seeded_db().await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO space_graph_state (space, graph_generation, dirty) VALUES (?1, 77, 0)",
            libsql::params![NEW],
        )
        .await
        .unwrap();
    }

    db.update_space(OLD, NEW, None).await.unwrap();

    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*), MAX(graph_generation), MAX(dirty) FROM space_graph_state
             WHERE space = ?1",
            libsql::params![NEW],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let count: i64 = row.get(0).unwrap();
    let generation: i64 = row.get(1).unwrap();
    let dirty: i64 = row.get(2).unwrap();
    assert_eq!(count, 1, "the orphan must be retired, not kept alongside");
    assert_eq!(generation, 0, "the renamed row must be the survivor");
    assert_eq!(dirty, 1, "the renamed row must be the survivor");
}

#[tokio::test]
async fn a_rename_does_not_change_the_space_id() {
    // G8.11a: a rename keeps the space's id.
    let (db, _dir) = seeded_db().await;
    let before = db.get_space(OLD).await.unwrap().unwrap();
    db.update_space(OLD, NEW, None).await.unwrap();
    let after = db.get_space(NEW).await.unwrap().unwrap();

    assert_eq!(before.id, after.id, "a rename must not re-key the space");
}

#[tokio::test]
async fn the_new_name_resolves_and_the_old_one_does_not() {
    // G8.11b's reachable half. S0-161 refuses a mint whose stored space does
    // not resolve; a routine rename must not be what makes it refuse.
    let (db, _dir) = seeded_db().await;
    db.update_space(OLD, NEW, None).await.unwrap();

    assert!(db.get_space(NEW).await.unwrap().is_some());
    assert!(db.get_space(OLD).await.unwrap().is_none());

    // Every substrate row now names a space that resolves.
    let conn = db.conn.lock().await;
    for table in closed_tables() {
        let mut rows = conn
            .query(
                &format!(
                    "SELECT COUNT(*) FROM {table} t
                     WHERE NOT EXISTS (SELECT 1 FROM spaces s WHERE s.name = t.space)"
                ),
                (),
            )
            .await
            .unwrap();
        let unresolvable: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(unresolvable, 0, "{table} holds a row naming no live space");
    }
}

#[tokio::test]
async fn renaming_a_space_with_no_substrate_is_a_no_op() {
    // The catalog's suite-level control.
    let (db, _dir) = test_db().await;
    db.create_space(OLD, None, false).await.unwrap();
    db.update_space(OLD, NEW, None).await.unwrap();

    let conn = db.conn.lock().await;
    for table in closed_tables() {
        assert_eq!(rows_naming(&conn, table, NEW).await, 0, "{table}");
        assert_eq!(rows_naming(&conn, table, OLD).await, 0, "{table}");
    }
}

#[tokio::test]
async fn every_space_keyed_table_is_accounted_for() {
    // The enumeration S0-162 calls closed is only closed if something checks
    // it. A new table carrying a `space` column must land in the closure, in
    // the pre-existing cascade, or in the recorded exclusions — never silently
    // outside all three.
    let (db, _dir) = test_db().await;
    let conn = db.conn.lock().await;

    let mut tables: Vec<String> = Vec::new();
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
            (),
        )
        .await
        .unwrap();
    while let Some(row) = rows.next().await.unwrap() {
        tables.push(row.get(0).unwrap());
    }

    let mut space_keyed: Vec<String> = Vec::new();
    for table in tables {
        let mut cols = conn
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .unwrap();
        while let Some(col) = cols.next().await.unwrap() {
            let name: String = col.get(1).unwrap();
            if name == "space" {
                space_keyed.push(table.clone());
                break;
            }
        }
    }

    let accounted: Vec<&str> = closed_tables()
        .into_iter()
        .chain(PRE_EXISTING_CASCADE.iter().copied())
        .chain(DELIBERATE_EXCLUSIONS.iter().copied())
        .collect();

    let unaccounted: Vec<&String> = space_keyed
        .iter()
        .filter(|t| !accounted.contains(&t.as_str()))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "these tables carry a `space` column and are in no S0-162 bucket: {unaccounted:?}"
    );

    for table in closed_tables() {
        assert!(
            space_keyed.iter().any(|t| t == table),
            "{table} is in the closure but carries no `space` column"
        );
    }
}

// ---------------------------------------------------------------------------
// Merge cascade (`reassign_memories_space`, `delete_space(_, "move:…")`).
//
// The same seeded substrate, folded into a LIVE second space rather than
// renamed: edges follow the moved entities, the source's communities retire,
// its re-derivable control rows go, and the target's grouping input sees the
// moved edge.
// ---------------------------------------------------------------------------

async fn seeded_db_with_live_target() -> (MemoryDB, tempfile::TempDir) {
    let (db, dir) = seeded_db().await;
    db.create_space(NEW, None, false).await.unwrap();
    (db, dir)
}

async fn assert_substrate_merged_into_target(db: &MemoryDB) {
    let conn = db.conn.lock().await;
    assert_eq!(
        rows_naming(&conn, "edges", OLD).await,
        0,
        "an edge still names the source space after the merge"
    );
    assert_eq!(rows_naming(&conn, "edges", NEW).await, 1);

    let mut rows = conn
        .query(
            "SELECT space, retired_at FROM communities WHERE community_id = 'com-1'",
            (),
        )
        .await
        .unwrap();
    let community = rows.next().await.unwrap().unwrap();
    assert_eq!(community.get::<String>(0).unwrap(), OLD);
    assert!(
        community.get::<Option<i64>>(1).unwrap().is_some(),
        "the source community must be retired, not left live under the old name"
    );
    drop(rows);

    for table in closed_tables() {
        if table == "communities" || table == "edges" {
            continue;
        }
        assert_eq!(
            rows_naming(&conn, table, OLD).await,
            0,
            "{table} rows naming the source space after the merge"
        );
    }

    let mut rows = conn
        .query(
            "SELECT dirty FROM space_graph_state WHERE space = ?1",
            libsql::params![NEW],
        )
        .await
        .unwrap();
    let dirty: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        dirty, 1,
        "the target must be re-grouped over the merged input"
    );
    drop(rows);
    drop(conn);

    let (edges, _) = db.load_community_ungrounded_edges_paged(NEW).await.unwrap();
    let ids: Vec<&str> = edges.iter().map(|e| e.edge_id.as_str()).collect();
    assert_eq!(
        ids,
        ["edge-1"],
        "the target's grouping input must see the moved edge"
    );
}

#[tokio::test]
async fn reassigning_memories_merges_the_substrate_into_the_target() {
    let (db, _dir) = seeded_db_with_live_target().await;
    db.reassign_memories_space(OLD, NEW).await.unwrap();
    assert!(db.get_space(OLD).await.unwrap().is_some());
    assert_substrate_merged_into_target(&db).await;
}

#[tokio::test]
async fn deleting_a_space_with_move_merges_the_substrate_into_the_target() {
    let (db, _dir) = seeded_db_with_live_target().await;
    db.delete_space(OLD, &format!("move:{NEW}")).await.unwrap();
    assert!(db.get_space(OLD).await.unwrap().is_none());
    assert_substrate_merged_into_target(&db).await;
}
