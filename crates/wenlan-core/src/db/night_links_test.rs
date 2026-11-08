// SPDX-License-Identifier: Apache-2.0

use super::night_links::{NightChange, NightLink, NightTopic};
use crate::conversation_intake::{FoundConversation, Grouping, Label, PriorPoint};
use crate::db::tests::test_db;
use crate::db::MemoryDB;
use crate::read_scope::ReadScope;
use crate::sources::RawDocument;
use std::collections::HashMap;
use std::path::PathBuf;

fn found(id: &str, last_at: i64, said: &[&str]) -> FoundConversation {
    FoundConversation {
        tool: "claude-code".into(),
        external_id: id.into(),
        path: PathBuf::from(format!("/home/me/.claude/projects/-w/{id}.jsonl")),
        len: 10,
        cwd: Some("/w/wenlan".into()),
        branch: None,
        said: said.iter().map(|s| s.to_string()).collect(),
        last_reply: String::new(),
        first_at: last_at - 60,
        last_at,
        automated: false,
        on_behalf: false,
        touched: Vec::new(),
    }
}

async fn remember(db: &MemoryDB, id: &str, space: &str) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: id.to_string(),
        content: format!("the point {id}"),
        last_modified: chrono::Utc::now().timestamp(),
        metadata: HashMap::new(),
        memory_type: Some("decision".to_string()),
        space: Some(space.to_string()),
        ..Default::default()
    }])
    .await
    .unwrap();
}

/// One thing, "Site redesign", with one conversation.
async fn one_thing(db: &MemoryDB) -> String {
    let now = chrono::Utc::now().timestamp();
    let label = Label {
        work: true,
        title: "Site redesign".into(),
        space: "wenlan".into(),
        about: "the site".into(),
        status: "Going".into(),
    };
    db.record_conversation_label(&found("c0", now - 7_200, &["a", "b"]), &label, None)
        .await
        .unwrap();
    let pending = db.intake_pending(50).await.unwrap();
    db.apply_intake_grouping(
        &pending,
        &[],
        &Grouping {
            new: vec![("Site redesign".into(), vec![0])],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.intake_open_things().await.unwrap()[0].id.clone()
}

async fn topic(db: &MemoryDB, name: &str, space: Option<&str>) -> String {
    db.store_entity(name, "concept", space, Some("test"), Some(1.0))
        .await
        .unwrap()
}

/// Links a topic to enough memories that the hub rule calls it a hub.
async fn make_hub(db: &MemoryDB, entity: &str) {
    let conn = db.conn.lock().await;
    for i in 0..super::HUB_MIN_LINKS {
        conn.execute(
            "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
            libsql::params![format!("hub_mem_{i}"), entity],
        )
        .await
        .unwrap();
    }
}

async fn links_of(db: &MemoryDB, memory: &str) -> Vec<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT entity_id FROM memory_entities WHERE memory_id = ?1 ORDER BY rowid",
            libsql::params![memory],
        )
        .await
        .unwrap();
    let mut links = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        links.push(row.get::<String>(0).unwrap());
    }
    links
}

async fn main_topic(db: &MemoryDB, memory: &str) -> Option<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT entity_id FROM memories WHERE source_id = ?1 LIMIT 1",
            libsql::params![memory],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    row.get::<Option<String>>(0).unwrap()
}

/// UX-CHAT-5: a thing's model is offered the topics its earlier points connect
/// to and specific names the person typed. Hubs, other Spaces and names of
/// code are not on the list; everyday words are not there for being typed.
#[tokio::test]
async fn ux_chat_5_a_night_is_offered_specific_topics_and_never_a_hub() {
    let (db, _dir) = test_db().await;
    let earlier = topic(&db, "Landing Page", Some("wenlan")).await;
    let github = topic(&db, "GitHub", None).await;
    topic(&db, "Memory", Some("wenlan")).await;
    topic(&db, "dream_with", Some("wenlan")).await;
    topic(&db, "Payroll Sheet", Some("other")).await;
    let hub = topic(&db, "Wenlan", Some("wenlan")).await;
    make_hub(&db, &hub).await;
    remember(&db, "mem_old", "wenlan").await;
    db.link_night_point("mem_old", &earlier).await.unwrap();

    let conversations = [found(
        "c0",
        100,
        &[
            "Wenlan pushes to GitHub, and the memory, memory and Memory all stay",
            "see dream_with and the Payroll Sheet",
        ],
    )];
    let prior = [PriorPoint {
        id: "mem_old".into(),
        text: "the point".into(),
        decided: true,
    }];
    let topics = db
        .night_topics("Site redesign", &conversations, &prior, &["wenlan".into()])
        .await
        .unwrap();
    let names: Vec<&str> = topics.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names[0], "Landing Page",
        "earlier points' topics come first"
    );
    assert!(topics.iter().any(|t| t.entity_id == github), "{names:?}");
    assert!(!names.contains(&"Wenlan"), "a hub: {names:?}");
    assert!(!names.contains(&"dream_with"), "code: {names:?}");
    assert!(
        !names.contains(&"Payroll Sheet"),
        "another Space: {names:?}"
    );
    assert!(topics.len() <= super::night_links::MAX_NIGHT_TOPICS);
}

/// A link never crosses Spaces; an unfiled topic can take a point from anywhere.
#[test]
fn ux_chat_5_a_link_stays_in_its_space() {
    let filed = NightTopic {
        entity_id: "e".into(),
        name: "Pricing".into(),
        space: Some("wenlan".into()),
    };
    let unfiled = NightTopic {
        space: None,
        ..filed.clone()
    };
    assert!(filed.fits(Some("wenlan")));
    assert!(!filed.fits(Some("other")));
    assert!(!filed.fits(None));
    assert!(unfiled.fits(Some("other")) && unfiled.fits(None));
}

/// UX-CHAT-5: a link goes through the hub rule, and only a link this made is
/// reported, so undo never takes back what other work linked.
#[tokio::test]
async fn ux_chat_5_a_point_is_linked_only_when_that_is_new_and_not_a_hub() {
    let (db, _dir) = test_db().await;
    remember(&db, "mem_a", "wenlan").await;
    let pricing = topic(&db, "Pricing", Some("wenlan")).await;
    let hub = topic(&db, "Wenlan", Some("wenlan")).await;
    make_hub(&db, &hub).await;

    assert_eq!(db.link_night_point("mem_a", &hub).await.unwrap(), None);
    assert!(links_of(&db, "mem_a").await.is_empty());

    assert_eq!(
        db.link_night_point("mem_a", &pricing).await.unwrap(),
        Some(NightLink {
            memory: "mem_a".into(),
            entity: pricing.clone(),
        })
    );
    assert_eq!(links_of(&db, "mem_a").await, [pricing.as_str()]);
    assert_eq!(main_topic(&db, "mem_a").await, Some(pricing.clone()));
    // Already there: not this one's to take back.
    assert_eq!(db.link_night_point("mem_a", &pricing).await.unwrap(), None);
    // No such memory: nothing to link.
    assert_eq!(
        db.link_night_point("mem_gone", &pricing).await.unwrap(),
        None
    );
}

/// UX-CHAT-5: taking a night back removes the links it made, and only those. A
/// point's main topic falls back to what it still has.
#[tokio::test]
async fn ux_chat_5_undoing_a_night_removes_its_links() {
    let (db, _dir) = test_db().await;
    let site = one_thing(&db).await;
    remember(&db, "mem_a", "wenlan").await;
    remember(&db, "mem_b", "wenlan").await;
    let pricing = topic(&db, "Pricing", Some("wenlan")).await;
    let other_work = topic(&db, "Hero Image", Some("wenlan")).await;

    let link = db
        .link_night_point("mem_a", &pricing)
        .await
        .unwrap()
        .unwrap();
    // Linked by other work, after the night.
    db.link_memory_entities("mem_a", &[other_work.as_str()])
        .await
        .unwrap();
    db.record_dream(
        &site,
        &["mem_a".into()],
        &["mem_b".into()],
        &Default::default(),
        std::slice::from_ref(&link),
        &[],
    )
    .await
    .unwrap();

    let forget = db.undo_thing_dream(&site).await.unwrap();
    assert_eq!(forget, ["mem_a", "mem_b"]);
    assert_eq!(links_of(&db, "mem_a").await, [other_work.as_str()]);
    assert_eq!(main_topic(&db, "mem_a").await, Some(other_work));
}

/// One record: a night writes what it made into `changes`; every kind can be
/// held, and an entry from before there were changes still reads as a night.
#[tokio::test]
async fn ux_chat_5_one_nightly_record_holds_every_kind_of_change() {
    let (db, _dir) = test_db().await;
    let site = one_thing(&db).await;
    remember(&db, "mem_a", "wenlan").await;
    remember(&db, "mem_b", "wenlan").await;
    let link = NightLink {
        memory: "mem_a".into(),
        entity: "ent".into(),
    };
    db.record_dream(
        &site,
        &["mem_a".into()],
        &["mem_b".into()],
        &Default::default(),
        std::slice::from_ref(&link),
        &[],
    )
    .await
    .unwrap();
    let data = {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT data FROM thing_history WHERE op = 'thing.dream' AND thing_id = ?1",
                libsql::params![site.as_str()],
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap()
    };
    let entry: serde_json::Value = serde_json::from_str(&data).unwrap();
    let written: Vec<NightChange> =
        serde_json::from_value(entry.get("changes").unwrap().clone()).unwrap();
    assert_eq!(
        written,
        [
            NightChange::New {
                memory: "mem_a".into()
            },
            NightChange::New {
                memory: "mem_b".into()
            },
            NightChange::Link {
                memory: "mem_a".into(),
                entity: "ent".into()
            },
        ]
    );

    let every = [
        NightChange::New { memory: "m".into() },
        NightChange::Link {
            memory: "m".into(),
            entity: "e".into(),
        },
        NightChange::Update {
            page: "p".into(),
            before: 1,
            after: 2,
        },
        NightChange::Replace {
            old: "m".into(),
            new: "n".into(),
        },
        NightChange::Merge {
            from: "e".into(),
            into: "f".into(),
        },
        NightChange::Remove { memory: "m".into() },
    ];
    let text = serde_json::to_string(&every).unwrap();
    for kind in ["new", "link", "update", "replace", "merge", "remove"] {
        assert!(text.contains(&format!("\"kind\":\"{kind}\"")), "{text}");
    }
    assert_eq!(
        serde_json::from_str::<Vec<NightChange>>(&text).unwrap(),
        every
    );

    // An entry written before this still reads, and undo has no links to remove.
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE thing_history SET data = '{\"decided\":[\"mem_a\"],\"open\":[]}'
              WHERE op = 'thing.dream' AND thing_id = ?1",
            libsql::params![site.as_str()],
        )
        .await
        .unwrap();
    let dream = db.thing_dream(&site).await.unwrap().unwrap();
    assert_eq!((dream.decided, dream.open), (1, 0));
    assert_eq!(db.undo_thing_dream(&site).await.unwrap(), ["mem_a"]);
}

/// UX-CHAT-5: connecting old points is recorded, but it is not a night: the
/// thing's page, its last night and its night undo pass over it. Its own undo
/// takes back only what it linked, and lets it be run again.
#[tokio::test]
async fn ux_chat_5_connecting_old_points_is_not_a_night_and_has_its_own_undo() {
    let (db, _dir) = test_db().await;
    let site = one_thing(&db).await;
    remember(&db, "mem_a", "wenlan").await;
    remember(&db, "mem_b", "wenlan").await;
    db.record_dream(
        &site,
        &["mem_a".into()],
        &["mem_b".into()],
        &Default::default(),
        &[],
        &[],
    )
    .await
    .unwrap();
    let before = db.thing_dream(&site).await.unwrap().unwrap();
    let thing_before = db.list_things(&ReadScope::Global, None).await.unwrap()[0].clone();
    let unlinked: Vec<String> = db
        .unlinked_night_points(&site)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(unlinked, ["mem_a", "mem_b"]);
    let space = db.unlinked_night_points(&site).await.unwrap()[0]
        .space
        .clone();
    assert_eq!(space.as_deref(), Some("wenlan"));

    let pricing = topic(&db, "Pricing", Some("wenlan")).await;
    let already = topic(&db, "Hero Image", Some("wenlan")).await;
    let link = db
        .link_night_point("mem_a", &pricing)
        .await
        .unwrap()
        .unwrap();
    db.record_night_links_backfill(&site, &[link])
        .await
        .unwrap();
    db.mark_night_links_backfilled(1).await.unwrap();
    // Linked by other work after that.
    db.link_memory_entities("mem_a", &[already.as_str()])
        .await
        .unwrap();

    // Only the point with nothing is still unlinked.
    let unlinked: Vec<String> = db
        .unlinked_night_points(&site)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(unlinked, ["mem_b"]);
    assert!(db.night_links_backfilled().await.unwrap().is_some());

    // The thing still says what its last night wrote, at its time.
    let after = db.thing_dream(&site).await.unwrap().unwrap();
    assert_eq!(
        (after.decided, after.open, after.at),
        (before.decided, before.open, before.at)
    );
    let thing_after = db.list_things(&ReadScope::Global, None).await.unwrap()[0].clone();
    assert_eq!(thing_after.dreamed_at, thing_before.dreamed_at);

    // Its own undo: the link goes, other work's stays, and it can run again.
    assert_eq!(db.undo_night_links_backfill().await.unwrap(), 1);
    assert_eq!(links_of(&db, "mem_a").await, [already.as_str()]);
    assert_eq!(main_topic(&db, "mem_a").await, Some(already));
    assert!(db.night_links_backfilled().await.unwrap().is_none());
    assert_eq!(
        db.undo_night_links_backfill().await.unwrap(),
        0,
        "only once"
    );

    // The night undo skips the taken-back entry and finds the night itself.
    assert_eq!(
        db.undo_thing_dream(&site).await.unwrap(),
        ["mem_a", "mem_b"]
    );
    assert!(db.thing_dream(&site).await.unwrap().is_none());
}
