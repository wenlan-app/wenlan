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

pub(crate) async fn fixture() -> (MemoryDB, tempfile::TempDir) {
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
    assert_eq!(done.entity_stems["ent_123456789"], "Bob");

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
        [
            "Prefers window seats · on long flights second chunk",
            "mem_two"
        ],
        "a memory's label is the start of its text, chunks joined in order"
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

    let bob = entity::parse(&read(&dirs, "backstage/entities/Bob.md")).unwrap();
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
    // An episode shares its parent's id and is not part of its text.
    sql(
        &db,
        "INSERT INTO memories (id, source_id, chunk_index, title, last_modified, chunk_type, \
         source, content) VALUES ('episode_row', ?1, 0, '', 0, 'text', 'episode', 'a slice')",
        "mem_long",
    )
    .await;
    memory_row(&db, "mem_odd", 0, "", ", 'mystery', 'who knows'").await;
    memory_row(&db, "clipboard_1", 0, "", ", 'clipboard', 'copied text'").await;
    for (chunk, text) in [(0, "# Notes"), (1, "Body.")] {
        memory_row(
            &db,
            "docs::README.md",
            chunk,
            "",
            &format!(", 'file', '{text}'"),
        )
        .await;
    }
    for (chunk, text) in [(0, "Saved page text."), (1, "Second part.")] {
        memory_row(
            &db,
            "https://example.com/post",
            chunk,
            ", url",
            &format!(", 'webpage', '{text}', 'https://example.com/post'"),
        )
        .await;
    }
    sql(
        &db,
        "UPDATE memories SET retrieval_cue = 'window seat', last_modified = 1793404900 \
         WHERE source_id = ?1",
        "mem_cap",
    )
    .await;
    sql(
        &db,
        "UPDATE enrichment_origin SET space_rejected = 1 WHERE source_id = ?1",
        "mem_cap",
    )
    .await;
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
    assert_eq!((done.memories, done.originals), (3, 2));
    let clip = memory::parse(&read(&dirs, "backstage/memories/undated/clipboard_1.md")).unwrap();
    assert_eq!(clip.text, "copied text");
    assert_eq!(clip.other["source"], "clipboard");
    let doc = crate::store::format::original::parse(&read(
        &dirs,
        "backstage/originals/file/docs README.md.md",
    ))
    .unwrap();
    assert_eq!(doc.external_id, "docs::README.md");
    let texts: Vec<_> = doc.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["# Notes", "Body."]);
    assert_eq!(doc.other["source"], "file");

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
    assert_eq!(cap.other["cue"], "window seat");
    assert_eq!(cap.other["modified"], "2026-10-31T00:01:40Z");
    assert_eq!(cap.other["space_rejected"], true);
    assert!(!cap.other.contains_key("source"));

    let web = crate::store::format::original::parse(&read(
        &dirs,
        "backstage/originals/web/https example.com post.md",
    ))
    .unwrap();
    assert_eq!(web.vendor, "web");
    assert_eq!(web.external_id, "https://example.com/post");
    let texts: Vec<_> = web.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["Saved page text.", "Second part."]);
    assert_eq!(web.other["source"], "webpage");

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
        [
            ("mem_cap", "about[ent_gone]"),
            ("mem_odd", "memory with source mystery"),
            ("mem_pending", "memory")
        ]
    );
    let skipped: Vec<_> = manifest
        .not_files
        .iter()
        .map(|n| (n.what.as_str(), n.count))
        .collect();
    assert_eq!(
        skipped,
        [
            ("document chunks", 1),
            ("memories with source episode", 1),
            ("memories with source page", 1)
        ]
    );
}

#[tokio::test]
async fn an_imported_chat_becomes_an_original_with_its_messages_in_order() {
    use super::memories::export_memories;
    use crate::store::format::original;

    let (db, _db_dir) = test_db().await;
    for (chunk, text, at) in [
        (0, "Where should we go in May?", 1_793_404_800),
        (1, "A few ideas.\n\n## Lisbon\nWarm.", 1_793_404_805),
        (2, "Ends with a line end.\n", 1_793_404_810),
    ] {
        memory_row(
            &db,
            "import_claude_6720-abc",
            chunk,
            ", created_at, url, session_id",
            &format!(", 'memory', '{text}', {at}, 'https://claude.ai/chat/6720-abc', 'sess_1'"),
        )
        .await;
    }
    sql(
        &db,
        "UPDATE memories SET title = 'Trip ideas', pinned = 1, entity_id = 'ent_trip' \
         WHERE source_id = ?1",
        "import_claude_6720-abc",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    let done = export_memories(&db, &HashMap::new(), &dirs, &mut manifest)
        .await
        .unwrap();
    assert_eq!((done.memories, done.originals), (0, 1));
    let chat = original::parse(&read(&dirs, "backstage/originals/claude/6720-abc.md")).unwrap();
    assert_eq!(chat.id, "import_claude_6720-abc");
    assert_eq!(chat.vendor, "claude");
    assert_eq!(chat.title.as_deref(), Some("Trip ideas"));
    assert_eq!(chat.url.as_deref(), Some("https://claude.ai/chat/6720-abc"));
    let texts: Vec<_> = chat.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Where should we go in May?",
            "A few ideas.\n\n## Lisbon\nWarm.",
            "Ends with a line end.\n"
        ]
    );
    assert_eq!(chat.messages[1].at.as_deref(), Some("2026-10-31T00:00:05Z"));
    // The person's pin and entity on the chat are kept.
    assert_eq!(chat.other["pinned"], true);
    assert_eq!(chat.other["entity"], "ent_trip");
    assert!(manifest.left_out.is_empty(), "{:?}", manifest.left_out);
}

#[tokio::test]
async fn each_brief_keeps_its_lists_in_order() {
    use super::briefs::export_briefs;
    use crate::store::format::brief;

    let (db, _db_dir) = test_db().await;
    sql(
        &db,
        "INSERT INTO spaces (id, name, created_at, updated_at) VALUES (?1, 'Work', 0, 0)",
        "space_123456789",
    )
    .await;
    sql(
        &db,
        "INSERT INTO briefs (space_id, last_session_summary, last_handoff_at, version, updated_at) \
         VALUES (?1, 'Left off at the export.', 1793404800, 7, 1793404900)",
        "space_123456789",
    )
    .await;
    for (id, state, added, gate) in [
        ("item_b", "active", 2, "NULL"),
        ("item_a", "active", 1, "NULL"),
        ("item_c", "backlog", 3, "'after <the> export -->'"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO brief_items (id, space_id, text, state, added_at, gate, version, \
                 updated_at) VALUES ('{id}', ?1, 'Do {id}', '{state}', {added}, {gate}, 1, 0)"
            ),
            "space_123456789",
        )
        .await;
    }

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    assert_eq!(export_briefs(&db, &dirs, &mut manifest).await.unwrap(), 1);
    let file = brief::parse(&read(&dirs, "backstage/briefs/Work-456789.md")).unwrap();
    assert_eq!(file.id, "space_123456789");
    assert_eq!(file.space, "Work");
    assert_eq!(file.version, 7);
    assert_eq!(file.summary, "Left off at the export.");
    let active: Vec<_> = file.active.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(active, ["item_a", "item_b"]);
    assert_eq!(
        file.backlog[0].gate.as_deref(),
        Some("after <the> export -->")
    );
    assert!(manifest.left_out.is_empty(), "{:?}", manifest.left_out);
}

#[tokio::test]
async fn a_canvas_shows_accepted_nodes_beside_its_page_and_keeps_the_rest_backstage() {
    use super::canvases::export_canvases;
    use crate::store::format::canvas;

    let (db, _db_dir) = test_db().await;
    page(&db, "p1", "Trip", "Plans.", "sp").await;
    page(&db, "p2", "Loose", "No file for this one.", "sp").await;
    for id in ["p1", "p2"] {
        sql(
            &db,
            "INSERT INTO page_maps (page_id, revision, map_schema, viewport, generated_at, \
             created_at, updated_at) VALUES (?1, 4, 1, '{\"zoom\":1.5}', NULL, \
             '2026-01-01 00:00:00', '2026-01-02 00:00:00')",
            id,
        )
        .await;
    }
    for (id, status, coordinates) in [
        ("n_placed", "active", "12.5, -40, 300, 80"),
        ("n_unplaced", "active", "NULL, 7, NULL, NULL"),
        ("n_suggested", "suggested", "1, 2, 3, 4"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO page_map_nodes (id, page_id, parent_id, rank, ref_kind, ref_id, \
                 label, status, pinned, placed, collapsed, x, y, width, height, fingerprint, \
                 provenance, created_at, updated_at) VALUES ('{id}', ?1, NULL, 1.25, 'memory', \
                 'mem_{id}', 'Label {id}', '{status}', 1, 0, 0, {coordinates}, 'fp_{id}', \
                 'agent', 't0', 't1')"
            ),
            "p1",
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
                 'link', NULL, '{status}', NULL, 't0')"
            ),
            "p1",
        )
        .await;
    }

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    let page_paths = HashMap::from([("p1".to_string(), "visible/Trips/Trip.md".to_string())]);
    let done = export_canvases(&db, &page_paths, &dirs, &mut manifest)
        .await
        .unwrap();
    assert_eq!((done.canvases, done.hidden), (2, 1));

    let shown = canvas::parse(&read(&dirs, "visible/Trips/Trip.canvas")).unwrap();
    let ids: Vec<_> = shown.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["n_placed", "n_unplaced"]);
    assert_eq!(shown.nodes[0].x.to_string(), "12.5");
    assert_eq!(shown.nodes[0].y.to_string(), "-40");
    let placed = shown.nodes[0].wenlan.as_ref().unwrap();
    assert_eq!(placed.reference.as_deref(), Some("memory:mem_n_placed"));
    assert!(placed.accepted);
    assert_eq!(placed.rest["rank"], serde_json::json!(1.25));
    assert!(!placed.rest.contains_key("unset"));
    let unplaced = shown.nodes[1].wenlan.as_ref().unwrap();
    assert_eq!(
        unplaced.rest["unset"],
        serde_json::json!(["x", "width", "height"])
    );
    assert_eq!(shown.nodes[1].y.to_string(), "7");
    let edges: Vec<_> = shown.edges.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(edges, ["e_shown"]);
    assert_eq!(shown.rest["wenlan"]["revision"], serde_json::json!(4));
    assert_eq!(
        shown.rest["wenlan"]["viewport"],
        serde_json::json!("{\"zoom\":1.5}")
    );

    let hidden = canvas::parse(&read(&dirs, "backstage/proposals/canvas/p1.canvas")).unwrap();
    // The suggested node, and a copy of the shown node its line starts at.
    assert_eq!(hidden.nodes.len(), 2);
    assert!(!hidden.nodes[0].wenlan.as_ref().unwrap().accepted);
    let copy = hidden.nodes[1].wenlan.as_ref().unwrap();
    assert_eq!(hidden.nodes[1].id, "n_placed");
    assert_eq!(copy.rest["shown"], true);
    assert_eq!(hidden.edges[0].id, "e_hidden");

    let loose = canvas::parse(&read(&dirs, "backstage/layout/canvas/p2.canvas")).unwrap();
    assert!(loose.nodes.is_empty());
    assert_eq!(manifest.files.len(), 3);
    assert!(manifest.left_out.is_empty(), "{:?}", manifest.left_out);
}

#[tokio::test]
async fn artifact_originals_are_copied_byte_for_byte_with_their_records() {
    use super::artifacts::{export_artifacts, RECORDS_PATH};

    let (db, _db_dir) = test_db().await;
    let data_root = tempfile::tempdir().unwrap();
    let kept = data_root.path().join("originals/claude/conv1");
    std::fs::create_dir_all(&kept).unwrap();
    let bytes = b"<svg>\r\n\xff not utf-8</svg>";
    std::fs::write(kept.join("art1.svg"), bytes).unwrap();
    for (page_id, artifact, file) in [
        ("orig_kept", "art1", "originals/claude/conv1/art1.svg"),
        ("orig_lost", "art2", "originals/claude/conv1/art2.md"),
    ] {
        page(&db, page_id, page_id, "", "sp").await;
        sql(
            &db,
            &format!(
                "INSERT INTO originals (page_id, vendor, conversation_id, artifact_id, kind, \
                 title, file_path, conversation_url, written_at, imported_at) VALUES (?1, \
                 'claude', 'conv1', '{artifact}', 'image/svg+xml', NULL, '{file}', \
                 'https://claude.ai/chat/conv1', NULL, 1793404800)"
            ),
            page_id,
        )
        .await;
    }

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    let copied = export_artifacts(&db, data_root.path(), &dirs, &mut manifest)
        .await
        .unwrap();
    assert_eq!(copied, 1);
    let written =
        std::fs::read(dirs.root.join("backstage/originals/claude/conv1/art1.svg")).unwrap();
    assert_eq!(written, bytes);
    assert_eq!(manifest.files[0].sha256, sha256(bytes));
    let records: serde_json::Value = serde_json::from_str(&read(&dirs, RECORDS_PATH)).unwrap();
    assert_eq!(records[0]["page"], "orig_kept");
    assert_eq!(records[1]["file"], "originals/claude/conv1/art2.md");
    assert_eq!(manifest.left_out.len(), 1);
    assert_eq!(manifest.left_out[0].id, "orig_lost");
}

#[tokio::test]
async fn settings_tables_are_copied_whole_with_their_stored_types() {
    use super::settings::{export_settings, SETTINGS_FILES};

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
        "export_test_bytes",
    )
    .await;
    sql(
        &db,
        "INSERT INTO agent_connections (id, name, trust_level, created_at, updated_at) \
         VALUES (?1, 'codex', 'review', 1, 1)",
        "agent_c",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    assert!(export_settings(&db, &dirs, &mut manifest).await.unwrap() >= 3);
    assert_eq!(manifest.files.len(), SETTINGS_FILES.len());

    let file = |name: &str| -> serde_json::Value {
        serde_json::from_str(&read(&dirs, &format!("backstage/settings/{name}.json"))).unwrap()
    };
    let row = |value: &serde_json::Value, table: &str, key: &str| -> serde_json::Map<_, _> {
        let table = &value["tables"][table];
        let columns = table["columns"].as_array().unwrap();
        let row = table["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row[0] == key)
            .unwrap_or_else(|| panic!("{key} missing"));
        columns
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .zip(row.as_array().unwrap().iter().cloned())
            .collect()
    };
    let space = row(&file("spaces"), "spaces", "space_w");
    // A real stays a real and an integer an integer.
    assert_eq!(space["created_at"].to_string(), "1.5");
    assert_eq!(space["sort_order"], serde_json::json!(0));
    let agent = row(&file("agents"), "agent_connections", "agent_c");
    assert_eq!(agent["trust_level"], "review");
    let meta = row(&file("metadata"), "app_metadata", "export_test_bytes");
    assert_eq!(meta["value"], serde_json::json!({"hex": "00ff"}));
    assert!(file("things")["tables"]["thing_sessions"]["rows"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(manifest.left_out.is_empty(), "{:?}", manifest.left_out);
}

#[test]
fn every_file_class_table_has_an_export_part() {
    use super::records::RECORD_TABLES;
    use super::settings::SETTINGS_FILES;
    use super::FILE_TABLE_HOMES;
    use crate::db::table_classes::{TableClass, TABLE_CLASSES};
    use std::collections::BTreeSet;

    let file_tables: BTreeSet<&str> = TABLE_CLASSES
        .iter()
        .filter(|(_, class)| *class == TableClass::File)
        .map(|(table, _)| *table)
        .collect();
    let homes: BTreeSet<&str> = FILE_TABLE_HOMES.iter().map(|(table, _)| *table).collect();
    assert_eq!(homes, file_tables, "a file-class table has no export part");

    let home = |part: &str| -> BTreeSet<&str> {
        FILE_TABLE_HOMES
            .iter()
            .filter(|(_, home)| *home == part)
            .map(|(table, _)| *table)
            .collect()
    };
    let records: BTreeSet<&str> = RECORD_TABLES.iter().copied().collect();
    assert_eq!(home("records"), records);
    // `thing_sessions` rides with `things`; it is classed history.
    let settings: BTreeSet<&str> = SETTINGS_FILES
        .iter()
        .flat_map(|(_, tables)| tables.iter().copied())
        .filter(|table| *table != "thing_sessions")
        .collect();
    assert_eq!(home("settings"), settings);
}

#[tokio::test]
async fn records_keep_every_confirmation_and_annotation_column() {
    use super::records::{export_records, RECORD_TABLES};

    let (db, _db_dir) = test_db().await;
    sql(
        &db,
        "INSERT INTO source_annotations (source, source_id, context_key, notes, page_ids, \
         version, updated_at) VALUES ('memory', ?1, 'space:sp', 'Check the date.', '[]', 2, 9)",
        "mem_1",
    )
    .await;
    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    assert_eq!(export_records(&db, &dirs, &mut manifest).await.unwrap(), 1);
    assert_eq!(manifest.files.len(), RECORD_TABLES.len());
    let file: serde_json::Value =
        serde_json::from_str(&read(&dirs, "backstage/records/source_annotations.json")).unwrap();
    let table = &file["tables"]["source_annotations"];
    assert_eq!(
        table["columns"],
        serde_json::json!([
            "source",
            "source_id",
            "context_key",
            "notes",
            "page_ids",
            "updated_at",
            "version"
        ])
    );
    assert_eq!(
        table["rows"][0],
        serde_json::json!(["memory", "mem_1", "space:sp", "Check the date.", "[]", 9, 2])
    );
    assert!(manifest.left_out.is_empty(), "{:?}", manifest.left_out);
}

#[tokio::test]
async fn names_never_clash_and_page_and_entity_state_is_kept() {
    let (db, _db_dir) = test_db().await;
    page(&db, "p1", "Beta", "one", "sp").await;
    page(&db, "p2", "beta-p3", "two", "sp").await;
    page(&db, "p3", "beta", "three", "sp").await;
    sql(
        &db,
        "UPDATE pages SET summary = 'The short claim.' WHERE id = ?1",
        "p1",
    )
    .await;
    // A page distilled about an entity keeps that entity, as stored.
    sql(
        &db,
        "UPDATE pages SET entity_id = 'ent_bob', entity_type = 'person' WHERE id = ?1",
        "p2",
    )
    .await;
    page(&db, "shadow", "Bob", "Bob ran it.", "sp").await;
    sql(
        &db,
        "UPDATE pages SET kind = 'entity', entity_id = 'ent_bob', status = 'archived', \
         entity_created_at = 1793404800 WHERE id = ?1",
        "shadow",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    let done = export_pages(&db, &HashMap::new(), &dirs, &mut manifest)
        .await
        .unwrap();
    assert_eq!((done.pages, done.entities), (3, 1));
    let p3 = page::parse(&read(&dirs, "visible/beta-p3-2.md"), "").unwrap();
    assert_eq!(p3.id.as_deref(), Some("p3"));
    let p1 = page::parse(&read(&dirs, "visible/Beta.md"), "").unwrap();
    assert_eq!(p1.other["summary"], "The short claim.");

    let bob = entity::parse(&read(&dirs, "backstage/entities/Bob.md")).unwrap();
    assert_eq!(bob.other["status"], "archived");
    assert_eq!(bob.other["first_seen"], "2026-10-31T00:00:00Z");
    assert!(bob.other.contains_key("created"));
    let left: Vec<_> = manifest
        .left_out
        .iter()
        .map(|l| (l.id.as_str(), l.field.as_str()))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    let p2 = page::parse(&read(&dirs, "visible/beta-p3.md"), "").unwrap();
    assert_eq!(p2.other["entity_id"], "ent_bob");
    assert_eq!(p2.other["entity_type"], "person");
}

#[tokio::test]
async fn every_column_of_a_field_by_field_table_is_written_or_deferred() {
    use super::{DEFERRED_COLUMNS, FILE_COLUMNS};
    use std::collections::BTreeSet;

    let (db, _db_dir) = test_db().await;
    let mut problems = Vec::new();
    for (table, written) in FILE_COLUMNS {
        let live: BTreeSet<String> = db
            .export_table(table)
            .await
            .unwrap()
            .unwrap()
            .columns
            .into_iter()
            .collect();
        let mut listed: BTreeSet<String> = written.iter().map(|c| c.to_string()).collect();
        for (deferred_table, _, columns) in DEFERRED_COLUMNS {
            if deferred_table == table {
                for column in *columns {
                    assert!(
                        listed.insert(column.to_string()),
                        "{table}.{column} is listed twice"
                    );
                }
            }
        }
        let missing: Vec<_> = live.difference(&listed).collect();
        let gone: Vec<_> = listed.difference(&live).collect();
        if !missing.is_empty() || !gone.is_empty() {
            problems.push(format!(
                "{table}: not written or deferred {missing:?}; listed but not in the schema {gone:?}"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_history_class_table_has_a_place() {
    use super::history::{HISTORY_ELSEWHERE, HISTORY_TABLES};
    use crate::db::table_classes::{TableClass, TABLE_CLASSES};
    use std::collections::BTreeSet;

    let history: BTreeSet<&str> = TABLE_CLASSES
        .iter()
        .filter(|(_, class)| *class == TableClass::History)
        .map(|(table, _)| *table)
        .collect();
    let placed: BTreeSet<&str> = HISTORY_TABLES
        .iter()
        .map(|spec| spec.table)
        .chain(HISTORY_ELSEWHERE.iter().map(|(table, _)| *table))
        .collect();
    assert_eq!(
        placed, history,
        "a history-class table has no place in the export"
    );
}

#[tokio::test]
async fn replaying_the_history_log_gives_back_every_row() {
    use super::history::{export_history, Log, HISTORY_TABLES};
    use super::settings::cell;
    use crate::store::history::{self, ActorKind};

    let (db, _db_dir) = test_db().await;
    page(&db, "p1", "Alpha", "Version one.", "sp").await;
    let long = "x".repeat(40 * 1024);
    for (version, content, edited_by) in [
        (2, "Version two.", "manual_edit"),
        (3, long.as_str(), "distill"),
    ] {
        sql(
            &db,
            &format!(
                "INSERT INTO page_history (page_id, version, content, edited_by, created_at, \
                 title, minor) VALUES (?1, {version}, '{content}', '{edited_by}', \
                 '2026-03-0{version}T00:00:00Z', 'Alpha', 0)"
            ),
            "p1",
        )
        .await;
    }
    sql(
        &db,
        "INSERT INTO thing_history (ts, actor, op, thing_id, session_id, data) \
         VALUES (1793404800, 'person', 'thing.rename', ?1, NULL, '{\"title\":\"Trip\"}')",
        "thing_1",
    )
    .await;
    sql(
        &db,
        "INSERT INTO access_log (source_id, accessed_at) VALUES (?1, 1793404801)",
        "mem_1",
    )
    .await;
    // No time column: stamped with the export time, never 1970.
    sql(
        &db,
        "INSERT INTO enrichment_origin (source_id, memory_type_explicit, \
         structured_fields_explicit, space_rejected) VALUES (?1, 1, 0, 0)",
        "mem_1",
    )
    .await;

    let out = tempfile::tempdir().unwrap();
    let dirs = ExportDirs::create(out.path().join("export")).unwrap();
    let mut manifest = Manifest::new();
    let done = export_history(&db, &dirs, &mut manifest, &Default::default())
        .await
        .unwrap();
    assert!(done.entries >= 4 && done.reads >= 1, "{done:?}");
    assert!(manifest
        .files
        .iter()
        .any(|file| file.path.starts_with("backstage/history/blobs/")));
    assert!(!manifest
        .files
        .iter()
        .any(|file| file.path.ends_with(".lock")));

    let root = dirs.backstage().join("history");
    assert!(!root.join("1970-01").exists(), "an entry claims 1970");
    let mut rebuilt: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for (log_root, log) in [(root.clone(), Log::Main), (root.join("reads"), Log::Reads)] {
        let replay = history::replay(&log_root).unwrap();
        assert!(replay.report.is_clean(), "{:?}", replay.report);
        for entry in &replay.entries {
            let data = entry.data.as_ref().unwrap();
            if data.get("state_of").is_some() {
                assert_eq!(entry.op, "state.seed");
                assert_eq!(log, Log::Main);
                continue;
            }
            let table = data["table"].as_str().unwrap().to_string();
            if table == "enrichment_origin" {
                assert_eq!(data["time_unknown"], true);
            }
            let spec = HISTORY_TABLES.iter().find(|s| s.table == table).unwrap();
            assert_eq!(spec.log, log, "{table} is in the wrong log");
            let mut row = data["row"].as_object().unwrap().clone();
            if let Some(text) = history::read_text(&log_root, entry).unwrap() {
                let column = if table == "page_history" {
                    "content"
                } else {
                    "text"
                };
                row.insert(column.to_string(), serde_json::json!(text));
            }
            if table == "page_history" && row["edited_by"] == "manual_edit" {
                assert_eq!(entry.actor.kind, ActorKind::Person);
                assert_eq!(entry.op, "page.write");
                assert_eq!(entry.version, Some(2));
            }
            if table == "page_history" && row["edited_by"] == "distill" {
                assert_eq!(entry.actor.kind, ActorKind::Ai);
                assert_eq!(data["text_column"], "content");
            }
            rebuilt
                .entry(table)
                .or_default()
                .push(serde_json::Value::Object(row));
        }
    }
    for spec in HISTORY_TABLES {
        let Some(stored) = db.export_table(spec.table).await.unwrap() else {
            continue;
        };
        let stored: Vec<serde_json::Value> = stored
            .rows
            .iter()
            .map(|values| {
                serde_json::Value::Object(
                    stored
                        .columns
                        .iter()
                        .cloned()
                        .zip(values.iter().map(cell))
                        .collect(),
                )
            })
            .collect();
        let mut got = rebuilt.remove(spec.table).unwrap_or_default();
        let key = |row: &serde_json::Value| row.to_string();
        let mut want = stored;
        got.sort_by_key(key);
        want.sort_by_key(key);
        assert_eq!(got, want, "{} does not replay to its rows", spec.table);
    }
}

#[tokio::test]
async fn one_call_exports_everything_and_writes_the_manifest_last() {
    use super::run::export_lossless;
    use super::MANIFEST_FILE;

    let (db, _db_dir) = fixture().await;
    let data_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_root.path().join("layout/versions")).unwrap();
    std::fs::write(data_root.path().join("layout/palace.json"), "{}\n").unwrap();
    std::fs::write(data_root.path().join("layout/versions/1.json"), "{}\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("export");
    let placement = HashMap::from([("a".to_string(), "Plans/a.md".to_string())]);
    let folder = super::run::PagesFolder::recorded(placement);
    let summary = export_lossless(&db, &folder, data_root.path(), &target)
        .await
        .unwrap();
    assert_eq!((summary.pages, summary.archived), (4, 1));
    assert_eq!(summary.layout_files, 2);
    assert!(target.join("visible/Plans/Alpha.md").is_file());
    assert!(target.join("backstage/layout/versions/1.json").is_file());
    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(target.join(MANIFEST_FILE)).unwrap())
            .unwrap();
    assert_eq!(manifest.files.len(), summary.files);
    assert!(!manifest.deferred.is_empty());
    for file in &manifest.files {
        let bytes = std::fs::read(target.join(&file.path)).unwrap();
        assert_eq!(sha256(&bytes), file.sha256, "{}", file.path);
    }
    // A second run into the same folder is refused.
    assert!(export_lossless(&db, &folder, data_root.path(), &target)
        .await
        .is_err());
}

#[tokio::test]
async fn the_pages_folder_is_read_and_nothing_in_it_is_lost() {
    use super::run::{export_lossless, PagesFolder};

    let (db, _db_dir) = fixture().await;
    let pages = tempfile::tempdir().unwrap();
    let write = |relative: &str, text: &str| {
        let path = pages.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("Plans/a.md", "---\norigin_id: a\n---\nAlpha\n");
    // state.json forgot page b; its file still says whose it is.
    write(
        "Notes/Beta page.md",
        "---\norigin_id: b\ntitle: Beta\n---\nBeta\n",
    );
    write(
        "external-compile-topic-001.md",
        "a file no page row claims\n",
    );
    write("deleted.md", "---\norigin_id: deleted\n---\nold\n");
    // Edited in another app after Wenlan last wrote it.
    write("Rust.md", "---\norigin_id: c\n---\nEdited elsewhere.\n");
    write(".wenlan/state.json", "{}");
    write(".obsidian/app.json", "{}");
    let before = snapshot(pages.path());

    let recorded = HashMap::from([
        ("a".to_string(), "Plans/a.md".to_string()),
        ("deleted".to_string(), "deleted.md".to_string()),
        ("deleted_too".to_string(), "missing.md".to_string()),
        ("c".to_string(), "Rust.md".to_string()),
    ]);
    let digests = HashMap::from([
        (
            "a".to_string(),
            super::sha256(b"---\norigin_id: a\n---\nAlpha\n"),
        ),
        ("c".to_string(), super::sha256(b"what Wenlan wrote")),
    ]);
    let folder = PagesFolder {
        dir: Some(pages.path().to_path_buf()),
        recorded,
        digests,
    };
    let data_root = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("export");
    let summary = export_lossless(&db, &folder, data_root.path(), &target)
        .await
        .unwrap();
    assert_eq!(
        snapshot(pages.path()),
        before,
        "the pages folder is never written"
    );
    assert_eq!(summary.found_by_id, 1);
    assert_eq!(summary.stale_records, 2);
    assert_eq!(summary.folder_only, 2);
    // c was edited; b has no digest (state.json forgot it); a is as written.
    assert_eq!(summary.edited_on_disk, 1);
    assert_eq!(summary.no_digest, 1);
    assert_eq!(
        std::fs::read_to_string(target.join("backstage/.pages-folder/edited/Rust.md")).unwrap(),
        "---\norigin_id: c\n---\nEdited elsewhere.\n"
    );
    assert!(target
        .join("backstage/.pages-folder/edited/Notes/Beta page.md")
        .is_file());
    assert!(!target
        .join("backstage/.pages-folder/edited/Plans/a.md")
        .exists());
    assert!(target.join("visible/Plans/Alpha.md").is_file());
    assert!(target.join("visible/Notes/Beta.md").is_file());
    assert_eq!(
        std::fs::read_to_string(
            target.join("backstage/.pages-folder/external-compile-topic-001.md")
        )
        .unwrap(),
        "a file no page row claims\n"
    );
    assert!(target.join("backstage/.pages-folder/deleted.md").is_file());
    assert!(!target.join("backstage/.pages-folder/.wenlan").exists());
}

/// Every file under `dir` with its bytes and modified time.
fn snapshot(dir: &std::path::Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut found = Vec::new();
    for entry in walk(dir) {
        let bytes = std::fs::read(&entry).unwrap();
        let modified = std::fs::metadata(&entry).unwrap().modified().unwrap();
        found.push((entry.display().to_string(), bytes, modified));
    }
    found.sort();
    found
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
