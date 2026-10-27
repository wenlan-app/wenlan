// SPDX-License-Identifier: Apache-2.0
//! Steep's new pages: the list of uncited material, the claim, and the commit
//! that writes a note.
//!
//! Holds are process-wide, so every test uses its own source ids.

use super::*;
use crate::db::code_basis_test::test_db;

const BREAD_1: &str = "The sourdough starter peaks four to six hours after feeding.";
const BREAD_2: &str = "Below 22 degrees the bulk ferment needs about eight hours.";
const BODY: &str = "The sourdough starter peaks four to six hours after feeding [1]. \
                    Below 22 degrees the bulk ferment needs about eight hours [2].";

#[allow(clippy::too_many_arguments)]
async fn row(
    db: &MemoryDB,
    id: &str,
    source_id: &str,
    chunk: i64,
    title: &str,
    content: &str,
    agent: &str,
    space: &str,
) {
    let now = chrono::Utc::now().timestamp();
    row_at(db, id, source_id, chunk, title, content, agent, space, now).await;
}

/// [`row`], last changed at `now` (unix seconds).
#[allow(clippy::too_many_arguments)]
async fn row_at(
    db: &MemoryDB,
    id: &str,
    source_id: &str,
    chunk: i64,
    title: &str,
    content: &str,
    agent: &str,
    space: &str,
    now: i64,
) {
    let conn = db.test_primary_session().await;
    conn.execute(
        "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'text', 'fact', ?6, ?7, ?8, ?8, 1, 'confirmed', 'memory')",
        libsql::params![id, source_id, title, content, chunk, space, agent, now],
    )
    .await
    .unwrap();
}

async fn capture(db: &MemoryDB, id: &str, content: &str) {
    row(db, id, id, 0, id, content, "claude-code", "test").await;
}

fn ids(items: &[NewMaterial]) -> Vec<&str> {
    items.iter().map(|m| m.item.item_id.as_str()).collect()
}

#[tokio::test]
async fn the_list_folds_notes_and_chats_and_leaves_out_cited_material() {
    let (db, _dir) = test_db().await;
    row(
        &db,
        "l1",
        "lv::bread.md::0",
        0,
        "Bread > Starter",
        BREAD_1,
        "obsidian",
        "test",
    )
    .await;
    row(
        &db,
        "l2",
        "lv::bread.md::1",
        0,
        "Bread > Proofing",
        BREAD_2,
        "obsidian",
        "test",
    )
    .await;
    row(
        &db,
        "l3",
        "import_chatgpt_lc1",
        0,
        "Kyoto",
        "Plan three days in Kyoto.",
        "",
        "test",
    )
    .await;
    row(
        &db,
        "l4",
        "import_chatgpt_lc1",
        1,
        "Kyoto",
        "Day one: Fushimi Inari.",
        "",
        "test",
    )
    .await;
    capture(&db, "l_cited", "Already in a note.").await;
    capture(&db, "l_doc", "Only the file's own source page cites this.").await;
    let now = chrono::Utc::now().to_rfc3339();
    db.insert_page(
        "l_page",
        "A note",
        None,
        "Already in a note [1].",
        None,
        None,
        &["l_cited"],
        &now,
    )
    .await
    .unwrap();
    db.insert_page_with_kind(
        "l_src",
        "A file",
        None,
        "file body",
        None,
        None,
        &["l_doc"],
        &now,
        "source",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();

    let all = new_material(&db, &ReadScope::Global).await.unwrap();
    let mut got = ids(&all);
    got.sort();
    assert_eq!(got, vec!["import_chatgpt_lc1", "l_doc", "lv::bread.md"]);

    let bread = all
        .iter()
        .find(|m| m.item.item_id == "lv::bread.md")
        .unwrap();
    assert_eq!(bread.item.kind, STEEP_NEW_KIND_NOTE);
    assert_eq!(bread.item.title, "Bread");
    assert_eq!(bread.source_ids, vec!["lv::bread.md::0", "lv::bread.md::1"]);
    let chat = all
        .iter()
        .find(|m| m.item.item_id == "import_chatgpt_lc1")
        .unwrap();
    assert_eq!(chat.item.kind, STEEP_NEW_KIND_CHAT);
    assert_eq!(chat.source_ids, vec!["import_chatgpt_lc1"]);
    assert!(chat.item.snippet.starts_with("Plan three days"));

    let other = new_material(&db, &ReadScope::Space("elsewhere".into()))
        .await
        .unwrap();
    assert!(other.is_empty());
}

#[tokio::test]
async fn an_item_a_note_drew_on_leaves_the_list_even_with_a_chunk_uncited() {
    let (db, _dir) = test_db().await;
    row(
        &db,
        "p1",
        "pf::notes.md::0",
        0,
        "Notes",
        BREAD_1,
        "folder",
        "test",
    )
    .await;
    row(
        &db,
        "p2",
        "pf::notes.md::1",
        1,
        "Notes",
        "## Links",
        "folder",
        "test",
    )
    .await;
    row(
        &db,
        "p3",
        "pf::other.md::0",
        0,
        "Other",
        BREAD_2,
        "folder",
        "test",
    )
    .await;
    let now = chrono::Utc::now().to_rfc3339();
    db.insert_page(
        "p_page",
        "Starter",
        None,
        "The sourdough starter peaks four to six hours after feeding [1].",
        None,
        None,
        &["pf::notes.md::0"],
        &now,
    )
    .await
    .unwrap();

    let left = new_material(&db, &ReadScope::Global).await.unwrap();
    assert_eq!(ids(&left), vec!["pf::other.md"]);
}

#[tokio::test]
async fn claim_then_write_creates_a_cited_note_and_the_items_leave_the_list() {
    let (db, _dir) = test_db().await;
    capture(&db, "w_a", BREAD_1).await;
    capture(&db, "w_b", BREAD_2).await;
    let items = vec!["w_a".to_string(), "w_b".to_string()];

    let claim = claim_new(&db, &ReadScope::Global, &items, None)
        .await
        .unwrap();
    assert_eq!(claim.sources.len(), 2);
    assert_eq!(claim.sources[0].n, 1);
    assert!(claim.skipped.is_empty());

    // A second run cannot take the same items while the first holds them.
    assert!(claim_new(&db, &ReadScope::Global, &items, None)
        .await
        .is_err());
    let listed = new_material(&db, &ReadScope::Global).await.unwrap();
    assert!(listed
        .iter()
        .filter(|m| items.contains(&m.item.item_id))
        .all(|m| m.item.held));

    let done = commit_new(
        &db,
        &claim.token,
        "Sourdough timing",
        BODY,
        "claude-code",
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "created");
    assert!(done.citations.unwrap().starts_with("2 verified"));
    let page = db.get_page(&done.page_id.unwrap()).await.unwrap().unwrap();
    assert_eq!(page.title, "Sourdough timing");
    assert_eq!(page.creation_kind, "distilled");
    assert_eq!(page.citations.len(), 2);

    let after = new_material(&db, &ReadScope::Global).await.unwrap();
    assert!(after.iter().all(|m| !items.contains(&m.item.item_id)));
    assert!(matches!(
        commit_new(&db, &claim.token, "Again", BODY, "claude-code", None).await,
        Err(SteepError::TokenGone)
    ));
}

#[tokio::test]
async fn a_citation_out_of_range_is_refused_and_the_hold_kept() {
    let (db, _dir) = test_db().await;
    capture(&db, "r_a", BREAD_1).await;
    capture(&db, "r_b", BREAD_2).await;
    let claim = claim_new(&db, &ReadScope::Global, &["r_a".into(), "r_b".into()], None)
        .await
        .unwrap();
    let bad = format!("{BODY} It is also great toast [7].");
    assert!(matches!(
        commit_new(&db, &claim.token, "Bread", &bad, "claude-code", None).await,
        Err(SteepError::CitationGate(_))
    ));
    let done = commit_new(&db, &claim.token, "Bread", BODY, "claude-code", None)
        .await
        .unwrap();
    assert_eq!(done.outcome, "created");
}

#[tokio::test]
async fn a_source_edited_after_the_claim_refuses_the_commit() {
    let (db, _dir) = test_db().await;
    capture(&db, "e_a", BREAD_1).await;
    capture(&db, "e_b", BREAD_2).await;
    let claim = claim_new(&db, &ReadScope::Global, &["e_a".into(), "e_b".into()], None)
        .await
        .unwrap();
    {
        let conn = db.test_primary_session().await;
        conn.execute(
            "UPDATE memories SET content = 'The starter peaks after two hours.' WHERE id = 'e_a'",
            (),
        )
        .await
        .unwrap();
    }
    assert!(matches!(
        commit_new(&db, &claim.token, "Bread", BODY, "claude-code", None).await,
        Err(SteepError::Changed(_))
    ));
    // The refused claim let go of its items: they can be claimed again.
    assert!(
        claim_new(&db, &ReadScope::Global, &["e_a".into(), "e_b".into()], None)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn release_gives_the_items_back() {
    let (db, _dir) = test_db().await;
    capture(&db, "x_a", BREAD_1).await;
    capture(&db, "x_b", BREAD_2).await;
    let items = vec!["x_a".to_string(), "x_b".to_string()];
    let claim = claim_new(&db, &ReadScope::Global, &items, None)
        .await
        .unwrap();
    assert_eq!(release_new(&claim.token).unwrap().outcome, "released");
    assert!(claim_new(&db, &ReadScope::Global, &items, None)
        .await
        .is_ok());
}

/// A compact list names items by short id; a claim takes it, and the same
/// item named twice (short and full) is claimed once.
#[tokio::test]
async fn a_claim_takes_the_short_ids_a_compact_list_shows() {
    let (db, _dir) = test_db().await;
    capture(&db, "short_a", BREAD_1).await;
    capture(&db, "short_b", BREAD_2).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &[
            wenlan_types::steep_short_id("short_a"),
            wenlan_types::steep_short_id("short_b"),
            "short_b".to_string(),
        ],
        None,
    )
    .await
    .unwrap();
    assert_eq!(claim.sources.len(), 2);
    assert!(claim.skipped.is_empty(), "{:?}", claim.skipped);
    release_new(&claim.token).unwrap();
}

#[tokio::test]
async fn one_short_item_is_too_little_for_a_note_but_one_long_one_is_enough() {
    let (db, _dir) = test_db().await;
    capture(&db, "t_short", BREAD_1).await;
    let Err(SteepError::Invalid(message)) =
        claim_new(&db, &ReadScope::Global, &["t_short".into()], None).await
    else {
        panic!("one short item must be refused as too little");
    };
    assert!(
        !message.contains("  "),
        "the refusal must read with single spaces: {message:?}"
    );
    capture(&db, "t_long", &BREAD_1.repeat(20)).await;
    assert!(claim_new(&db, &ReadScope::Global, &["t_long".into()], None)
        .await
        .is_ok());
}

#[tokio::test]
async fn a_space_locked_claim_cannot_take_material_from_another_space() {
    let (db, _dir) = test_db().await;
    capture(&db, "s_a", BREAD_1).await;
    assert!(claim_new(
        &db,
        &ReadScope::Space("other".into()),
        &["s_a".into()],
        None
    )
    .await
    .is_err());
}

/// One run groups only the newest page of the list: `limit` items, newest
/// first, each with a short snippet, and `total` counts everything left for
/// the next run.
#[tokio::test]
async fn a_limited_list_is_the_newest_items_with_short_snippets() {
    let (db, _dir) = test_db().await;
    let long = "The kiln cools for eight hours before anyone opens it. ".repeat(20);
    for i in 0..5_i64 {
        let id = format!("lim_{i}");
        row_at(
            &db,
            &id,
            &id,
            0,
            &id,
            &long,
            "claude-code",
            "test",
            1_700_000_000 + i,
        )
        .await;
    }
    let list = list_new(&db, &ReadScope::Global, 2, None).await.unwrap();
    assert_eq!(list.total, 5);
    let got: Vec<&str> = list.items.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(got, vec!["lim_4", "lim_3"], "newest first");
    assert_eq!(list.next_cursor.as_deref(), Some("2"));
    assert_eq!(SNIPPET_CHARS, 120);
    for item in &list.items {
        assert!(
            item.snippet.chars().count() <= SNIPPET_CHARS + 1,
            "{}",
            item.snippet
        );
    }
}

/// A claim finds the items a list handed out by short id without reading
/// every item again; a short id no list handed out is no such item.
#[tokio::test]
async fn a_claim_reads_only_its_listed_items() {
    let (db, _dir) = test_db().await;
    capture(&db, "only_a", BREAD_1).await;
    capture(&db, "only_b", BREAD_2).await;
    let unlisted = vec![wenlan_types::steep_short_id("only_a")];
    // Another test may have listed it already; the full id always reads.
    let full = material_of(&db, &ReadScope::Global, &["only_a".to_string()])
        .await
        .unwrap()
        .expect("a full item id is read directly");
    assert_eq!(ids(&full), vec!["only_a"]);
    new_material(&db, &ReadScope::Global).await.unwrap();
    let listed = material_of(&db, &ReadScope::Global, &unlisted)
        .await
        .unwrap()
        .expect("a listed short id is read directly");
    assert_eq!(ids(&listed), vec!["only_a"]);
    assert!(
        material_of(&db, &ReadScope::Global, &["0000000000".to_string()])
            .await
            .unwrap()
            .is_none(),
        "an id that names nothing sends the claim to the full list"
    );
}

/// Notes written from vault notes the claimed notes link to, or that link to
/// them, lead the link targets, before a note the text only names. The
/// claim's existing page is never one.
#[tokio::test]
async fn vault_links_lead_the_link_targets() {
    let (db, _dir) = test_db().await;
    let studio = "Mira fires the kiln every Sunday and keeps notes on [[Kiln|the kiln]]. \
                  Her starter recipe sits under Sourdough in the pantry book.";
    row(
        &db,
        "vl1",
        "vl::/v/Studio/Mira studio.md::0",
        0,
        "Mira studio",
        studio,
        "obsidian",
        "test",
    )
    .await;
    row(
        &db,
        "vl2",
        "vl::/v/Studio/Mira studio.md::1",
        1,
        "Mira studio",
        BREAD_1,
        "obsidian",
        "test",
    )
    .await;
    row(
        &db,
        "vl3",
        "vl::/v/Kiln.md::0",
        0,
        "Kiln",
        "Eight hours to cool.",
        "obsidian",
        "test",
    )
    .await;
    row(
        &db,
        "vl4",
        "vl::/v/Glaze.md::0",
        0,
        "Glaze",
        "Glazes for [[Studio/Mira Studio#Tiles]].",
        "obsidian",
        "test",
    )
    .await;
    row(
        &db,
        "vl5",
        "vl::/v/Bread.md::0",
        0,
        "Bread",
        BREAD_2,
        "obsidian",
        "test",
    )
    .await;
    let now = chrono::Utc::now().to_rfc3339();
    for (id, title, source) in [
        ("vl_sour", "Sourdough", "vl::/v/Bread.md::0"),
        ("vl_kiln", "Kiln care", "vl::/v/Kiln.md::0"),
        ("vl_glaze", "Glaze recipes", "vl::/v/Glaze.md::0"),
    ] {
        db.insert_page(
            id,
            title,
            None,
            "A note [1].",
            None,
            Some("test"),
            &[source],
            &now,
        )
        .await
        .unwrap();
    }
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["vl::/v/Studio/Mira studio.md".to_string()],
        None,
    )
    .await
    .unwrap();
    assert_eq!(claim.sources.len(), 2);
    assert_eq!(claim.link_targets.len(), 3, "{:?}", claim.link_targets);
    let mut linked = claim.link_targets[..2].to_vec();
    linked.sort();
    assert_eq!(linked, vec!["Glaze recipes", "Kiln care"]);
    assert_eq!(claim.link_targets[2], "Sourdough");

    let sources: Vec<(String, String)> = claim
        .sources
        .iter()
        .map(|s| (s.memory_id.clone(), s.text.clone()))
        .collect();
    let without = link_targets(&db, &ReadScope::Global, None, &sources, Some("vl_kiln")).await;
    assert!(!without.contains(&"Kiln care".to_string()), "{without:?}");
    release_new(&claim.token).unwrap();
}

#[test]
fn vault_note_names_come_from_the_file_and_the_link() {
    assert_eq!(
        vault_note_name("vl::/v/Studio/Mira studio.md::3"),
        Some("Mira studio")
    );
    assert_eq!(vault_note_name("vl::C:\\v\\Kiln.MD::0"), Some("Kiln"));
    assert_eq!(vault_note_name("import_chatgpt_x"), None);
    assert_eq!(vault_note_name("capture_1"), None);
    assert_eq!(
        linked_note_names(
            "See [[Kiln|the kiln]], [[Studio/Mira studio#Tiles]] and [[kiln]]. ![[pic.png]]"
        ),
        vec!["Kiln".to_string(), "Mira studio".to_string()]
    );
}
