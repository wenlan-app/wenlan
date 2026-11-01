// SPDX-License-Identifier: Apache-2.0
use super::compare::compare_tables;
use super::rebuild_tables;
use crate::db::tests::test_db;
use crate::store::export::history::HISTORY_TABLES;
use crate::store::export::records::RECORD_TABLES;
use crate::store::export::run::export_lossless;
use crate::store::export::settings::SETTINGS_FILES;

async fn sql(db: &crate::db::MemoryDB, statement: &str, id: &str) {
    db.export_fixture_sql(statement, id).await;
}

#[tokio::test]
async fn whole_tables_and_history_rebuild_field_for_field() {
    let (db, _db_dir) = test_db().await;
    sql(
        &db,
        "INSERT INTO spaces (id, name, created_at, updated_at) VALUES (?1, 'Work', 1.5, 2)",
        "space_w",
    )
    .await;
    sql(
        &db,
        "INSERT INTO app_metadata (key, value) VALUES (?1, X'00ff')",
        "rebuild_bytes",
    )
    .await;
    sql(
        &db,
        "INSERT INTO source_annotations (source, source_id, context_key, notes, page_ids, \
         version, updated_at) VALUES ('memory', ?1, 'space:sp', 'Check.', '[]', 2, 9)",
        "mem_1",
    )
    .await;
    sql(
        &db,
        "INSERT INTO thing_history (ts, actor, op, thing_id, session_id, data) \
         VALUES (1793404800, 'person', 'thing.rename', ?1, NULL, '{}')",
        "thing_1",
    )
    .await;
    sql(
        &db,
        "INSERT INTO access_log (source_id, accessed_at) VALUES (?1, 1793404801)",
        "mem_1",
    )
    .await;
    sql(
        &db,
        "INSERT INTO rejected_memories (id, content, source_agent, rejection_reason, \
         created_at) VALUES (?1, 'dup', 'codex', 'duplicate', 1793404802)",
        "rej_1",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let target = out.path().join("export");
    export_lossless(
        &db,
        &crate::store::export::run::PagesFolder::default(),
        data_root.path(),
        &target,
    )
    .await
    .unwrap();

    let (fresh, _fresh_dir) = test_db().await;
    let rebuilt = rebuild_tables(&target, &fresh).await.unwrap();
    assert!(rebuilt.problems.is_empty(), "{:?}", rebuilt.problems);

    let mut tables: Vec<&'static str> = SETTINGS_FILES
        .iter()
        .flat_map(|(_, tables)| tables.iter().copied())
        .collect();
    tables.extend(RECORD_TABLES.iter().copied());
    tables.extend(HISTORY_TABLES.iter().map(|spec| spec.table));
    let diffs = compare_tables(&db, &fresh, &tables).await.unwrap();
    assert!(diffs.is_empty(), "{diffs:#?}");
}

#[tokio::test]
async fn state_seeds_put_back_the_columns_no_file_holds() {
    use super::apply_states;

    let (db, _db_dir) = test_db().await;
    db.insert_page_with_kind(
        "p1",
        "Alpha",
        None,
        "Text.",
        None,
        Some("sp"),
        &[],
        "2026-10-31T00:00:00Z",
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    sql(
        &db,
        "UPDATE pages SET version = 4, user_edited = 1, incarnation = 2, \
         changelog = 'edited twice' WHERE id = ?1",
        "p1",
    )
    .await;
    for chunk in [0, 1] {
        sql(
            &db,
            &format!(
                "INSERT INTO memories (id, source_id, chunk_index, title, last_modified, \
                 chunk_type, source, content, version, access_count) VALUES \
                 ('m#{chunk}', ?1, {chunk}, '', 0, 'text', 'memory', 'part', 3, 7)"
            ),
            "mem_1",
        )
        .await;
    }

    let out = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let target = out.path().join("export");
    export_lossless(
        &db,
        &crate::store::export::run::PagesFolder::default(),
        data_root.path(),
        &target,
    )
    .await
    .unwrap();

    // The rows as the file rebuild leaves them: everything but history.
    let (fresh, _fresh_dir) = test_db().await;
    for table in ["pages", "memories"] {
        let copied = db.export_table(table).await.unwrap().unwrap();
        fresh
            .rebuild_replace_table(table, &copied.columns, &copied.rows)
            .await
            .unwrap();
    }
    sql(
        &fresh,
        "UPDATE pages SET version = 1, user_edited = 0, incarnation = 0, changelog = NULL \
         WHERE id = ?1",
        "p1",
    )
    .await;
    sql(
        &fresh,
        "UPDATE memories SET version = 1, access_count = 0 WHERE source_id = ?1",
        "mem_1",
    )
    .await;
    assert!(!compare_tables(&db, &fresh, &["pages", "memories"])
        .await
        .unwrap()
        .is_empty());

    let (set, problems) = apply_states(&target, &fresh).await.unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(set, 2);
    let diffs = compare_tables(&db, &fresh, &["pages", "memories"])
        .await
        .unwrap();
    assert!(diffs.is_empty(), "{diffs:#?}");
}

#[tokio::test]
async fn pages_and_entities_rebuild_from_their_files() {
    use super::compare::compare_keyed;
    use crate::store::export::DEFERRED_COLUMNS;

    let (db, _db_dir) = test_db().await;
    let page = |id: &'static str, title: &'static str, content: &'static str| {
        let db = &db;
        async move {
            db.insert_page_with_kind(
                id,
                title,
                None,
                content,
                None,
                Some("sp"),
                &[],
                "2026-10-31T00:00:00Z",
                "authored",
                "confirmed",
                None,
                None,
            )
            .await
            .unwrap();
        }
    };
    page(
        "p1",
        "Alpha",
        "The launch moved [1]. Again [1], and see [2].",
    )
    .await;
    sql(
        &db,
        "UPDATE pages SET summary = 'Short.', version = 3, user_edited = 1, \
         citations = '[{\"occurrence\":1,\"marker\":1,\"source_kind\":\"memory\",\"locator\":\"mem_1\",\
         \"score\":0.8,\"status\":\"verified\",\"scope\":\"sentence\",\"cited_version\":2},\
         {\"occurrence\":2,\"marker\":1,\"source_kind\":\"memory\",\"locator\":\"mem_1\",\
         \"score\":0.5,\"status\":\"verified\",\"scope\":\"sentence\",\"cited_version\":2},\
         {\"occurrence\":3,\"marker\":2,\"source_kind\":\"url\",\"locator\":\"https://a.example/x\",\
         \"score\":0.1,\"status\":\"unverified\",\"scope\":\"paragraph\"}]' WHERE id = ?1",
        "p1",
    )
    .await;
    page("p2", "Beta", "About Bob.").await;
    sql(
        &db,
        "UPDATE pages SET entity_id = 'ent_bob', status = 'archived' WHERE id = ?1",
        "p2",
    )
    .await;
    page("p3", "Gamma", "Odd status.").await;
    sql(
        &db,
        "UPDATE pages SET status = 'failed' WHERE id = ?1",
        "p3",
    )
    .await;
    page("shadow", "Bob", "Bob runs the launch.").await;
    sql(
        &db,
        "UPDATE pages SET kind = 'entity', entity_id = 'ent_bob', entity_type = 'person', \
         entity_confirmed = 1, aliases = '[\"Robert\"]', established_by = 'human', \
         entity_created_at = 1793404800, entity_updated_at = 1793404900 WHERE id = ?1",
        "shadow",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let target = out.path().join("export");
    export_lossless(
        &db,
        &crate::store::export::run::PagesFolder::default(),
        data_root.path(),
        &target,
    )
    .await
    .unwrap();

    let (fresh, _fresh_dir) = test_db().await;
    let rebuilt = super::rebuild(&target, &fresh).await.unwrap();
    assert!(rebuilt.problems.is_empty(), "{:?}", rebuilt.problems);
    assert_eq!(rebuilt.tables["pages"], 4);

    let cache: Vec<&str> = DEFERRED_COLUMNS
        .iter()
        .filter(|(table, to, _)| *table == "pages" && *to == "cache")
        .flat_map(|(_, _, columns)| columns.iter().copied())
        .collect();
    let diff = compare_keyed(&db, &fresh, "pages", &["id"], &cache)
        .await
        .unwrap();
    assert!(diff.is_empty(), "{diff:#?}");
}

/// export(rebuild(export(db))) == export(db), file by file, and the tables
/// copied whole or kept in history are equal row by row.
#[tokio::test]
async fn an_export_of_the_rebuild_is_the_same_export() {
    use super::{compare_exports, export_again, rebuild};

    let (db, _db_dir) = crate::store::export::tests::fixture().await;
    let memory = |id: &'static str, chunk: i64, columns: &'static str, values: String| {
        let db = &db;
        async move {
            sql(
                db,
                &format!(
                    "INSERT INTO memories (id, source_id, chunk_index, title, last_modified, \
                     chunk_type, source, content{columns}) VALUES ('{id}#{chunk}', ?1, {chunk}, \
                     '', 1793404900, 'text'{values})"
                ),
                id,
            )
            .await;
        }
    };
    // 2026-10-31T00:00:00Z
    let at = 1_793_404_800;
    memory(
        "mem_cap",
        0,
        ", created_at, pinned, memory_type, structured_fields, space, origin_class, retrieval_cue",
        format!(
            ", 'memory', 'My own note', {at}, 1, 'preference', '{{\"a\":\"x\",\"b\":1}}', 'sp', \
             'human_capture', 'window seat'"
        ),
    )
    .await;
    sql(
        &db,
        "INSERT INTO enrichment_origin (source_id, memory_type_explicit, \
         structured_fields_explicit, space_rejected) VALUES (?1, 1, 1, 1)",
        "mem_cap",
    )
    .await;
    // A classified type and extracted fields are cache: not in the file.
    memory(
        "mem_auto",
        0,
        ", memory_type, structured_fields, created_at",
        format!(", 'memory', 'Auto typed', 'fact', '{{\"k\":2}}', {at}"),
    )
    .await;
    sql(
        &db,
        "INSERT INTO enrichment_origin (source_id, memory_type_explicit, \
         structured_fields_explicit, space_rejected) VALUES (?1, 0, 0, 0)",
        "mem_auto",
    )
    .await;
    memory("clipboard_1", 0, "", ", 'clipboard', 'copied'".to_string()).await;
    for (chunk, text) in [(0, "Hello?"), (1, "Hi there.")] {
        memory(
            "import_chatgpt_conv1",
            chunk,
            ", title, created_at, space",
            format!(", 'memory', '{text}', 'A chat', {}, 'sp'", at + chunk),
        )
        .await;
    }
    for (chunk, text) in [(0, "Saved page."), (1, "Part two.")] {
        memory(
            "https://example.com/post",
            chunk,
            ", url",
            format!(", 'webpage', '{text}', 'https://example.com/post'"),
        )
        .await;
    }
    memory("docs::README.md", 0, "", ", 'file', '# Notes'".to_string()).await;
    sql(
        &db,
        "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, 'ent_123456789')",
        "mem_cap",
    )
    .await;
    sql(
        &db,
        "INSERT INTO spaces (id, name, created_at, updated_at) VALUES (?1, 'Work', 0, 0)",
        "space_123456789",
    )
    .await;
    sql(
        &db,
        "INSERT INTO briefs (space_id, last_session_summary, last_handoff_at, version, \
         updated_at) VALUES (?1, 'Left off here.', 1793404800, 7, 1793404900)",
        "space_123456789",
    )
    .await;
    for (id, state, gate) in [
        ("item_a", "active", "NULL"),
        ("item_b", "backlog", "'after the export'"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO brief_items (id, space_id, text, state, added_at, gate, version, \
                 updated_at) VALUES ('{id}', ?1, 'Do {id}', '{state}', 5, {gate}, 2, 9)"
            ),
            "space_123456789",
        )
        .await;
    }
    for page_id in ["a", "shadow"] {
        sql(
            &db,
            "INSERT INTO page_maps (page_id, revision, map_schema, viewport, generated_at, \
             created_at, updated_at) VALUES (?1, 4, 1, '{\"zoom\":1.5}', NULL, \
             '2026-01-01 00:00:00', '2026-01-02 00:00:00')",
            page_id,
        )
        .await;
    }
    for (id, status, coordinates, parent) in [
        ("n_placed", "active", "12.5, -40, 300, 80", "NULL"),
        ("n_unplaced", "active", "NULL, 7, NULL, NULL", "'n_placed'"),
        ("n_suggested", "suggested", "1, 2, 3, 4", "NULL"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO page_map_nodes (id, page_id, parent_id, rank, ref_kind, ref_id, \
                 label, status, pinned, placed, collapsed, x, y, width, height, fingerprint, \
                 provenance, created_at, updated_at) VALUES ('{id}', ?1, {parent}, 2, \
                 'memory', 'mem:{id}', 'Label {id}', '{status}', 1, 0, 1, {coordinates}, \
                 'fp_{id}', 'agent', 't0', 't1')"
            ),
            "a",
        )
        .await;
    }
    for (id, to, status) in [
        ("e_shown", "n_unplaced", "active"),
        ("e_hidden", "n_suggested", "suggested"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO page_map_edges (id, page_id, from_node, to_node, kind, label, \
                 status, provenance, created_at) VALUES ('{id}', ?1, 'n_placed', '{to}', \
                 'link', 'why', '{status}', NULL, 't0')"
            ),
            "a",
        )
        .await;
    }

    let out = tempfile::tempdir().unwrap();
    let data_root = tempfile::tempdir().unwrap();
    let first = out.path().join("first");
    export_lossless(
        &db,
        &crate::store::export::run::PagesFolder::default(),
        data_root.path(),
        &first,
    )
    .await
    .unwrap();

    let (fresh, _fresh_dir) = test_db().await;
    let rebuilt = rebuild(&first, &fresh).await.unwrap();
    assert!(rebuilt.problems.is_empty(), "{:?}", rebuilt.problems);

    let second = out.path().join("second");
    export_again(&fresh, &first, &second).await.unwrap();
    let diff = compare_exports(&first, &second).unwrap();
    assert!(diff.is_empty(), "{diff:#?}");

    let mut tables: Vec<&'static str> = vec![
        "briefs",
        "brief_items",
        "page_maps",
        "page_map_nodes",
        "page_map_edges",
    ];
    tables.extend(
        SETTINGS_FILES
            .iter()
            .flat_map(|(_, tables)| tables.iter().copied()),
    );
    tables.extend(RECORD_TABLES.iter().copied());
    tables.extend(HISTORY_TABLES.iter().map(|spec| spec.table));
    let diffs = compare_tables(&db, &fresh, &tables).await.unwrap();
    assert!(diffs.is_empty(), "{diffs:#?}");
}
