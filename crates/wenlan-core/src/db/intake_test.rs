// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::conversation_intake::{FoundConversation, Grouping, Label};
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
    // Where it stands comes from its newest conversation.
    assert_eq!(things[1].status_line.as_deref(), Some("Hide button added"));
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
    let grouping = Grouping {
        joins: vec![(0, 1)],
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
    assert_eq!(things[0].status_line.as_deref(), Some("Two rows tried"));
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
