// SPDX-License-Identifier: Apache-2.0
//! The per-document lookups of a write must use the `(source, source_id)`
//! index. Planned off `idx_memories_source (source=?)` they read every
//! 'memory' row, once per document, and a first import of 10,000 notes slowed
//! down as the table grew. Plans are checked on the bare schema, so these run
//! without the embedding model.

use super::{prefix_upper_bound, MemoryDB, SCHEMA};

async fn schema_conn() -> (libsql::Connection, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = libsql::Builder::new_local(dir.path().join("plan.db"))
        .build()
        .await
        .expect("libsql open");
    let conn = db.connect().expect("libsql connect");
    conn.execute_batch(SCHEMA).await.expect("schema");
    // Columns later migrations add that the checked queries read.
    for column in ["space TEXT", "version INTEGER"] {
        let _ = conn
            .execute(&format!("ALTER TABLE memories ADD COLUMN {column}"), ())
            .await;
    }
    (conn, dir)
}

async fn plan(conn: &libsql::Connection, sql: &str, params: Vec<libsql::Value>) -> String {
    let mut rows = conn
        .query(&format!("EXPLAIN QUERY PLAN {sql}"), params)
        .await
        .expect("explain");
    let mut plan = String::new();
    while let Some(row) = rows.next().await.expect("plan row") {
        plan.push_str(&row.get::<String>(3).unwrap_or_default());
        plan.push('\n');
    }
    plan
}

fn text(values: &[&str]) -> Vec<libsql::Value> {
    values
        .iter()
        .map(|value| libsql::Value::Text((*value).to_string()))
        .collect()
}

#[tokio::test]
async fn per_document_write_lookups_use_the_source_and_source_id_index() {
    let (conn, _dir) = schema_conn().await;
    let note = "obsidian-vault::/vault/a.md::0";
    for (name, sql, params) in [
        (
            "replacement read",
            MemoryDB::REPLACEMENT_SOURCE_SQL,
            text(&["memory", note]),
        ),
        (
            "replacement delete",
            MemoryDB::DELETE_SOURCE_ROWS_SQL,
            text(&["memory", note]),
        ),
        (
            "note chunk lookup",
            MemoryDB::SOURCE_IDS_IN_RANGE_SQL,
            text(&[
                "memory",
                "obsidian-vault::/vault/a.md::",
                "obsidian-vault::/vault/a.md:;",
            ]),
        ),
    ] {
        let plan = plan(&conn, sql, params).await;
        assert!(
            plan.contains("idx_memories_source_source_id"),
            "{name} does not use idx_memories_source_source_id. Plan:\n{plan}"
        );
        assert!(
            !plan.contains("idx_memories_source (source"),
            "{name} scans every row of its source. Plan:\n{plan}"
        );
        // Without the range, the composite index still serves `source=?` and
        // the lookup reads every index entry of the source.
        assert!(
            plan.contains("source_id=?") || plan.contains("source_id>? AND source_id<?"),
            "{name} does not narrow on source_id. Plan:\n{plan}"
        );
    }
}

#[test]
fn prefix_upper_bound_is_just_past_every_string_with_the_prefix() {
    assert_eq!(
        prefix_upper_bound("v::a.md::").as_deref(),
        Some("v::a.md:;")
    );
    assert_eq!(prefix_upper_bound("é").as_deref(), Some("ê"));
    // The character after U+D7FF is U+E000: surrogates are not characters.
    assert_eq!(
        prefix_upper_bound("a\u{D7FF}").as_deref(),
        Some("a\u{E000}")
    );
    // char::MAX cannot be bumped, so the one before it is.
    assert_eq!(prefix_upper_bound("ab\u{10FFFF}").as_deref(), Some("ac"));
    assert_eq!(prefix_upper_bound(""), None);
    assert_eq!(prefix_upper_bound("\u{10FFFF}"), None);

    let prefix = "src::notes/日記.md::";
    let upper = prefix_upper_bound(prefix).expect("bound");
    for inside in [
        prefix.to_string(),
        format!("{prefix}0"),
        format!("{prefix}\u{10FFFF}"),
    ] {
        assert!(
            inside.as_bytes() >= prefix.as_bytes() && inside.as_bytes() < upper.as_bytes(),
            "{inside:?} is outside the range"
        );
    }
    assert!("src::notes/日記.md:;".as_bytes() >= upper.as_bytes());
}
