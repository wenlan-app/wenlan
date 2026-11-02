// SPDX-License-Identifier: Apache-2.0
use std::collections::HashMap;

use super::pages::export_pages;
use super::{sha256, ExportDirs, Manifest};
use crate::db::tests::test_db;
use crate::db::MemoryDB;
use crate::store::format::entity;
use crate::store::format::page::{self, Locator, PageKind};

const T0: &str = "2026-01-01T00:00:00Z";

async fn page(db: &MemoryDB, id: &str, title: &str, content: &str, space: &str) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        content,
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

async fn sql(db: &MemoryDB, statement: &str, id: &str) {
    db.export_fixture_sql(statement, id).await;
}

fn citation(occurrence: u32, marker: u32, locator: &str, version: Option<i64>) -> String {
    serde_json::json!({
        "occurrence": occurrence,
        "marker": marker,
        "source_kind": "memory",
        "locator": locator,
        "score": 0.5,
        "status": "verified",
        "scope": "sentence",
        "cited_version": version,
    })
    .to_string()
}

fn read(dirs: &ExportDirs, relative: &str) -> String {
    std::fs::read_to_string(dirs.root.join(relative))
        .unwrap_or_else(|error| panic!("{relative}: {error}"))
}

async fn fixture() -> (MemoryDB, tempfile::TempDir) {
    let (db, dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    // Line ends at either end, a CRLF and a details part are kept as stored.
    let beta = "\nPlans built on Alpha [1] and [2], again [1].\r\n\n<!-- wenlan:details -->\n## Details for AI\n\nMore.\n";
    page(&db, "b", "Beta", beta, "sp").await;
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();
    let citations = format!(
        "[{},{},{},{}]",
        citation(1, 1, "mem_one", Some(3)),
        citation(2, 2, "mem_two", None),
        citation(3, 1, "mem_one", Some(3)),
        citation(4, 2, "mem_other", Some(1)),
    );
    db.set_page_citations_for_test("b", &citations)
        .await
        .unwrap();
    for (chunk, text) in [
        (1, "second chunk"),
        (0, "Prefers window seats · on long flights"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
                 last_modified, chunk_type) VALUES ('c{chunk}', '{text}', 'memory', ?1, '', \
                 {chunk}, 0, 'text')"
            ),
            "mem_one",
        )
        .await;
    }
    page(&db, "c", "Rust", "A concept page.", "sp").await;
    sql(&db, "UPDATE pages SET kind = 'concept' WHERE id = ?1", "c").await;
    page(&db, "x", "beta", "Same name as Beta, other case.", "sp").await;
    page(&db, "gone", "Gone", "Archived text.", "sp").await;
    sql(
        &db,
        "UPDATE pages SET status = 'archived' WHERE id = ?1",
        "gone",
    )
    .await;
    page(&db, "shadow", "Bob", "Bob runs the launch.", "sp").await;
    sql(
        &db,
        "UPDATE pages SET kind = 'entity', creation_kind = 'entity', \
         entity_id = 'ent_123456789', entity_type = 'person', entity_confirmed = 1, \
         aliases = '[\"Robert\"]', established_by = 'human' WHERE id = ?1",
        "shadow",
    )
    .await;
    page(&db, "odd", "Odd", "", "sp").await;
    sql(
        &db,
        "UPDATE pages SET kind = 'entity', entity_id = 'ent_odd', aliases = 'not a list' \
         WHERE id = ?1",
        "odd",
    )
    .await;
    (db, dir)
}

#[tokio::test]
async fn every_page_row_becomes_a_file_that_reads_back_as_stored() {
    let (db, _db_dir) = fixture().await;
    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let placement = HashMap::from([
        ("a".to_string(), "Trips/Alpha.md".to_string()),
        ("gone".to_string(), "archive/Old/Gone.md".to_string()),
        ("x".to_string(), "../escape.md".to_string()),
    ]);
    let mut manifest = Manifest::new();
    let done = export_pages(&db, &placement, &dirs, &mut manifest)
        .await
        .unwrap();
    assert_eq!((done.pages, done.archived, done.entities), (4, 1, 2));
    assert_eq!(done.entity_stems["ent_123456789"], "Bob-456789");

    let alpha = page::parse(&read(&dirs, "visible/Trips/Alpha.md"), "x").unwrap();
    assert_eq!(alpha.id.as_deref(), Some("a"));
    assert_eq!(alpha.body, "The launch is on 2026-03-01.");
    assert_eq!(alpha.tags, ["sp"]);

    let beta = page::parse(&read(&dirs, "visible/Beta.md"), "x").unwrap();
    assert_eq!(
        beta.body,
        "\nPlans built on Alpha [1] and [2], again [1].\r\n\n<!-- wenlan:details -->\n## Details for AI\n\nMore.\n"
    );
    let lines: Vec<_> = beta
        .citations
        .iter()
        .map(|c| (c.marker, c.locator.clone()))
        .collect();
    assert_eq!(
        lines,
        [
            (
                1,
                Locator::Wenlan {
                    id: "mem_one".into(),
                    version: 3
                }
            ),
            (2, Locator::Other("wenlan:mem_two".into())),
        ]
    );
    let labels: Vec<_> = beta.citations.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Prefers window seats · on long flights", "mem_two"],
        "a memory's label is the start of its first chunk"
    );
    assert_eq!(beta.basis.len(), 1);
    assert_eq!(beta.basis[0].label, "[[Alpha]]");
    assert_eq!(
        beta.basis[0].locator,
        Locator::Wenlan {
            id: "a".into(),
            version: 1
        }
    );

    // A recorded path outside the folder is not kept; the name is taken
    // case-insensitively, so the second page gets the end of its id.
    let other = page::parse(&read(&dirs, "visible/beta-x.md"), "x").unwrap();
    assert_eq!(other.id.as_deref(), Some("x"));

    let rust = page::parse(&read(&dirs, "visible/Rust.md"), "x").unwrap();
    assert_eq!(
        rust.kind,
        PageKind::Page,
        "kind is not read until it can be trusted"
    );

    let gone = page::parse(&read(&dirs, "backstage/archive/Old/Gone.md"), "x").unwrap();
    assert_eq!(gone.body, "Archived text.");

    let bob = entity::parse(&read(&dirs, "backstage/entities/Bob-456789.md")).unwrap();
    assert_eq!(bob.id, "ent_123456789");
    assert_eq!(bob.page.as_deref(), Some("shadow"));
    assert_eq!(bob.aliases, ["Robert"]);
    assert!(bob.confirmed);
    assert_eq!(bob.entity_type.as_deref(), Some("person"));
    assert_eq!(bob.description, "Bob runs the launch.");

    // Every file is in the manifest with the digest of its bytes.
    assert_eq!(manifest.files.len(), 7);
    for file in &manifest.files {
        assert_eq!(
            file.sha256,
            sha256(read(&dirs, &file.path).as_bytes()),
            "{}",
            file.path
        );
    }
    // What the files cannot hold is listed, not dropped.
    let left: Vec<_> = manifest
        .left_out
        .iter()
        .map(|l| (l.id.as_str(), l.field.as_str()))
        .collect();
    assert_eq!(left, [("b", "citations[occurrence 4]"), ("odd", "aliases")]);
}

#[tokio::test]
async fn an_export_never_writes_into_a_folder_with_files() {
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("keep.md"), "mine").unwrap();
    assert!(ExportDirs::create(out.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(out.path().join("keep.md")).unwrap(),
        "mine"
    );

    let empty = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(empty.path()).unwrap();
    dirs.write("visible/a.md", "a", "page", "one").unwrap();
    assert!(dirs.write("visible/a.md", "a", "page", "two").is_err());
    assert!(dirs.write("../out.md", "a", "page", "x").is_err());
    assert!(dirs.write("/abs.md", "a", "page", "x").is_err());
    assert_eq!(read(&dirs, "visible/a.md"), "one");
}

async fn memory_row(db: &MemoryDB, id: &str, chunk: i64, columns: &str, values: &str) {
    sql(
        db,
        &format!(
            "INSERT INTO memories (id, source_id, chunk_index, title, last_modified, chunk_type, \
             source, content{columns}) VALUES ('{id}#{chunk}', ?1, {chunk}, '', 0, 'text'{values})"
        ),
        id,
    )
    .await;
}

#[tokio::test]
async fn each_memory_becomes_one_file_with_what_someone_stated() {
    use super::memories::export_memories;
    use crate::store::format::memory;

    let (db, _db_dir) = test_db().await;
    // 2026-10-31T00:00:00Z
    let at = 1_793_404_800;
    memory_row(
        &db,
        "mem_cap",
        0,
        ", authored_content, origin_class, created_at, pinned, memory_type, structured_fields, space",
        &format!(", 'memory', 'derived', 'My own note\n', 'human_capture', {at}, 1, 'preference', '{{\"b\":1,\"a\":\"x\"}}', 'Work'"),
    )
    .await;
    sql(
        &db,
        "INSERT INTO enrichment_origin (source_id, memory_type_explicit, \
         structured_fields_explicit, space_rejected) VALUES (?1, 1, 1, 0)",
        "mem_cap",
    )
    .await;
    for (chunk, text) in [(0, "part one"), (1, "part two")] {
        memory_row(
            &db,
            "mem_long",
            chunk,
            ", memory_type, source_text, structured_fields",
            &format!(", 'memory', '{text}', 'fact', 'Original prose.', '{{\"k\":2}}'"),
        )
        .await;
    }
    sql(
        &db,
        "INSERT INTO enrichment_origin (source_id, memory_type_explicit, \
         structured_fields_explicit, space_rejected) VALUES (?1, 0, 0, 0)",
        "mem_long",
    )
    .await;
    memory_row(
        &db,
        "mem_doc",
        0,
        ", source_agent",
        ", 'memory', 'a note chunk', 'obsidian'",
    )
    .await;
    memory_row(&db, "page_derived", 0, "", ", 'page', 'from a page'").await;
    memory_row(
        &db,
        "mem_pending",
        0,
        ", pending_revision",
        ", 'memory', 'staged', 1",
    )
    .await;
    sql(
        &db,
        "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, 'ent_bob'), (?1, 'ent_gone')",
        "mem_cap",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let stems = HashMap::from([("ent_bob".to_string(), "Bob-ent_bob".to_string())]);
    let mut manifest = Manifest::new();
    let done = export_memories(&db, &stems, &dirs, &mut manifest)
        .await
        .unwrap();
    assert_eq!(done.memories, 2);

    let cap = memory::parse(&read(&dirs, "backstage/memories/2026-10/mem_cap.md")).unwrap();
    assert_eq!(
        cap.text, "My own note\n",
        "a capture keeps what the person typed"
    );
    assert_eq!(cap.kind.as_deref(), Some("preference"));
    assert_eq!(cap.origin.as_deref(), Some("human_capture"));
    assert_eq!(cap.created.as_deref(), Some("2026-10-31T00:00:00Z"));
    assert!(cap.pinned);
    assert_eq!(cap.space.as_deref(), Some("Work"));
    assert_eq!(cap.about, ["Bob-ent_bob"]);
    let fields: serde_json::Value = serde_yaml::from_value(cap.fields.clone().unwrap()).unwrap();
    assert_eq!(fields, serde_json::json!({"a": "x", "b": 1}));

    let long = memory::parse(&read(&dirs, "backstage/memories/undated/mem_long.md")).unwrap();
    assert_eq!(long.text, "part one\npart two");
    assert_eq!(long.kind, None, "a classified type is not written");
    assert_eq!(long.fields, None, "extracted fields are not written");
    assert_eq!(long.source_text.as_deref(), Some("Original prose."));

    let left: Vec<_> = manifest
        .left_out
        .iter()
        .map(|l| (l.id.as_str(), l.field.as_str()))
        .collect();
    assert_eq!(
        left,
        [("mem_cap", "about[ent_gone]"), ("mem_pending", "memory")]
    );
    let skipped: Vec<_> = manifest
        .not_files
        .iter()
        .map(|n| (n.what.as_str(), n.count))
        .collect();
    assert_eq!(
        skipped,
        [("document chunks", 1), ("memories with source page", 1)]
    );
}
