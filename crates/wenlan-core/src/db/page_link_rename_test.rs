// SPDX-License-Identifier: Apache-2.0
//! Renaming a page rewrites the `[[links]]` other pages make to it.

use super::tests::test_db;
use crate::{
    export::knowledge::KnowledgeProjectionWrite,
    post_write::{rename_page, restore_page},
    read_scope::ReadScope,
};
use std::{path::Path, sync::Arc};

struct Fixture {
    db: Arc<super::MemoryDB>,
    _db_dir: tempfile::TempDir,
    page_root: tempfile::TempDir,
}

/// `(id, title, space, body, user_edited, review_required, creation_kind)`.
type PageRow<'a> = (&'a str, &'a str, &'a str, &'a str, i64, i64, &'a str);

/// Pages are inserted as rows, then every link graph is built by the real
/// resolver, so the edges the rename reads are the ones a save would mint.
async fn fixture(pages: &[PageRow<'_>]) -> Fixture {
    let (db, db_dir) = test_db().await;
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "INSERT INTO spaces (id,name,created_at,updated_at) VALUES
                 ('space-work','work',1,1),('space-home','home',1,1);",
        )
        .await
        .unwrap();
        for (id, title, space, body, user_edited, review_required, creation_kind) in pages {
            conn.execute(
                "INSERT INTO pages
                     (id,title,summary,content,space,source_memory_ids,version,status,
                      created_at,last_compiled,last_modified,creation_kind,review_status,
                      user_edited,review_required)
                 VALUES (?1,?2,NULL,?3,?4,'[]',3,'active','2026-01-01T00:00:00Z',
                         '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',?5,'confirmed',?6,?7)",
                libsql::params![
                    *id,
                    *title,
                    *body,
                    *space,
                    *creation_kind,
                    *user_edited,
                    *review_required
                ],
            )
            .await
            .unwrap();
        }
    }
    for (id, _, _, body, ..) in pages {
        db.refresh_page_wikilinks(id, body).await.unwrap();
    }
    let db = Arc::new(db);
    let page_root = tempfile::tempdir().unwrap();
    for (id, ..) in pages {
        let page = db.get_page(id).await.unwrap().unwrap();
        KnowledgeProjectionWrite::new(page_root.path().to_path_buf(), &db)
            .write_page(&page)
            .unwrap();
    }
    Fixture {
        db,
        _db_dir: db_dir,
        page_root,
    }
}

fn projected_markdown(root: &Path, id: &str) -> String {
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".wenlan/state.json")).unwrap()).unwrap();
    let file = state["pages"][id]["file"].as_str().unwrap();
    std::fs::read_to_string(root.join(file)).unwrap()
}

const LINKING_BODY: &str = "See [[Old]] and [[Old|alias text]] and [[ old #Part]].\n\
Embed ![[Old]] and qualified [[work/Old]].\n\
Inline `[[Old]]` and ``a ` [[Old]]`` stay. Escaped \\[[Old]] stays.\n\
\n\
```\n\
[[Old]]\n\
```\n";

const REWRITTEN_BODY: &str = "See [[New]] and [[New|alias text]] and [[ New #Part]].\n\
Embed ![[New]] and qualified [[work/New]].\n\
Inline `[[Old]]` and ``a ` [[Old]]`` stay. Escaped \\[[Old]] stays.\n\
\n\
```\n\
[[Old]]\n\
```\n";

#[tokio::test]
async fn rename_rewrites_every_link_form_in_same_space_pages() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        ("page-b", "Linker", "work", LINKING_BODY, 0, 0, "distilled"),
        (
            "page-c",
            "Elsewhere",
            "home",
            "From home: [[Old]].",
            0,
            0,
            "distilled",
        ),
    ])
    .await;
    let db = &fixture.db;
    let before = db.get_page("page-b").await.unwrap().unwrap();
    let inbound_before = db
        .get_page_inbound_links_scoped("page-a", &ReadScope::Global)
        .await
        .unwrap();
    assert!(inbound_before.iter().any(|(id, _)| id == "page-b"));
    assert!(inbound_before.iter().any(|(id, _)| id == "page-c"));

    let response = rename_page(
        db,
        "page-a",
        "New",
        3,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    )
    .await
    .unwrap();
    assert_eq!(response.title, "New");
    assert_eq!(response.version, 4);
    assert_eq!(response.links_updated, 1);
    assert_eq!(response.links_pending_review, 0);
    // The page in the other Space still links by the old title, and says so.
    assert_eq!(response.links_not_updated, 1);

    let after = db.get_page("page-b").await.unwrap().unwrap();
    assert_eq!(after.content, REWRITTEN_BODY);
    assert_eq!(after.version, before.version + 1);
    assert!(!after.user_edited, "a link rewrite is not a person's edit");
    let changelog: Vec<wenlan_types::responses::PageChangelogEntry> =
        serde_json::from_str(after.changelog.as_deref().unwrap()).unwrap();
    assert_eq!(changelog.last().unwrap().edited_by, "link_rename");
    assert_eq!(changelog.last().unwrap().version, after.version);
    let history = db.list_page_history("page-b", 5).await.unwrap();
    assert_eq!(history[0].version, after.version);
    assert_eq!(history[0].content, REWRITTEN_BODY);
    assert!(projected_markdown(fixture.page_root.path(), "page-b").contains("[[New|alias text]]"));

    // The other Space is untouched.
    let elsewhere = db.get_page("page-c").await.unwrap().unwrap();
    assert_eq!(elsewhere.content, "From home: [[Old]].");
    assert_eq!(elsewhere.version, 3);

    // Backlinks still find the linking page, now by the new title.
    let inbound = db
        .get_page_inbound_links_scoped("page-a", &ReadScope::Global)
        .await
        .unwrap();
    assert!(inbound
        .iter()
        .any(|(id, label)| id == "page-b" && label == "New"));
    let outbound = db
        .get_page_outbound_links_scoped("page-b", &ReadScope::Global)
        .await
        .unwrap();
    assert!(outbound
        .iter()
        .any(|link| link.label == "New" && link.target_page_id.as_deref() == Some("page-a")));
}

#[tokio::test]
async fn replaying_the_rename_changes_nothing_more() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        ("page-b", "Linker", "work", LINKING_BODY, 0, 0, "distilled"),
    ])
    .await;
    let db = &fixture.db;
    let first = rename_page(db, "page-a", "New", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(first.links_updated, 1);
    let written = db.get_page("page-b").await.unwrap().unwrap();

    let replay = rename_page(db, "page-a", "New", first.version, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(replay.version, first.version);
    assert_eq!(
        (
            replay.links_updated,
            replay.links_pending_review,
            replay.links_not_updated
        ),
        (0, 0, 0)
    );
    let again = db.get_page("page-b").await.unwrap().unwrap();
    assert_eq!(again.version, written.version);
    assert_eq!(again.content, REWRITTEN_BODY);
}

#[tokio::test]
async fn a_rename_whose_link_step_never_ran_is_finished_by_a_repeated_request() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-b",
            "Linker",
            "work",
            "See [[Old]].",
            0,
            0,
            "distilled",
        ),
    ])
    .await;
    let db = &fixture.db;
    // Title only, as if the process stopped right after the rename committed.
    let renamed = db
        .rename_page("page-a", "New", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(
        db.get_page("page-b").await.unwrap().unwrap().content,
        "See [[Old]]."
    );

    let repeated = rename_page(
        db,
        "page-a",
        "New",
        renamed.version,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();
    assert_eq!(repeated.version, renamed.version);
    assert_eq!(repeated.links_updated, 1);
    assert_eq!(
        db.get_page("page-b").await.unwrap().unwrap().content,
        "See [[New]]."
    );
}

#[tokio::test]
async fn an_ambiguous_title_rewrites_nothing() {
    let fixture = fixture(&[
        ("page-a", "Dup", "work", "One of two.", 0, 0, "distilled"),
        (
            "page-twin",
            "Dup",
            "work",
            "The other one.",
            0,
            0,
            "distilled",
        ),
        (
            "page-b",
            "Linker",
            "work",
            "Which [[Dup]]?",
            0,
            0,
            "distilled",
        ),
    ])
    .await;
    let db = &fixture.db;
    let response = rename_page(db, "page-a", "Dup one", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(
        (
            response.links_updated,
            response.links_pending_review,
            response.links_not_updated
        ),
        (0, 0, 0)
    );
    let linker = db.get_page("page-b").await.unwrap().unwrap();
    assert_eq!(linker.content, "Which [[Dup]]?");
    assert_eq!(linker.version, 3);
}

#[tokio::test]
async fn a_page_the_person_wrote_is_rewritten_and_a_locked_page_gets_a_card() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-mine",
            "Mine",
            "work",
            "I wrote [[Old]].",
            1,
            0,
            "authored",
        ),
        (
            "page-locked",
            "Locked",
            "work",
            "Locked [[Old]].",
            1,
            1,
            "authored",
        ),
    ])
    .await;
    let db = &fixture.db;
    let response = rename_page(
        db,
        "page-a",
        "New",
        3,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    )
    .await
    .unwrap();
    assert_eq!(response.links_updated, 1);
    assert_eq!(response.links_pending_review, 1);
    assert_eq!(response.links_not_updated, 0);

    let mine = db.get_page("page-mine").await.unwrap().unwrap();
    assert_eq!(mine.content, "I wrote [[New]].");
    assert_eq!(mine.version, 4);
    assert!(mine.user_edited);
    assert_eq!(mine.creation_kind, "authored");

    // The locked page keeps the person's text; the update waits for review.
    let locked = db.get_page("page-locked").await.unwrap().unwrap();
    assert_eq!(locked.content, "Locked [[Old]].");
    assert_eq!(locked.version, 3);
    assert!(projected_markdown(fixture.page_root.path(), "page-locked").contains("[[Old]]"));
    let cards = db.list_pending_revisions(50).await.unwrap();
    let card = cards
        .iter()
        .find(|card| card.target_source_id == "page-locked")
        .expect("a revision card for the locked page");
    assert_eq!(card.revision_content, "Locked [[New]].");
}

#[tokio::test]
async fn a_caller_held_to_one_space_changes_no_page_outside_it() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-b",
            "Linker",
            "work",
            "See [[Old]].",
            0,
            0,
            "distilled",
        ),
    ])
    .await;
    let db = &fixture.db;
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "UPDATE pages SET workspace='work' WHERE id='page-a';
             UPDATE pages SET workspace='home' WHERE id='page-b';",
        )
        .await
        .unwrap();
    }
    let response = rename_page(
        db,
        "page-a",
        "New",
        3,
        &ReadScope::Space("work".to_string()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            response.links_updated,
            response.links_pending_review,
            response.links_not_updated
        ),
        (0, 0, 0)
    );
    assert_eq!(
        db.get_page("page-b").await.unwrap().unwrap().content,
        "See [[Old]]."
    );
}

#[tokio::test]
async fn restoring_under_a_new_title_updates_links_as_a_rename_does() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The archived page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-b",
            "Linker",
            "work",
            "See [[Old]].",
            0,
            0,
            "distilled",
        ),
        (
            "page-locked",
            "Locked",
            "work",
            "Locked [[Old]].",
            1,
            1,
            "authored",
        ),
        (
            "page-c",
            "Elsewhere",
            "home",
            "From home: [[Old]].",
            0,
            0,
            "distilled",
        ),
    ])
    .await;
    let db = &fixture.db;
    db.archive_page("page-a").await.unwrap();
    let archived = db.get_page("page-a").await.unwrap().unwrap();

    let response = restore_page(
        db,
        "page-a",
        Some("New"),
        archived.version,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    )
    .await
    .unwrap();
    assert_eq!(response.title, "New");
    assert_eq!(response.status, "active");
    assert_eq!(
        (
            response.links_updated,
            response.links_pending_review,
            response.links_not_updated
        ),
        (1, 1, 1)
    );

    let linker = db.get_page("page-b").await.unwrap().unwrap();
    assert_eq!(linker.content, "See [[New]].");
    assert_eq!(linker.version, 4);
    let changelog: Vec<wenlan_types::responses::PageChangelogEntry> =
        serde_json::from_str(linker.changelog.as_deref().unwrap()).unwrap();
    assert_eq!(changelog.last().unwrap().edited_by, "link_rename");
    assert!(projected_markdown(fixture.page_root.path(), "page-b").contains("[[New]]"));

    // A locked page keeps its text; the update waits as a card.
    let locked = db.get_page("page-locked").await.unwrap().unwrap();
    assert_eq!(locked.content, "Locked [[Old]].");
    let cards = db.list_pending_revisions(50).await.unwrap();
    let card = cards
        .iter()
        .find(|card| card.target_source_id == "page-locked")
        .expect("a revision card for the locked page");
    assert_eq!(card.revision_content, "Locked [[New]].");

    // Another Space keeps its link.
    let elsewhere = db.get_page("page-c").await.unwrap().unwrap();
    assert_eq!(elsewhere.content, "From home: [[Old]].");
    assert_eq!(elsewhere.version, 3);
}

#[tokio::test]
async fn restoring_under_the_same_title_leaves_links_alone() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The archived page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-b",
            "Linker",
            "work",
            "See [[Old]].",
            0,
            0,
            "distilled",
        ),
    ])
    .await;
    let db = &fixture.db;
    db.archive_page("page-a").await.unwrap();
    let archived = db.get_page("page-a").await.unwrap().unwrap();
    for title in [Some("Old"), None] {
        let version = db.get_page("page-a").await.unwrap().unwrap().version;
        let response = restore_page(db, "page-a", title, version, &ReadScope::Global, None)
            .await
            .unwrap();
        assert_eq!(response.title, "Old");
        assert_eq!(
            (
                response.links_updated,
                response.links_pending_review,
                response.links_not_updated
            ),
            (0, 0, 0)
        );
    }
    // The second call was a replay of the first restore.
    assert_eq!(
        db.get_page("page-a").await.unwrap().unwrap().version,
        archived.version + 1
    );
    let linker = db.get_page("page-b").await.unwrap().unwrap();
    assert_eq!(linker.content, "See [[Old]].");
    assert_eq!(linker.version, 3);
}

/// The link cards waiting on `page_id`, as their proposed bodies.
async fn link_cards(db: &super::MemoryDB, page_id: &str) -> Vec<String> {
    let mut bodies: Vec<String> = db
        .list_pending_revisions(50)
        .await
        .unwrap()
        .into_iter()
        .filter(|card| card.target_source_id == page_id)
        .map(|card| card.revision_content)
        .collect();
    bodies.sort();
    bodies
}

#[tokio::test]
async fn a_second_rename_replaces_the_link_card_the_first_one_left() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-other",
            "Other",
            "work",
            "Another page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-locked",
            "Locked",
            "work",
            "Locked [[Old]] and [[Other]].",
            1,
            1,
            "authored",
        ),
        (
            "page-later",
            "Later",
            "work",
            "Written later.",
            1,
            1,
            "authored",
        ),
    ])
    .await;
    let db = &fixture.db;
    // A card for a different renamed page on the same locked page stays.
    let other = rename_page(db, "page-other", "Other two", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(other.links_pending_review, 1);

    let first = rename_page(db, "page-a", "Mid", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(first.links_pending_review, 1);
    assert_eq!(
        link_cards(db, "page-locked").await,
        vec![
            "Locked [[Mid]] and [[Other]].".to_string(),
            "Locked [[Old]] and [[Other two]].".to_string(),
        ]
    );
    // A locked page that came to link by the new title before the next rename.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET content='Later [[Mid]].' WHERE id='page-later'",
            (),
        )
        .await
        .unwrap();
    }
    db.refresh_page_wikilinks("page-later", "Later [[Mid]].")
        .await
        .unwrap();

    let second = rename_page(db, "page-a", "New", first.version, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(second.links_pending_review, 2);
    assert_eq!(
        link_cards(db, "page-locked").await,
        vec![
            "Locked [[New]] and [[Other]].".to_string(),
            "Locked [[Old]] and [[Other two]].".to_string(),
        ]
    );
    assert_eq!(
        link_cards(db, "page-later").await,
        vec!["Later [[New]].".to_string()]
    );
    // Nothing was written to the locked pages.
    assert_eq!(
        db.get_page("page-locked").await.unwrap().unwrap().content,
        "Locked [[Old]] and [[Other]]."
    );

    // Repeating the rename stages no second copy of the card.
    let replay = rename_page(
        db,
        "page-a",
        "New",
        second.version,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();
    assert_eq!(replay.links_pending_review, 2);
    assert_eq!(link_cards(db, "page-locked").await.len(), 2);
    assert_eq!(link_cards(db, "page-later").await.len(), 1);
}

#[tokio::test]
async fn renaming_back_to_the_old_title_removes_the_link_card() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-locked",
            "Locked",
            "work",
            "Locked [[Old]].",
            1,
            1,
            "authored",
        ),
    ])
    .await;
    let db = &fixture.db;
    let first = rename_page(db, "page-a", "Mid", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(first.links_pending_review, 1);
    assert_eq!(
        link_cards(db, "page-locked").await,
        vec!["Locked [[Mid]].".to_string()]
    );

    let back = rename_page(db, "page-a", "old", first.version, &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(
        (
            back.links_updated,
            back.links_pending_review,
            back.links_not_updated
        ),
        (0, 0, 0)
    );
    assert!(link_cards(db, "page-locked").await.is_empty());
    let locked = db.get_page("page-locked").await.unwrap().unwrap();
    assert_eq!(locked.content, "Locked [[Old]].");
    assert_eq!(locked.version, 3);
}

/// The ids of the link cards waiting on `page_id`, with their bodies.
async fn link_card_ids(db: &super::MemoryDB, page_id: &str) -> Vec<(String, String)> {
    db.list_pending_revisions(50)
        .await
        .unwrap()
        .into_iter()
        .filter(|card| card.target_source_id == page_id)
        .map(|card| (card.revision_source_id, card.revision_content))
        .collect()
}

fn locked_pair() -> [PageRow<'static>; 2] {
    [
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-locked",
            "Locked",
            "work",
            "Locked [[Old]].",
            1,
            1,
            "authored",
        ),
    ]
}

#[tokio::test]
async fn a_link_card_the_person_edited_survives_the_next_rename() {
    let fixture = fixture(&locked_pair()).await;
    let db = &fixture.db;
    let first = rename_page(db, "page-a", "Mid", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    let cards = link_card_ids(db, "page-locked").await;
    assert_eq!(cards.len(), 1);
    // The person reworded the proposal before deciding on it.
    db.update_memory(&cards[0].0, "Locked [[Mid]], reworded by me.")
        .await
        .unwrap();

    rename_page(db, "page-a", "New", first.version, &ReadScope::Global, None)
        .await
        .unwrap();
    let mut bodies: Vec<String> = link_card_ids(db, "page-locked")
        .await
        .into_iter()
        .map(|(_, body)| body)
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec![
            "Locked [[Mid]], reworded by me.".to_string(),
            "Locked [[New]].".to_string(),
        ]
    );
}

#[tokio::test]
async fn reordered_links_never_cost_another_page_its_link_card() {
    let fixture = fixture(&[
        (
            "page-a",
            "Old",
            "work",
            "The renamed page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-other",
            "Other",
            "work",
            "Another page.",
            0,
            0,
            "distilled",
        ),
        (
            "page-locked",
            "Locked",
            "work",
            "[[Other]] [[Old]].",
            1,
            1,
            "authored",
        ),
    ])
    .await;
    let db = &fixture.db;
    rename_page(db, "page-other", "Other two", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    // The person swaps the two links on the locked page.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET content='[[Old]] [[Other]].',version=version+1 WHERE id='page-locked'",
            (),
        )
        .await
        .unwrap();
    }
    db.refresh_page_wikilinks("page-locked", "[[Old]] [[Other]].")
        .await
        .unwrap();

    rename_page(db, "page-a", "New", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    let mut bodies: Vec<String> = link_card_ids(db, "page-locked")
        .await
        .into_iter()
        .map(|(_, body)| body)
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec![
            "[[New]] [[Other]].".to_string(),
            "[[Other two]] [[Old]].".to_string(),
        ]
    );
}

#[tokio::test]
async fn an_untagged_link_card_from_an_earlier_release_is_still_replaced() {
    let fixture = fixture(&locked_pair()).await;
    let db = &fixture.db;
    let first = rename_page(db, "page-a", "Mid", 3, &ReadScope::Global, None)
        .await
        .unwrap();
    // A card as 0.20.5 staged it: no renamed-page tag.
    {
        let conn = db.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE memories SET structured_fields=json_remove(structured_fields,
                         '$.link_rename_page','$.link_rename_digest')
                  WHERE supersedes='page-locked' AND pending_revision=1
                    AND json_extract(structured_fields,'$.link_rename_page')='page-a'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(changed, 1, "the new card carries the tag");
    }
    rename_page(db, "page-a", "New", first.version, &ReadScope::Global, None)
        .await
        .unwrap();
    let cards = link_card_ids(db, "page-locked").await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].1, "Locked [[New]].");
}
