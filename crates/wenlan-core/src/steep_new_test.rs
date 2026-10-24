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

    let claim = claim_new(&db, &ReadScope::Global, &items).await.unwrap();
    assert_eq!(claim.sources.len(), 2);
    assert_eq!(claim.sources[0].n, 1);
    assert!(claim.skipped.is_empty());

    // A second run cannot take the same items while the first holds them.
    assert!(claim_new(&db, &ReadScope::Global, &items).await.is_err());
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
    let claim = claim_new(&db, &ReadScope::Global, &["r_a".into(), "r_b".into()])
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
    let claim = claim_new(&db, &ReadScope::Global, &["e_a".into(), "e_b".into()])
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
        claim_new(&db, &ReadScope::Global, &["e_a".into(), "e_b".into()])
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
    let claim = claim_new(&db, &ReadScope::Global, &items).await.unwrap();
    assert_eq!(release_new(&claim.token).unwrap().outcome, "released");
    assert!(claim_new(&db, &ReadScope::Global, &items).await.is_ok());
}

#[tokio::test]
async fn one_short_item_is_too_little_for_a_note_but_one_long_one_is_enough() {
    let (db, _dir) = test_db().await;
    capture(&db, "t_short", BREAD_1).await;
    let Err(SteepError::Invalid(message)) =
        claim_new(&db, &ReadScope::Global, &["t_short".into()]).await
    else {
        panic!("one short item must be refused as too little");
    };
    assert!(
        !message.contains("  "),
        "the refusal must read with single spaces: {message:?}"
    );
    capture(&db, "t_long", &BREAD_1.repeat(20)).await;
    assert!(claim_new(&db, &ReadScope::Global, &["t_long".into()])
        .await
        .is_ok());
}

#[tokio::test]
async fn a_space_locked_claim_cannot_take_material_from_another_space() {
    let (db, _dir) = test_db().await;
    capture(&db, "s_a", BREAD_1).await;
    assert!(
        claim_new(&db, &ReadScope::Space("other".into()), &["s_a".into()])
            .await
            .is_err()
    );
}
