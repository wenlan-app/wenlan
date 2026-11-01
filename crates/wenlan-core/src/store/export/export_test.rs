// SPDX-License-Identifier: Apache-2.0
use std::collections::HashMap;

use super::pages::{export_pages, PagesExported};
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
    assert_eq!(
        done,
        PagesExported {
            pages: 4,
            archived: 1,
            entities: 2,
        }
    );

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
