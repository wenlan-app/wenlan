// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;

async fn page(db: &MemoryDB, id: &str, title: &str, content: &str) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        content,
        None,
        Some("sp"),
        &[],
        "2026-01-01T00:00:00Z",
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
}

async fn edit(db: &MemoryDB, id: &str, content: &str, edited_by: &str) {
    db.update_page_content(id, content, &[], edited_by)
        .await
        .unwrap();
}

async fn exec(db: &MemoryDB, sql: &str) {
    let session = db.test_primary_session().await;
    session.execute(sql, ()).await.unwrap();
}

/// The page's changelog as the write paths record it: one entry per
/// version, without the text.
async fn changelog(db: &MemoryDB, id: &str) -> Vec<PageChangelogEntry> {
    let mut rows = db.list_page_history(id, 100).await.unwrap();
    rows.reverse();
    rows.into_iter()
        .map(|row| {
            serde_json::from_value(serde_json::json!({
                "version": row.version,
                "at": row.created_at,
                "edited_by": row.edited_by,
            }))
            .unwrap()
        })
        .collect()
}

#[test]
fn the_person_and_ai_are_told_apart_by_the_writer() {
    for by in ["manual_edit", "fs_edit", "restore"] {
        assert_eq!(kind_of_edit(by), PageHistoryKind::PersonEdit, "{by}");
    }
    for by in [
        "agent_refresh",
        "re_distill",
        "distill",
        "citation_backfill",
        "revision_accept",
    ] {
        assert_eq!(kind_of_edit(by), PageHistoryKind::AiEdit, "{by}");
    }
}

#[tokio::test]
async fn one_list_holds_both_edits_and_a_source_update_with_texts() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.").await;
    page(&db, "b", "Beta", "Plans built on Alpha.").await;
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();
    // Everything so far happened before the basis was added.
    exec(&db, "UPDATE page_history SET created_at = created_at - 100").await;
    exec(&db, "UPDATE page_basis SET added_at = added_at - 50").await;

    edit(&db, "b", "Plans built on Alpha, refined.", "manual_edit").await;
    edit(
        &db,
        "b",
        "Plans built on Alpha, refined by AI.",
        "agent_refresh",
    )
    .await;
    edit(&db, "a", "The launch moved to 2026-04-01.", "agent_refresh").await;

    let mut entries = changelog(&db, "b").await;
    attach_texts(&db, "b", &mut entries).await.unwrap();
    assert!(entries
        .iter()
        .any(|e| e.text.as_deref() == Some("Plans built on Alpha, refined.")));

    let visible: HashSet<String> = ["a".to_string()].into();
    let history = build_page_history(&db, "b", &entries, &visible, true)
        .await
        .unwrap();
    let kinds: Vec<PageHistoryKind> = history.iter().map(|h| h.kind).collect();
    assert!(kinds.contains(&PageHistoryKind::PersonEdit), "{history:#?}");
    assert!(kinds.contains(&PageHistoryKind::AiEdit), "{history:#?}");
    let update = history
        .iter()
        .find(|h| h.kind == PageHistoryKind::SourceUpdate)
        .expect("the source's new version is in the list");
    let source = update.source.as_ref().unwrap();
    assert_eq!(source.page_id, "a");
    assert_eq!(source.title, "Alpha");
    assert_eq!(source.version, 2);
    assert_eq!(source.relied_on_version, 1);
    assert!(source.pending);
    assert_eq!(
        source.text.as_deref(),
        Some("The launch moved to 2026-04-01.")
    );
    // Only the version written after the basis was added is an event.
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == PageHistoryKind::SourceUpdate)
            .count(),
        1
    );
    // Newest first.
    assert!(history.windows(2).all(|w| w[0].at >= w[1].at));

    // A source the caller may not see is left out, and no text without asking.
    let hidden = build_page_history(&db, "b", &entries, &HashSet::new(), false)
        .await
        .unwrap();
    assert!(hidden
        .iter()
        .all(|h| h.kind != PageHistoryKind::SourceUpdate));
    let plain = build_page_history(&db, "b", &changelog(&db, "b").await, &visible, false)
        .await
        .unwrap();
    assert!(plain
        .iter()
        .all(|h| h.text.is_none() && h.source.as_ref().is_none_or(|s| s.text.is_none())));
}

/// An authored page created by `actor`.
async fn made_by(db: &MemoryDB, id: &str, content: &str, actor: &str) {
    db.insert_page_with_basis(
        id,
        id,
        None,
        content,
        None,
        Some("sp"),
        &[],
        "2026-01-01T00:00:00Z",
        "authored",
        "confirmed",
        None,
        None,
        &[],
        actor,
        None,
    )
    .await
    .unwrap();
}

/// The changelog as the write paths leave it: an entry per edit, none for
/// the version a page was created at.
async fn real_changelog(db: &MemoryDB, id: &str) -> Vec<PageChangelogEntry> {
    assert_eq!(
        db.get_page_changelog(id).await.unwrap(),
        "[]",
        "created empty"
    );
    let mut entries = changelog(db, id).await;
    entries.retain(|entry| entry.version != 1);
    entries
}

#[tokio::test]
async fn the_first_version_is_listed_as_the_pages_birth_with_its_text() {
    let (db, _dir) = test_db().await;
    made_by(&db, "ai", "Day one is Belém.", "claude-code").await;
    let created = real_changelog(&db, "ai").await;
    assert!(created.is_empty());
    edit(
        &db,
        "ai",
        "Day one is Belém. Day two is Sintra.",
        "manual_edit",
    )
    .await;

    let mut entries = changelog(&db, "ai").await;
    entries.retain(|entry| entry.version != 1);
    attach_texts(&db, "ai", &mut entries).await.unwrap();
    let history = build_page_history(&db, "ai", &entries, &HashSet::new(), true)
        .await
        .unwrap();
    let versions: Vec<_> = history.iter().map(|item| item.version).collect();
    assert_eq!(versions, [Some(2), Some(1)], "{history:#?}");
    let birth = &history[1];
    assert_eq!(birth.kind, PageHistoryKind::AiEdit);
    assert_eq!(birth.edited_by.as_deref(), Some("agent_create"));
    assert_eq!(birth.text.as_deref(), Some("Day one is Belém."));
    assert_eq!(birth.changed_sentences, Some(1));
    assert_eq!(history[0].kind, PageHistoryKind::PersonEdit);
    assert_eq!(history[0].changed_sentences, Some(1));

    // A page the person made in the app is theirs from birth.
    // ("unknown" is the actor the app's own create passes.)
    made_by(&db, "mine", "My own note.", "unknown").await;
    let history = build_page_history(
        &db,
        "mine",
        &real_changelog(&db, "mine").await,
        &HashSet::new(),
        false,
    )
    .await
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].kind, PageHistoryKind::PersonEdit);
    assert_eq!(history[0].edited_by.as_deref(), Some("person_create"));
    assert_eq!(history[0].text, None, "no text without asking");
}
