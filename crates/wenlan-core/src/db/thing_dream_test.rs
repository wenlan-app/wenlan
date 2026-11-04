// SPDX-License-Identifier: Apache-2.0

use crate::conversation_intake::{FoundConversation, Grouping, Label};
use crate::db::tests::test_db;
use crate::db::MemoryDB;
use crate::read_scope::ReadScope;
use crate::sources::RawDocument;
use std::collections::HashMap;
use std::path::PathBuf;

fn found(id: &str, last_at: i64) -> FoundConversation {
    FoundConversation {
        tool: "claude-code".into(),
        external_id: id.into(),
        path: PathBuf::from(format!("/home/me/.claude/projects/-w/{id}.jsonl")),
        len: 10,
        cwd: Some("/w/wenlan".into()),
        branch: None,
        said: vec!["a".into(), "b".into()],
        last_reply: String::new(),
        first_at: last_at - 60,
        last_at,
        automated: false,
        on_behalf: false,
    }
}

fn work(title: &str) -> Label {
    Label {
        work: true,
        title: title.into(),
        space: "wenlan".into(),
        about: format!("about {title}"),
        status: "Going".into(),
    }
}

async fn remember(db: &MemoryDB, id: &str, content: &str, kind: &str, supersedes: Option<&str>) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: id.to_string(),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        metadata: HashMap::new(),
        memory_type: Some(kind.to_string()),
        supersedes: supersedes.map(str::to_string),
        ..Default::default()
    }])
    .await
    .unwrap();
}

/// Two things from three conversations: "Site redesign" (c0, c1) and
/// "Release notes" (c2).
async fn two_things(db: &MemoryDB, now: i64) -> (String, String) {
    for (id, title, ago) in [
        ("c0", "Site redesign", 7_200),
        ("c1", "Site redesign", 3_600),
        ("c2", "Release notes", 300),
    ] {
        db.record_conversation_label(&found(id, now - ago), &work(title), None)
            .await
            .unwrap();
    }
    let pending = db.intake_pending(50).await.unwrap();
    let members = |t: &str| -> Vec<usize> {
        pending
            .iter()
            .enumerate()
            .filter(|(_, p)| p.title == t)
            .map(|(i, _)| i)
            .collect()
    };
    db.apply_intake_grouping(
        &pending,
        &[],
        &Grouping {
            new: vec![
                ("Site redesign".into(), members("Site redesign")),
                ("Release notes".into(), members("Release notes")),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let open = db.intake_open_things().await.unwrap();
    let id = |t: &str| open.iter().find(|o| o.title == t).unwrap().id.clone();
    (id("Site redesign"), id("Release notes"))
}

/// UX-CHAT-5: each night the conversations that went on are read once for
/// what they settled; the points become memories on the thing's page, a
/// point that no longer holds is replaced, and the person can take a night
/// back. A night's reading is not activity: the thing still goes quiet.
#[tokio::test]
async fn ux_chat_5_each_night_what_a_thing_settled_goes_into_long_term_memory() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, notes) = two_things(&db, now).await;

    // A conversation still going on waits; the two that went quiet are due,
    // newest thing first, each thing's conversations newest first.
    let due = db.things_to_dream(now - 1_800, 10).await.unwrap();
    assert_eq!(due.len(), 1, "c2 ended five minutes ago and waits");
    assert_eq!(due[0].thing_id, site);
    let read: Vec<&str> = due[0]
        .conversations
        .iter()
        .map(|c| c.external_id.as_str())
        .collect();
    assert_eq!(read, ["c1", "c0"]);
    assert_eq!(db.things_to_dream(now, 10).await.unwrap().len(), 2);
    assert_eq!(db.things_to_dream(now, 1).await.unwrap().len(), 1);

    // The night writes one settled and one open point.
    let site_before = db
        .list_things(&ReadScope::Global, Some(&site))
        .await
        .unwrap()[0]
        .last_activity_at;
    remember(
        &db,
        "mem_tagline",
        "The site tagline is \"You do the thinking\".",
        "decision",
        None,
    )
    .await;
    remember(
        &db,
        "mem_pricing",
        "Whether the site gets a pricing page is open.",
        "fact",
        None,
    )
    .await;
    for id in ["mem_tagline", "mem_pricing"] {
        db.attach_dream_point(id, &due[0].conversations[0].session_id)
            .await
            .unwrap();
    }
    db.record_dream(
        &site,
        &["mem_tagline".into()],
        &["mem_pricing".into()],
        &Default::default(),
        &due[0].conversations,
    )
    .await
    .unwrap();
    let detail = db
        .get_thing_detail(&site, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    let dream = detail.dream.expect("the page says what was put away");
    assert_eq!((dream.decided, dream.open), (1, 1));
    assert_eq!(dream.fresh, ["mem_tagline", "mem_pricing"]);
    assert_eq!(
        detail.open_points,
        ["mem_pricing"],
        "the page can group what is open"
    );
    let listed: Vec<&str> = detail
        .memories
        .iter()
        .map(|m| m.source_id.as_str())
        .collect();
    assert!(listed.contains(&"mem_tagline") && listed.contains(&"mem_pricing"));
    let site_after = db
        .list_things(&ReadScope::Global, Some(&site))
        .await
        .unwrap()[0]
        .last_activity_at;
    assert_eq!(site_after, site_before, "reading at night is not activity");
    let listed = db
        .list_things(&ReadScope::Global, Some(&site))
        .await
        .unwrap();
    assert_eq!(
        listed[0].dreamed_at,
        Some(dream.at),
        "home can say it was steeped"
    );

    // Read conversations are not read again until they go on.
    let due = db.things_to_dream(now, 10).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].thing_id, notes);
    db.record_conversation_label(&found("c1", now - 60), &work("Site redesign"), None)
        .await
        .unwrap();
    let due = db.things_to_dream(now, 10).await.unwrap();
    let site_due = due.iter().find(|d| d.thing_id == site).expect("c1 went on");
    assert_eq!(site_due.conversations.len(), 1);

    // The next night sees what is kept; a point that was decided after all
    // replaces the open one and drops out of what is kept.
    let prior = db.dream_points(&site).await.unwrap();
    assert_eq!(prior.len(), 2);
    assert!(prior.iter().any(|p| p.id == "mem_tagline" && p.decided));
    assert!(prior.iter().any(|p| p.id == "mem_pricing" && !p.decided));
    remember(
        &db,
        "mem_pricing_2",
        "The site gets no pricing page for now.",
        "decision",
        Some("mem_pricing"),
    )
    .await;
    db.record_dream(
        &site,
        &["mem_pricing_2".into()],
        &[],
        &Default::default(),
        &site_due.conversations,
    )
    .await
    .unwrap();
    let kept: Vec<String> = db
        .dream_points(&site)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(kept, ["mem_tagline", "mem_pricing_2"]);

    // Taking the newest night back names only its memories, once.
    assert_eq!(db.undo_thing_dream(&site).await.unwrap(), ["mem_pricing_2"]);
    let dream = db.thing_dream(&site).await.unwrap().unwrap();
    assert_eq!(
        (dream.decided, dream.open),
        (1, 1),
        "back to the first night"
    );
    assert_eq!(db.undo_thing_dream(&site).await.unwrap().len(), 2);
    assert!(db.thing_dream(&site).await.unwrap().is_none());
    let listed = db
        .list_things(&ReadScope::Global, Some(&site))
        .await
        .unwrap();
    assert_eq!(listed[0].dreamed_at, None, "taken back, home says nothing");
    assert!(db.undo_thing_dream(&site).await.unwrap().is_empty());
    assert!(db.dream_points(&site).await.unwrap().is_empty());
    // Taken back is not read again for the same lines.
    assert!(db
        .things_to_dream(now, 10)
        .await
        .unwrap()
        .iter()
        .all(|d| d.thing_id != site));
}
