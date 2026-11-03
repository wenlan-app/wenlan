// SPDX-License-Identifier: Apache-2.0

use super::sessions::session_row_id;
use super::tests::{test_db, test_db_at};
use super::things::{title_likeness, MoveTarget, SortRule};
use super::MemoryDB;
use crate::read_scope::ReadScope;
use wenlan_types::{
    BriefItemState, BriefMutation, BriefUpdateRequest, SessionRef, ThingActor, UpsertSessionRequest,
};

/// A conversation with metadata, in `space` (`None` for no Space).
async fn conversation(
    db: &MemoryDB,
    name: &str,
    title: Option<&str>,
    branch: Option<&str>,
    space: Option<&str>,
    last_write_at: i64,
) -> String {
    let session = db
        .upsert_session_metadata(&UpsertSessionRequest {
            session: name.into(),
            title: title.map(str::to_string),
            repo_root: branch.map(|_| "/repo/wenlan".to_string()),
            repo_branch: branch.map(str::to_string),
            ..Default::default()
        })
        .await
        .unwrap();
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE agent_sessions SET space = ?1, first_write_at = ?2, last_write_at = ?2
          WHERE id = ?3",
        libsql::params![space, last_write_at, session.id.as_str()],
    )
    .await
    .unwrap();
    session.id
}

async fn things_of(db: &MemoryDB, session_id: &str) -> Vec<String> {
    db.session_things(session_id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}

async fn history_ops(db: &MemoryDB, thing_id: &str) -> Vec<(String, String)> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT op, actor FROM thing_history WHERE thing_id = ?1 ORDER BY seq",
            libsql::params![thing_id],
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
    }
    out
}

#[test]
fn titles_alike_share_words_beyond_stopwords() {
    assert!(title_likeness("New tagline for the site", "Site tagline, new").unwrap() >= 0.5);
    assert!(title_likeness("Fix the bug", "Fix the build").is_none());
    assert!(
        title_likeness("Plan the Lisbon trip", "Lisbon trip: day trips").unwrap() >= 0.5,
        "plurals fold"
    );
    assert!(title_likeness("Class notes", "Glass notes").is_none());
    assert!(title_likeness("Wenlan", "wenlan").is_some());
    assert!(title_likeness("新的標語", "標語新的").unwrap() >= 0.5);
    assert!(title_likeness("", "anything").is_none());
}

#[tokio::test]
async fn ux_thing_1_every_conversation_lands_in_a_thing_without_asking() {
    let (db, _temp) = test_db().await;
    let a = conversation(
        &db,
        "codex:a",
        Some("Product Hunt launch copy"),
        None,
        Some("site"),
        10,
    )
    .await;
    let b = conversation(
        &db,
        "codex:b",
        Some("Launch copy for Product Hunt"),
        None,
        Some("site"),
        20,
    )
    .await;
    let c = conversation(
        &db,
        "codex:c",
        Some("Storage migration"),
        None,
        Some("site"),
        30,
    )
    .await;
    let d = conversation(&db, "codex:d", None, None, None, 40).await;

    let report = db.sort_sessions_into_things().await.unwrap();
    let rules: Vec<SortRule> = report.sorted.iter().map(|(_, r)| *r).collect();
    assert_eq!(
        rules,
        [SortRule::New, SortRule::Title, SortRule::New, SortRule::New]
    );
    for id in [&a, &b, &c, &d] {
        assert_eq!(
            things_of(&db, id).await.len(),
            1,
            "every conversation has a thing"
        );
    }
    assert_eq!(things_of(&db, &a).await, things_of(&db, &b).await);
    assert_ne!(things_of(&db, &a).await, things_of(&db, &c).await);

    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 3);
    assert_eq!(
        things[0].title, None,
        "an untitled conversation waits for a name"
    );
    assert_eq!(things[1].title.as_deref(), Some("Storage migration"));
    assert_eq!(
        things[2].last_activity_at, 20,
        "newest of its conversations"
    );
    assert!(things.iter().all(|t| t.title_by == ThingActor::Ai));

    let again = db.sort_sessions_into_things().await.unwrap();
    assert!(again.sorted.is_empty() && again.named == 0, "{again:?}");

    // The untitled one is named once its conversation has a title.
    db.upsert_session_metadata(&UpsertSessionRequest {
        session: "codex:d".into(),
        title: Some("Reader sidebar".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    let named = db.sort_sessions_into_things().await.unwrap();
    assert_eq!(named.named, 1);
    let thing = db.session_things(&d).await.unwrap().remove(0);
    assert_eq!(thing.title.as_deref(), Some("Reader sidebar"));
    let ops = history_ops(&db, &thing.id).await;
    assert_eq!(
        ops,
        [
            ("thing.create".into(), "ai".into()),
            ("thing.assign".into(), "ai".into()),
            ("thing.rename".into(), "ai".into())
        ]
    );
}

#[tokio::test]
async fn a_conversation_on_the_same_branch_joins_its_thing() {
    let (db, _temp) = test_db().await;
    let a = conversation(
        &db,
        "claude-code:a",
        Some("Storage engine"),
        Some("feat/things"),
        Some("wenlan"),
        10,
    )
    .await;
    let b = conversation(
        &db,
        "claude-code:b",
        Some("Unrelated words entirely"),
        Some("feat/things"),
        Some("wenlan"),
        20,
    )
    .await;
    let c = conversation(
        &db,
        "claude-code:c",
        Some("Other words again"),
        Some("main"),
        Some("wenlan"),
        30,
    )
    .await;
    let d = conversation(
        &db,
        "claude-code:d",
        Some("More different words"),
        Some("main"),
        Some("wenlan"),
        40,
    )
    .await;
    let report = db.sort_sessions_into_things().await.unwrap();
    let rules: Vec<SortRule> = report.sorted.iter().map(|(_, r)| *r).collect();
    assert_eq!(
        rules,
        [
            SortRule::New,
            SortRule::Branch,
            SortRule::New,
            SortRule::New
        ],
        "a default branch says nothing"
    );
    assert_eq!(things_of(&db, &a).await, things_of(&db, &b).await);
    assert_ne!(things_of(&db, &c).await, things_of(&db, &d).await);
}

#[tokio::test]
async fn brief_items_become_things_and_their_conversations_join_them() {
    let (db, _temp) = test_db().await;
    db.create_space("site", None, false).await.unwrap();
    let session = SessionRef::parse("codex:brief").unwrap();
    db.apply_brief_update_in_session(
        &BriefUpdateRequest {
            space: "site".into(),
            caller_id: "things-test".into(),
            operation_id: "add-tagline".into(),
            summary: None,
            mutations: vec![BriefMutation::Add {
                text: "Ship the new tagline".into(),
                state: BriefItemState::Active,
                added_at: None,
                gate: None,
            }],
        },
        Some(&session),
    )
    .await
    .unwrap();
    let report = db.sort_sessions_into_things().await.unwrap();
    assert_eq!(report.brief_things, 1);
    assert_eq!(report.sorted, [(session_row_id(&session), SortRule::Brief)]);
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 1);
    assert_eq!(things[0].title.as_deref(), Some("Ship the new tagline"));
    assert_eq!(
        things[0].status_line.as_deref(),
        Some("Ship the new tagline")
    );
    assert_eq!(things[0].spaces, ["site"]);

    let scoped = db
        .list_things(&ReadScope::Space("other".into()), None)
        .await
        .unwrap();
    assert!(scoped.is_empty(), "another Space sees nothing");
}

#[tokio::test]
async fn the_person_renames_a_thing_and_moves_a_conversation() {
    let (db, _temp) = test_db().await;
    let a = conversation(&db, "codex:a", Some("Tagline copy"), None, Some("site"), 10).await;
    let b = conversation(&db, "codex:b", Some("Pricing page"), None, Some("site"), 20).await;
    db.sort_sessions_into_things().await.unwrap();
    let from = things_of(&db, &a).await.remove(0);
    let other = things_of(&db, &b).await.remove(0);

    db.rename_thing(&from, "  The   new tagline ")
        .await
        .unwrap();
    let renamed = db
        .get_thing(&from, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed.title.as_deref(), Some("The new tagline"));
    assert_eq!(renamed.title_by, ThingActor::Person);
    assert!(db.rename_thing(&from, "   ").await.is_err());
    assert!(matches!(
        db.rename_thing("thg_missing", "x").await,
        Err(crate::WenlanError::NotFound(_))
    ));

    // Move a into b's thing; nothing re-sorts it afterwards.
    let moved = db
        .move_session_thing(&a, &from, &MoveTarget::Thing(other.clone()))
        .await
        .unwrap();
    assert_eq!(moved, other);
    assert_eq!(things_of(&db, &a).await, std::slice::from_ref(&other));
    assert!(db
        .sort_sessions_into_things()
        .await
        .unwrap()
        .sorted
        .is_empty());
    assert!(
        db.get_thing(&from, &ReadScope::Global)
            .await
            .unwrap()
            .is_none(),
        "an emptied thing leaves the list"
    );
    assert!(matches!(
        db.move_session_thing(&a, &from, &MoveTarget::Thing(other.clone()))
            .await,
        Err(crate::WenlanError::Conflict(_))
    ));

    // Move it out into a new thing the person names.
    let fresh = db
        .move_session_thing(&a, &other, &MoveTarget::NewThing("Launch week".into()))
        .await
        .unwrap();
    let fresh_thing = db
        .get_thing(&fresh, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh_thing.title.as_deref(), Some("Launch week"));
    assert_eq!(fresh_thing.title_by, ThingActor::Person);
    assert_eq!(
        things_of(&db, &b).await,
        std::slice::from_ref(&other),
        "b stays"
    );
    let ops = history_ops(&db, &other).await;
    assert!(
        ops.contains(&("thing.unassign".into(), "person".into())),
        "{ops:?}"
    );

    // Moving to the same thing changes nothing.
    assert_eq!(
        db.move_session_thing(&a, &fresh, &MoveTarget::Thing(fresh.clone()))
            .await
            .unwrap(),
        fresh
    );
    assert_eq!(things_of(&db, &a).await, [fresh]);
}

#[tokio::test]
async fn detail_lists_its_conversations_and_their_pages() {
    let (db, _temp) = test_db().await;
    let a = conversation(&db, "codex:a", Some("Tagline copy"), None, None, 10).await;
    db.record_session_read(
        &SessionRef::parse("codex:a").unwrap(),
        wenlan_types::SessionReadKind::Recall,
        "tagline",
        None,
    )
    .await
    .unwrap();
    db.sort_sessions_into_things().await.unwrap();
    let thing = things_of(&db, &a).await.remove(0);
    let detail = db
        .get_thing_detail(&thing, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.thing.id, thing);
    assert_eq!(detail.sessions.len(), 1);
    assert_eq!(detail.sessions[0].id, a);
    assert_eq!(detail.sessions[0].title.as_deref(), Some("Tagline copy"));
    assert!(db
        .get_thing_detail("thg_missing", &ReadScope::Global)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn ux_thing_1_a_conversation_titled_only_in_its_transcript_shows_that_title() {
    let (db, temp) = test_db().await;
    let transcript = temp.path().join("conversation.jsonl");
    std::fs::write(
        &transcript,
        "{\"type\":\"ai-title\",\"aiTitle\":\"Kyoto stay near Gion\",\"sessionId\":\"k\"}\n",
    )
    .unwrap();
    let session = db
        .upsert_session_metadata(&UpsertSessionRequest {
            session: "claude-code:kyoto".into(),
            transcript_path: Some(transcript.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
    db.sort_sessions_into_things().await.unwrap();
    let thing = things_of(&db, &session.id).await.remove(0);
    let detail = db
        .get_thing_detail(&thing, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        detail.sessions[0].title.as_deref(),
        Some("Kyoto stay near Gion")
    );
}

#[tokio::test]
async fn migration_149_adds_the_thing_tables() {
    let (db, _temp) = test_db_at(148).await;
    db.migrate_149_things(148).await.unwrap();
    db.migrate_149_things(149).await.unwrap();
    let conn = db.conn.lock().await;
    for table in ["things", "thing_sessions", "thing_history"] {
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                libsql::params![table],
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_some(), "{table}");
    }
}

#[tokio::test]
async fn ux_home_5_a_thing_quiet_for_a_week_leaves_working_on_and_comes_back() {
    let (db, _temp) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let day = 24 * 60 * 60;
    let fresh = conversation(
        &db,
        "codex:fresh",
        Some("Fresh work"),
        None,
        None,
        now - day,
    )
    .await;
    let old = conversation(
        &db,
        "codex:old",
        Some("Old chat import"),
        None,
        None,
        now - 40 * day,
    )
    .await;
    let week_old = conversation(
        &db,
        "codex:week",
        Some("Last week's release"),
        None,
        None,
        now - 8 * day,
    )
    .await;
    db.sort_sessions_into_things().await.unwrap();
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 3, "a quiet thing is still listed, for search");
    let quiet = |id: &str| {
        let thing = things_of_list(&things, id);
        thing.quiet
    };
    let fresh_thing = things_of(&db, &fresh).await.remove(0);
    let old_thing = things_of(&db, &old).await.remove(0);
    let week_thing = things_of(&db, &week_old).await.remove(0);
    assert!(!quiet(&fresh_thing));
    assert!(quiet(&old_thing));
    assert!(quiet(&week_thing), "a week without anything new is quiet");
    assert_eq!(things[0].id, fresh_thing, "newest activity first");

    // A new conversation in it brings it back.
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE agent_sessions SET last_write_at = ?1 WHERE id = ?2",
        libsql::params![now, old.as_str()],
    )
    .await
    .unwrap();
    drop(conn);
    let back = db
        .get_thing(&old_thing, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert!(!back.quiet);
}

#[tokio::test]
async fn ux_home_5_a_hidden_thing_leaves_working_on_until_its_next_conversation() {
    let (db, _temp) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let session = conversation(&db, "codex:done", Some("New Mac"), None, None, now - 60).await;
    db.sort_sessions_into_things().await.unwrap();
    let thing = things_of(&db, &session).await.remove(0);
    async fn quiet(db: &MemoryDB, id: &str) -> bool {
        db.get_thing(id, &ReadScope::Global)
            .await
            .unwrap()
            .unwrap()
            .quiet
    }
    assert!(!quiet(&db, &thing).await);

    db.hide_thing(&thing).await.unwrap();
    assert!(quiet(&db, &thing).await, "hidden from home");
    let listed = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert!(
        listed.iter().any(|t| t.id == thing),
        "still listed, for search"
    );

    // The next conversation in it brings it back.
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE agent_sessions SET last_write_at = ?1 WHERE id = ?2",
        libsql::params![now + 60, session.as_str()],
    )
    .await
    .unwrap();
    drop(conn);
    assert!(!quiet(&db, &thing).await);

    assert!(matches!(
        db.hide_thing("thg_missing").await,
        Err(crate::error::WenlanError::NotFound(_))
    ));
}

#[tokio::test]
async fn ux_home_5_a_thing_from_an_old_brief_item_is_as_old_as_the_item() {
    let (db, _temp) = test_db().await;
    db.create_space("site", None, false).await.unwrap();
    let now = chrono::Utc::now().timestamp();
    db.apply_brief_update(&BriefUpdateRequest {
        space: "site".into(),
        caller_id: "things-test".into(),
        operation_id: "old-item".into(),
        summary: None,
        mutations: vec![BriefMutation::Add {
            text: "Rewrite the pricing page".into(),
            state: BriefItemState::Active,
            added_at: Some(now - 30 * 24 * 60 * 60),
            gate: None,
        }],
    })
    .await
    .unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE brief_items SET updated_at = ?1",
            libsql::params![now - 30 * 24 * 60 * 60],
        )
        .await
        .unwrap();
    }
    db.sort_sessions_into_things().await.unwrap();
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 1);
    assert!(
        things[0].quiet,
        "made today from a month-old item, still quiet"
    );
    assert!(things[0].last_activity_at <= now - 29 * 24 * 60 * 60);
}

fn things_of_list<'a>(
    things: &'a [wenlan_types::ThingSummary],
    id: &str,
) -> &'a wenlan_types::ThingSummary {
    things.iter().find(|t| t.id == id).unwrap()
}
