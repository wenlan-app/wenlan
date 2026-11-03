// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::conversation_intake::{Correction, FoundConversation, Grouping, Label, Piece};
use crate::db::tests::test_db;
use crate::read_scope::ReadScope;
use std::path::PathBuf;
use wenlan_types::UpsertSessionRequest;

fn found(id: &str, last_at: i64) -> FoundConversation {
    FoundConversation {
        tool: "claude-code".into(),
        external_id: id.into(),
        path: PathBuf::from(format!("/home/me/.claude/projects/-w/{id}.jsonl")),
        len: 10,
        cwd: Some("/w/wenlan".into()),
        branch: Some("home".into()),
        said: vec!["a".into(), "b".into()],
        last_reply: String::new(),
        first_at: last_at - 60,
        last_at,
        automated: false,
        on_behalf: false,
    }
}

fn work(title: &str, status: &str) -> Label {
    Label {
        work: true,
        title: title.into(),
        space: "wenlan".into(),
        about: format!("about {title}"),
        status: status.into(),
    }
}

#[tokio::test]
async fn ux_thing_1_a_read_conversation_is_recorded_once_and_read_again_only_when_it_went_on() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let conv = found("c1", now - 3_600);
    let id = found_session_id(&conv).unwrap();
    assert!(needs_label(
        &conv,
        db.conversation_reads(std::slice::from_ref(&id))
            .await
            .unwrap()
            .get(&id)
    ));
    db.record_conversation_label(
        &conv,
        &work("Home redesign", "Cards done"),
        Some("/w/wenlan"),
    )
    .await
    .unwrap();
    let reads = db
        .conversation_reads(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert!(!needs_label(&conv, reads.get(&id)));
    let mut later = conv.clone();
    later.last_at += RELABEL_AFTER_SECS + 1;
    assert!(needs_label(&later, reads.get(&id)));

    // Work is a conversation like one that wrote to Wenlan.
    let session = db.get_session(&id).await.unwrap().unwrap();
    assert_eq!(session.space.as_deref(), Some("wenlan"));
    assert_eq!(session.cwd.as_deref(), Some("/w/wenlan"));
    assert_eq!(session.repo_branch.as_deref(), Some("home"));
    assert_eq!(session.last_write_at, now - 3_600);

    // Not work: remembered, but no conversation row.
    let chat = found("c2", now - 100);
    let chat_id = found_session_id(&chat).unwrap();
    db.record_conversation_label(
        &chat,
        &Label {
            work: false,
            title: String::new(),
            space: String::new(),
            about: String::new(),
            status: String::new(),
        },
        None,
    )
    .await
    .unwrap();
    assert!(db.get_session(&chat_id).await.unwrap().is_none());
    assert!(db.conversation_reads(&[chat_id]).await.unwrap().len() == 1);
}

#[tokio::test]
async fn ux_thing_1_grouping_puts_read_conversations_into_things_with_where_they_stand() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    for (id, at, title, status) in [
        ("a", now - 7_200, "Home redesign", "Cards drafted"),
        ("b", now - 3_600, "Home cards", "Hide button added"),
        ("c", now - 1_800, "Lisbon trip", "Flights booked"),
        ("e", now - 900, "Merge check", "Merged"),
    ] {
        db.record_conversation_label(&found(id, at), &work(title, status), None)
            .await
            .unwrap();
    }
    let pending = db.intake_pending(50).await.unwrap();
    assert_eq!(
        pending.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["Home redesign", "Home cards", "Lisbon trip", "Merge check"]
    );
    // The daemon's own rules leave read conversations alone.
    assert!(db
        .sort_sessions_into_things()
        .await
        .unwrap()
        .sorted
        .is_empty());

    let grouping = Grouping {
        joins: vec![],
        new: vec![
            ("Home redesign".into(), vec![0, 1]),
            ("Lisbon trip".into(), vec![2]),
        ],
        left_out: vec![3],
        lines: vec![(Piece::New(0), "Cards drafted, hide button in".into())],
        same: vec![],
    };
    assert_eq!(
        db.apply_intake_grouping(&pending, &[], &grouping)
            .await
            .unwrap(),
        2
    );
    assert!(db.intake_pending(50).await.unwrap().is_empty());

    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 2);
    assert_eq!(things[0].title.as_deref(), Some("Lisbon trip"));
    assert_eq!(things[0].status_line.as_deref(), Some("Flights booked"));
    assert_eq!(things[1].title.as_deref(), Some("Home redesign"));
    // Where it stands: the grouping model's line for the whole thing, else
    // its newest conversation's.
    assert_eq!(
        things[1].status_line.as_deref(),
        Some("Cards drafted, hide button in")
    );
    assert_eq!(things[1].spaces, vec!["wenlan".to_string()]);

    // A new conversation can join a thing already there.
    db.record_conversation_label(
        &found("d", now - 60),
        &work("Home spacing", "Two rows tried"),
        None,
    )
    .await
    .unwrap();
    let pending = db.intake_pending(50).await.unwrap();
    let open = db.intake_open_things().await.unwrap();
    assert_eq!(
        open.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["Lisbon trip", "Home redesign"]
    );
    assert_eq!(open[1].about, "about Home cards");
    assert!(open.iter().all(|t| t.news.is_empty()));
    let grouping = Grouping {
        joins: vec![(0, 1)],
        lines: vec![(Piece::Open(1), "Cards done; two rows tried".into())],
        ..Default::default()
    };
    assert_eq!(
        db.apply_intake_grouping(&pending, &open, &grouping)
            .await
            .unwrap(),
        0
    );
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things[0].title.as_deref(), Some("Home redesign"));
    assert_eq!(
        things[0].status_line.as_deref(),
        Some("Cards done; two rows tried")
    );

    // A conversation read again after the line was written makes it out of
    // date: its own status shows, and the thing has news for the model.
    let mut later = found("d", now);
    later.last_at = now + 1;
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    db.record_conversation_label(&later, &work("Home spacing", "Grid dropped"), None)
        .await
        .unwrap();
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things[0].status_line.as_deref(), Some("Grid dropped"));
    let open = db.intake_open_things().await.unwrap();
    assert_eq!(open[0].title, "Home redesign");
    assert_eq!(open[0].news, vec!["Grid dropped".to_string()]);
    // Answered with no line, it is marked written and not offered again.
    db.apply_intake_grouping(&[], &open, &Grouping::default())
        .await
        .unwrap();
    assert!(db
        .intake_open_things()
        .await
        .unwrap()
        .iter()
        .all(|t| t.news.is_empty()));
}

#[tokio::test]
async fn ux_thing_1_a_conversation_the_rules_placed_moves_and_the_persons_placement_stays() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    // Two conversations that wrote to Wenlan, sorted by the rules first.
    for (name, title) in [
        ("claude-code:r1", "Rule made tagline"),
        ("claude-code:r2", "Storage export check"),
    ] {
        db.upsert_session_metadata(&UpsertSessionRequest {
            session: name.into(),
            title: Some(title.into()),
            ..Default::default()
        })
        .await
        .unwrap();
    }
    db.sort_sessions_into_things().await.unwrap();
    let r2 = found_session_id(&found("r2", now)).unwrap();
    let rule_things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(rule_things.len(), 2);
    let r2_thing = db.session_things(&r2).await.unwrap()[0].id.clone();
    db.move_session_thing(
        &r2,
        &r2_thing,
        &crate::db::things::MoveTarget::NewThing("Mine".into()),
    )
    .await
    .unwrap();

    for id in ["r1", "r2"] {
        db.record_conversation_label(&found(id, now - 60), &work("Release", "Tagged"), None)
            .await
            .unwrap();
    }
    let pending = db.intake_pending(50).await.unwrap();
    assert_eq!(
        pending.len(),
        1,
        "the person's own placement is not offered again"
    );
    let grouping = Grouping {
        joins: vec![],
        new: vec![("Release".into(), vec![0])],
        ..Default::default()
    };
    db.apply_intake_grouping(&pending, &[], &grouping)
        .await
        .unwrap();
    let titles: Vec<_> = db
        .list_things(&ReadScope::Global, None)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.title)
        .collect();
    assert!(titles.contains(&"Release".to_string()));
    assert!(titles.contains(&"Mine".to_string()));
    assert!(
        !titles.iter().any(|t| t == "Rule made tagline"),
        "the rule-made thing is left empty: {titles:?}"
    );
}

#[tokio::test]
async fn ux_thing_1_a_space_long_out_of_use_is_not_offered() {
    let (db, _dir) = test_db().await;
    // No space written to lately: every space is offered.
    db.create_space("wenlan", None, false).await.unwrap();
    db.create_space("old-app", None, false).await.unwrap();
    let all = db.intake_spaces().await.unwrap();
    assert!(all.contains(&"wenlan".to_string()) && all.contains(&"old-app".to_string()));

    let now = chrono::Utc::now().timestamp();
    {
        let conn = db.conn.lock().await;
        for (id, space, at) in [
            ("m1", "wenlan", now - 3_600),
            ("m2", "old-app", now - SPACE_IN_USE_SECS - 86_400),
        ] {
            conn.execute(
                "INSERT INTO memories (id, content, source, source_id, title, chunk_index,
                                       last_modified, chunk_type, space)
                 VALUES (?1, 'x', 'memory', ?1, 'x', 0, ?3, 'text', ?2)",
                libsql::params![id, space, at],
            )
            .await
            .unwrap();
        }
    }
    assert_eq!(
        db.intake_spaces().await.unwrap(),
        vec!["wenlan".to_string()]
    );
}

#[tokio::test]
async fn ux_thing_1_a_left_out_chore_that_grows_into_work_is_offered_again() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let chore = found("chore", now - 7_200);
    db.record_conversation_label(&chore, &work("Merge check", "Merged"), None)
        .await
        .unwrap();
    let pending = db.intake_pending(50).await.unwrap();
    db.apply_intake_grouping(
        &pending,
        &[],
        &Grouping {
            left_out: vec![0],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(db.intake_pending(50).await.unwrap().is_empty());
    // It went on and was read again: the grouping model looks at it again.
    let mut later = chore.clone();
    later.last_at = now;
    db.record_conversation_label(&later, &work("Release 0.21", "Tagged"), None)
        .await
        .unwrap();
    let pending = db.intake_pending(50).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].title, "Release 0.21");
}

#[tokio::test]
async fn ux_thing_1_grouping_never_overrides_the_person_or_places_a_conversation_twice() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    for (id, title) in [("p1", "Home cards"), ("p2", "Lisbon trip")] {
        db.record_conversation_label(&found(id, now - 600), &work(title, "Going"), None)
            .await
            .unwrap();
    }
    let pending = db.intake_pending(50).await.unwrap();
    assert_eq!(pending.len(), 2);
    // While the model thinks, the person puts p1 in a thing of their own.
    db.apply_intake_grouping(
        &pending[..1],
        &[],
        &Grouping {
            new: vec![("Draft".into(), vec![0])],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let p1 = pending[0].session_id.clone();
    let draft = db.session_things(&p1).await.unwrap()[0].id.clone();
    db.move_session_thing(
        &p1,
        &draft,
        &crate::db::things::MoveTarget::NewThing("Mine".into()),
    )
    .await
    .unwrap();
    db.apply_intake_grouping(
        &pending,
        &[],
        &Grouping {
            new: vec![("Home".into(), vec![0]), ("Trip".into(), vec![1])],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let p1_things: Vec<_> = db
        .session_things(&p1)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.title)
        .collect();
    assert_eq!(p1_things, vec!["Mine".to_string()]);

    // Labels rebuilt from the transcripts: nothing is offered twice.
    {
        let conn = db.conn.lock().await;
        conn.execute("DELETE FROM conversation_labels", ())
            .await
            .unwrap();
    }
    for (id, title) in [("p1", "Home cards"), ("p2", "Lisbon trip")] {
        db.record_conversation_label(&found(id, now - 600), &work(title, "Going"), None)
            .await
            .unwrap();
    }
    assert!(db.intake_pending(50).await.unwrap().is_empty());
}

#[tokio::test]
async fn ux_thing_1_a_read_conversation_follows_its_space_when_renamed() {
    let (db, _dir) = test_db().await;
    db.create_space("wenlan", None, false).await.unwrap();
    let conv = found("r", chrono::Utc::now().timestamp() - 60);
    db.record_conversation_label(&conv, &work("Home", "Cards"), None)
        .await
        .unwrap();
    db.update_space("wenlan", "wenlan-app", None).await.unwrap();
    let pending = db.intake_pending(50).await.unwrap();
    assert_eq!(pending[0].space, "wenlan-app");
}

#[tokio::test]
async fn ux_thing_2_the_persons_moves_are_what_the_grouping_follows() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    for (id, title) in [("z1", "Zotero sync"), ("z2", "Launch post")] {
        db.record_conversation_label(&found(id, now - 600), &work(title, "Going"), None)
            .await
            .unwrap();
    }
    let pending = db.intake_pending(50).await.unwrap();
    // The model put both in one thing; the Zotero one was wrong.
    db.apply_intake_grouping(
        &pending,
        &[],
        &Grouping {
            new: vec![("Wenlan growth".into(), vec![0, 1])],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(db.intake_corrections().await.unwrap().is_empty());
    let z1 = pending
        .iter()
        .find(|p| p.title == "Zotero sync")
        .unwrap()
        .session_id
        .clone();
    let growth = db.session_things(&z1).await.unwrap()[0].id.clone();
    let zotero = db
        .move_session_thing(
            &z1,
            &growth,
            &crate::db::things::MoveTarget::NewThing("Zotero workflow".into()),
        )
        .await
        .unwrap();
    let corrections = db.intake_corrections().await.unwrap();
    assert_eq!(
        corrections,
        vec![Correction {
            title: "Zotero sync".into(),
            about: "about Zotero sync".into(),
            from: "Wenlan growth".into(),
            to: "Zotero workflow".into(),
        }]
    );

    // The next grouping sees the move as the way to sort a case like it.
    let prompt = crate::conversation_intake::group_prompt(&[], &[], &corrections, "English");
    assert!(prompt.contains(
        "- \"Zotero sync\" (about Zotero sync): not \"Wenlan growth\" but \"Zotero workflow\""
    ));
    assert!(prompt.contains("the way the person did"));
    // With no moves, the prompt says nothing about them.
    assert!(
        !crate::conversation_intake::group_prompt(&[], &[], &[], "English").contains("corrected")
    );

    // Moved again: only where it is now counts.
    db.move_session_thing(
        &z1,
        &zotero,
        &crate::db::things::MoveTarget::NewThing("Research tools".into()),
    )
    .await
    .unwrap();
    let corrections = db.intake_corrections().await.unwrap();
    assert_eq!(corrections.len(), 1);
    assert_eq!(
        (corrections[0].from.as_str(), corrections[0].to.as_str()),
        ("Zotero workflow", "Research tools")
    );
}
