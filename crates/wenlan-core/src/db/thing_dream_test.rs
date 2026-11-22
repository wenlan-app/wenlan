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
        last_content: "b".into(),
        first_at: last_at - 60,
        last_content_at: last_at,
        last_at,
        automated: false,
        on_behalf: false,
        touched: Vec::new(),
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
        db.attach_dream_point(id, &due[0].conversations[0].session_id, None)
            .await
            .unwrap();
    }
    db.record_dream(
        &site,
        &["mem_tagline".into()],
        &["mem_pricing".into()],
        &Default::default(),
        &[],
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
        &[],
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

async fn remember_in(db: &MemoryDB, id: &str, title: &str, content: &str, space: &str) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        metadata: HashMap::new(),
        memory_type: Some("decision".to_string()),
        space: Some(space.to_string()),
        ..Default::default()
    }])
    .await
    .unwrap();
}

/// UX-CHAT-5: the big map draws each thing with the points its nights wrote,
/// read from the thing's history: a night taken back is left out even while
/// its memories are still there, a thing with no night is not listed, and a
/// point in another Space stays out of a Space-scoped read.
#[tokio::test]
async fn ux_chat_5_the_big_map_reads_each_things_night_points() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _notes) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    let site_due = due.iter().find(|d| d.thing_id == site).unwrap();

    remember_in(
        &db,
        "mem_tagline",
        "Tagline: You do the thinking",
        "The site tagline is \"You do the thinking\".",
        "wenlan",
    )
    .await;
    remember_in(
        &db,
        "mem_pricing",
        "",
        "Pricing page is open.\nMore.",
        "wenlan",
    )
    .await;
    remember_in(&db, "mem_other", "Elsewhere", "Filed elsewhere.", "other").await;
    db.record_dream(
        &site,
        &["mem_tagline".into(), "mem_other".into()],
        &["mem_pricing".into()],
        &Default::default(),
        &[],
        &site_due.conversations,
    )
    .await
    .unwrap();
    // A second night, taken back; its memory is not forgotten here, so only
    // the history keeps it off the map.
    remember_in(&db, "mem_undone", "Undone", "A point taken back.", "wenlan").await;
    db.record_dream(
        &site,
        &["mem_undone".into()],
        &[],
        &Default::default(),
        &[],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(db.undo_thing_dream(&site).await.unwrap(), ["mem_undone"]);

    let things = db.thing_night_points(&ReadScope::Global).await.unwrap();
    assert_eq!(things.len(), 1, "the thing with no night is not listed");
    assert_eq!(things[0].thing_id, site);
    assert_eq!(things[0].title.as_deref(), Some("Site redesign"));
    let points: Vec<(&str, &str, bool)> = things[0]
        .points
        .iter()
        .map(|p| (p.memory_id.as_str(), p.title.as_str(), p.decided))
        .collect();
    assert_eq!(
        points,
        [
            ("mem_tagline", "Tagline: You do the thinking", true),
            ("mem_other", "Elsewhere", true),
            // No title: the first line of its text.
            ("mem_pricing", "Pricing page is open.", false),
        ],
        "the night taken back is left out"
    );
    assert!(things[0].points.iter().all(|p| p.created_at > 0));
    assert!(!things[0].quiet, "it went on today: home shows it");

    // Taken off home, it is still listed, marked quiet as home has it.
    db.hide_thing(&site).await.unwrap();
    let hidden = db.thing_night_points(&ReadScope::Global).await.unwrap();
    assert!(hidden[0].quiet);
    assert_eq!(hidden[0].points.len(), 3);

    // A Space-scoped caller sees the thing's points in that Space only.
    let scoped = db
        .thing_night_points(&ReadScope::Space("wenlan".into()))
        .await
        .unwrap();
    let ids: Vec<(&str, Vec<&str>)> = scoped
        .iter()
        .map(|t| {
            (
                t.thing_id.as_str(),
                t.points.iter().map(|p| p.memory_id.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        ids,
        [(site.as_str(), vec!["mem_tagline", "mem_pricing"])],
        "a point filed in another Space stays out"
    );
    assert_eq!(scoped[0].spaces, ["wenlan"]);
    let other = db
        .thing_night_points(&ReadScope::Space("nowhere".into()))
        .await
        .unwrap();
    assert!(
        other.is_empty(),
        "a Space with none of its conversations sees no thing"
    );

    // Taking the last night back leaves the map with no thing at all.
    db.undo_thing_dream(&site).await.unwrap();
    assert!(db
        .thing_night_points(&ReadScope::Global)
        .await
        .unwrap()
        .is_empty());
}

/// UX-GRAPH-5: the big map's morning line and Replay read what each thing's
/// newest night did, kept to the points the caller can still see; Undo on
/// the map takes back the newest night of every thing that ran since the
/// person's midnight, and nothing older.
#[tokio::test]
async fn ux_graph_5_last_night_is_read_and_taken_back_whole() {
    use crate::db::night_links::NightLink;
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, notes) = two_things(&db, now).await;
    remember_in(&db, "mem_tagline", "Tagline", "Tagline settled.", "wenlan").await;
    remember_in(&db, "mem_other", "Elsewhere", "Filed elsewhere.", "other").await;
    remember_in(
        &db,
        "mem_notes",
        "Notes",
        "Release notes drafted.",
        "wenlan",
    )
    .await;
    let link = |memory: &str| NightLink {
        memory: memory.into(),
        entity: "ent_site".into(),
    };
    db.record_dream(
        &site,
        &["mem_tagline".into(), "mem_other".into()],
        &[],
        &Default::default(),
        &[link("mem_tagline"), link("mem_other")],
        &[],
    )
    .await
    .unwrap();
    db.record_dream(
        &notes,
        &["mem_notes".into()],
        &[],
        &Default::default(),
        &[],
        &[],
    )
    .await
    .unwrap();

    let things = db.thing_night_points(&ReadScope::Global).await.unwrap();
    let site_night = things
        .iter()
        .find(|t| t.thing_id == site)
        .and_then(|t| t.last_night.clone())
        .expect("the site's night");
    assert_eq!(site_night.written, ["mem_tagline", "mem_other"]);
    assert_eq!(
        site_night
            .linked
            .iter()
            .map(|l| (l.memory_id.as_str(), l.entity_id.as_str()))
            .collect::<Vec<_>>(),
        [("mem_tagline", "ent_site"), ("mem_other", "ent_site")]
    );
    assert!(site_night.at >= now);
    // A Space-scoped read counts only that Space's points.
    let scoped = db
        .thing_night_points(&ReadScope::Space("wenlan".into()))
        .await
        .unwrap();
    let scoped_night = scoped
        .iter()
        .find(|t| t.thing_id == site)
        .and_then(|t| t.last_night.clone())
        .unwrap();
    assert_eq!(scoped_night.written, ["mem_tagline"]);
    assert_eq!(scoped_night.linked.len(), 1);

    // Nothing ran after a later midnight: nothing is taken back.
    let (none, wrote) = db
        .undo_nights_since(&ReadScope::Global, now + 3_600)
        .await
        .unwrap();
    assert_eq!((none, wrote.len()), (0, 0));
    let (undone, mut wrote) = db
        .undo_nights_since(&ReadScope::Global, now - 3_600)
        .await
        .unwrap();
    wrote.sort();
    assert_eq!(undone, 2);
    assert_eq!(wrote, ["mem_notes", "mem_other", "mem_tagline"]);
    let after = db.thing_night_points(&ReadScope::Global).await.unwrap();
    assert!(after.iter().all(|t| t.last_night.is_none()));
    // Taken back once only.
    let (again, _) = db
        .undo_nights_since(&ReadScope::Global, now - 3_600)
        .await
        .unwrap();
    assert_eq!(again, 0);
}

/// UX-VIEW-6: a thing's page opens with the conclusion its newest night wrote.
/// It belongs to that night only: taking the night back brings back the one
/// before, and a sentence whose point was replaced or is out of the caller's
/// scope is not shown, since its source no longer backs it.
#[tokio::test]
async fn ux_view_6_the_page_opens_with_the_newest_nights_conclusion() {
    use wenlan_types::ThingConclusionSentence;
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    let due = due.iter().find(|d| d.thing_id == site).unwrap();
    remember(
        &db,
        "mem_tagline",
        "The tagline is \"You do the thinking\".",
        "decision",
        None,
    )
    .await;
    remember(
        &db,
        "mem_pricing",
        "Whether to add a pricing page is open.",
        "fact",
        None,
    )
    .await;
    let first = db
        .record_dream(
            &site,
            &["mem_tagline".into()],
            &["mem_pricing".into()],
            &Default::default(),
            &[],
            &due.conversations,
        )
        .await
        .unwrap()
        .expect("a night with points has a number");
    let sentence = |text: &str, points: &[&str]| ThingConclusionSentence {
        text: text.into(),
        points: points.iter().map(|p| p.to_string()).collect(),
    };
    // Without a conclusion yet, the page keeps its old first line.
    let dream = db.thing_dream(&site).await.unwrap().unwrap();
    assert!(dream.conclusion.is_empty());
    db.set_dream_conclusion(
        &site,
        first,
        &[
            sentence("The tagline is settled.", &["mem_tagline"]),
            sentence("A pricing page is still open.", &["mem_pricing"]),
        ],
    )
    .await
    .unwrap();
    let detail = db
        .get_thing_detail(&site, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    let shown: Vec<String> = detail
        .dream
        .unwrap()
        .conclusion
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(
        shown,
        ["The tagline is settled.", "A pricing page is still open."]
    );
    // A caller that sees another Space gets no sentence resting on points it
    // cannot see.
    let elsewhere = db
        .thing_dream_in(&site, &ReadScope::Space("elsewhere".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(elsewhere.conclusion.is_empty());

    // The next night replaces the open point and writes no conclusion: the
    // older night's conclusion is not shown above newer points.
    remember(
        &db,
        "mem_pricing_2",
        "No pricing page for now.",
        "decision",
        Some("mem_pricing"),
    )
    .await;
    let second = db
        .record_dream(
            &site,
            &["mem_pricing_2".into()],
            &[],
            &Default::default(),
            &[],
            &[],
        )
        .await
        .unwrap()
        .unwrap();
    assert!(db
        .thing_dream(&site)
        .await
        .unwrap()
        .unwrap()
        .conclusion
        .is_empty());
    // A sentence of that night that still cites the replaced point is hidden;
    // the rest stays.
    db.set_dream_conclusion(
        &site,
        second,
        &[
            sentence(
                "The tagline is settled; no pricing page for now.",
                &["mem_tagline", "mem_pricing_2"],
            ),
            sentence("Pricing is still open.", &["mem_tagline", "mem_pricing"]),
        ],
    )
    .await
    .unwrap();
    let shown: Vec<String> = db
        .thing_dream(&site)
        .await
        .unwrap()
        .unwrap()
        .conclusion
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(shown, ["The tagline is settled; no pricing page for now."]);
    // Writing onto another thing's night changes nothing.
    db.set_dream_conclusion("thg_other", second, &[sentence("x", &["mem_tagline"])])
        .await
        .unwrap();
    assert_eq!(
        db.thing_dream(&site)
            .await
            .unwrap()
            .unwrap()
            .conclusion
            .len(),
        1
    );

    // Undo the second night: the first night's conclusion is back.
    db.undo_thing_dream(&site).await.unwrap();
    db.delete_by_source_id("memory", "mem_pricing_2")
        .await
        .unwrap();
    let back: Vec<String> = db
        .thing_dream(&site)
        .await
        .unwrap()
        .unwrap()
        .conclusion
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(
        back,
        ["The tagline is settled.", "A pricing page is still open."]
    );
}

/// Trust audit F6: a conversation whose transcript is gone is not marked
/// read. It waits until it goes on again, and the thing says how many could
/// not be read; the others are read as usual.
#[tokio::test]
async fn ux_chat_5_a_conversation_that_could_not_be_read_is_not_marked_read() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    let site_due = due.iter().find(|d| d.thing_id == site).unwrap();
    let c0 = site_due
        .conversations
        .iter()
        .find(|c| c.external_id == "c0")
        .unwrap()
        .clone();
    let c1 = site_due
        .conversations
        .iter()
        .find(|c| c.external_id == "c1")
        .unwrap()
        .clone();

    // c0's transcript is gone; c1 is read.
    db.mark_dream_unread(std::slice::from_ref(&c0))
        .await
        .unwrap();
    db.record_dream(
        &site,
        &[],
        &[],
        &Default::default(),
        &[],
        std::slice::from_ref(&c1),
    )
    .await
    .unwrap();
    assert!(
        db.things_to_dream(now, 10)
            .await
            .unwrap()
            .iter()
            .all(|d| d.thing_id != site),
        "nothing of the thing is due: c1 was read, c0 waits"
    );
    assert_eq!(
        db.thing_unread_tools(&site, &ReadScope::Global)
            .await
            .unwrap(),
        ["claude-code"]
    );
    let detail = db
        .get_thing_detail(&site, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.unread, 1, "the page can say one could not be read");

    // c0 goes on: it is due again, and no longer counted as unread.
    db.record_conversation_label(&found("c0", now - 30), &work("Site redesign"), None)
        .await
        .unwrap();
    let due = db.things_to_dream(now, 10).await.unwrap();
    let site_due = due.iter().find(|d| d.thing_id == site).expect("c0 went on");
    assert_eq!(site_due.conversations.len(), 1);
    assert!(db
        .thing_unread_tools(&site, &ReadScope::Global)
        .await
        .unwrap()
        .is_empty());
}

/// Trust audit F4, F5, F7: a night's point whose label named no Space goes
/// to its conversation's own Space, or the one Space of the thing's other
/// conversations; it is Wenlan's own writing, not put in the review list,
/// and it keeps the line it came from.
#[tokio::test]
async fn ux_chat_5_a_nights_point_lands_in_its_space_and_keeps_its_line() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    db.create_space("wenlan", None, false).await.unwrap();
    db.create_space("engineering", None, false).await.unwrap();
    let (site, _) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    let site_due = due.iter().find(|d| d.thing_id == site).unwrap();
    let session = |id: &str| {
        site_due
            .conversations
            .iter()
            .find(|c| c.external_id == id)
            .unwrap()
            .session_id
            .clone()
    };
    let (c0, c1) = (session("c0"), session("c1"));
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE agent_sessions SET space = 'engineering' WHERE id = ?1",
            libsql::params![c0.as_str()],
        )
        .await
        .unwrap();
    }
    // In a repository, the conversation's own Space.
    assert_eq!(
        db.dream_point_space(&site, &c0, true).await.unwrap(),
        Some("engineering".into())
    );
    // Outside one, the tool's default Space says nothing: the thing's other
    // conversation is in "wenlan".
    assert_eq!(
        db.dream_point_space(&site, &c0, false).await.unwrap(),
        Some("wenlan".into())
    );
    // The other conversation's Space is gone: no guess.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE agent_sessions SET space = 'gone' WHERE id = ?1",
            libsql::params![c1.as_str()],
        )
        .await
        .unwrap();
    }
    assert_eq!(db.dream_point_space(&site, &c0, false).await.unwrap(), None);

    remember(&db, "mem_page", "Resume stays one page.", "decision", None).await;
    db.attach_dream_point("mem_page", &c0, Some("ok keep it to one page"))
        .await
        .unwrap();
    let unconfirmed = db
        .list_unconfirmed_memories_scoped(50, &ReadScope::Global)
        .await
        .unwrap();
    assert!(
        unconfirmed.iter().all(|m| m.id != "mem_page"),
        "the person sees it on the thing, not in the review list"
    );
    {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT source_agent FROM memories WHERE source_id = 'mem_page' LIMIT 1",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "wenlan");
    }
    let anchor = db
        .memory_said_anchor("mem_page", &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(anchor.session_id.as_deref(), Some(c0.as_ref()));
    assert_eq!(anchor.line.as_deref(), Some("ok keep it to one page"));
}

/// Trust audit F9: a thing the rules made that no conversation can name any
/// more is not listed; one still being organized is.
#[tokio::test]
async fn ux_home_3_an_untitled_thing_no_conversation_can_name_is_not_listed() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let conversation = found("c9", now - 60);
    db.record_conversation_label(&conversation, &work("Chore"), None)
        .await
        .unwrap();
    let session = db.intake_pending(10).await.unwrap()[0].session_id.clone();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO things (id, title, title_by, created_at, updated_at)
             VALUES ('thg_unnamed', NULL, 'ai', ?1, ?1)",
            libsql::params![now],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO thing_sessions (thing_id, session_id, assigned_by, at)
             VALUES ('thg_unnamed', ?1, 'ai', ?2)",
            libsql::params![session.as_str(), now],
        )
        .await
        .unwrap();
    }
    let listed =
        |things: Vec<wenlan_types::ThingSummary>| things.iter().any(|t| t.id == "thg_unnamed");
    // Labelled as work and waiting to be grouped: still being organized.
    assert!(listed(
        db.list_things(&ReadScope::Global, None).await.unwrap()
    ));
    // Read as not work: it will never be named.
    let mut chore = work("Chore");
    chore.work = false;
    db.record_conversation_label(&conversation, &chore, None)
        .await
        .unwrap();
    assert!(!listed(
        db.list_things(&ReadScope::Global, None).await.unwrap()
    ));
    // Still found by its id.
    assert!(listed(
        db.list_things(&ReadScope::Global, Some("thg_unnamed"))
            .await
            .unwrap()
    ));
}

/// One memory of a thing's conversation, written by `agent` in `space`,
/// `ago` seconds ago.
async fn said_by(
    db: &MemoryDB,
    id: &str,
    content: &str,
    agent: Option<&str>,
    space: &str,
    session: &str,
    ago: i64,
) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: id.to_string(),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        metadata: HashMap::new(),
        memory_type: Some("decision".to_string()),
        space: Some(space.to_string()),
        source_agent: agent.map(str::to_string),
        ..Default::default()
    }])
    .await
    .unwrap();
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE memories SET session_id = ?2, created_at = ?3 WHERE source_id = ?1",
        libsql::params![id, session, chrono::Utc::now().timestamp() - ago],
    )
    .await
    .unwrap();
}

/// UX-GRAPH-7: a night offers only what an AI wrote, within one Space; the
/// person's own memory and one nothing names are never offered. A replace
/// hides the older, counts on the morning line, and Undo puts back the mode
/// it had without forgetting anything the night did not write.
#[tokio::test]
async fn ux_graph_7_a_night_tidies_only_what_an_ai_wrote_and_undo_puts_it_back() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _notes) = two_things(&db, now).await;
    db.create_space("personal", None, false).await.unwrap();
    let due = db.things_to_dream(now, 10).await.unwrap();
    let session = due
        .iter()
        .find(|d| d.thing_id == site)
        .unwrap()
        .conversations[0]
        .session_id
        .clone();
    let s = session.as_str();
    said_by(
        &db,
        "mem_old",
        "The site has a pricing page.",
        Some("claude-code"),
        "wenlan",
        s,
        7_200,
    )
    .await;
    said_by(
        &db,
        "mem_new",
        "The site has a pricing page now.",
        Some("codex"),
        "wenlan",
        s,
        60,
    )
    .await;
    said_by(
        &db,
        "mem_none",
        "The site has a pricing page soon.",
        None,
        "wenlan",
        s,
        600,
    )
    .await;
    said_by(
        &db,
        "mem_other",
        "The site has a pricing page too.",
        Some("codex"),
        "personal",
        s,
        600,
    )
    .await;
    said_by(
        &db,
        "mem_mine",
        "The site has a pricing page, I think.",
        Some("codex"),
        "wenlan",
        s,
        600,
    )
    .await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET authored_content = content WHERE source_id = 'mem_mine'",
            (),
        )
        .await
        .unwrap();
    }
    // A preference Wenlan's own night kept is never tidied: a merge would
    // write it again as a decision or a fact.
    said_by(
        &db,
        "mem_preference",
        "The site has a pricing page, always say so.",
        Some("wenlan"),
        "wenlan",
        s,
        60,
    )
    .await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET memory_type = 'preference' WHERE source_id = 'mem_preference'",
            (),
        )
        .await
        .unwrap();
    }

    let offer = db.tidy_offer(&site, now).await.unwrap();
    let mut offered: Vec<&str> = offer.items.iter().map(|i| i.id.as_str()).collect();
    offered.sort();
    assert_eq!(offered, ["mem_new", "mem_old"], "{offer:?}");
    assert!(offer.groups.iter().all(|g| g
        .iter()
        .all(|&i| offer.items[i].space.as_deref() == Some("wenlan"))));

    let before = db.get_memory_detail("mem_old").await.unwrap().unwrap();
    db.record_dream_tidied(
        &site,
        &[],
        &[],
        &Default::default(),
        &[],
        &[],
        &[crate::db::night_tidy::TidyApplied {
            how: wenlan_types::NightTidyKind::Replace,
            old: vec!["mem_old".into()],
            new: "mem_new".into(),
            wrote: false,
            text: None,
            map: Default::default(),
        }],
    )
    .await
    .unwrap();
    let hidden = db.get_memory_detail("mem_old").await.unwrap().unwrap();
    assert_eq!(hidden.supersede_mode, "evicted");
    let nights = db.thing_night_points(&ReadScope::Global).await.unwrap();
    let night = nights.iter().find(|n| n.thing_id == site).unwrap();
    let tidied = &night.last_night.as_ref().unwrap().tidied;
    assert_eq!(tidied.len(), 1, "a night that only tidied still shows");
    assert_eq!(tidied[0].replaced, ["mem_old"]);

    let forget = db.undo_thing_dream(&site).await.unwrap();
    assert!(
        forget.is_empty(),
        "the night wrote nothing, so nothing is forgotten"
    );
    let back = db.get_memory_detail("mem_old").await.unwrap().unwrap();
    assert_eq!(back.supersede_mode, before.supersede_mode);
    assert!(db.get_memory_detail("mem_new").await.unwrap().is_some());
}

fn replace(old: &str, new: &str) -> crate::db::night_tidy::TidyApplied {
    crate::db::night_tidy::TidyApplied {
        how: wenlan_types::NightTidyKind::Replace,
        old: vec![old.into()],
        new: new.into(),
        wrote: false,
        text: None,
        map: Default::default(),
    }
}

async fn mode(db: &MemoryDB, id: &str) -> String {
    db.get_memory_detail(id)
        .await
        .unwrap()
        .unwrap()
        .supersede_mode
}

/// UX-GRAPH-7: hiding a memory that had hidden an older one keeps the older
/// one hidden; a memory pinned since the offer, or a replace whose kept
/// memory is gone, is left alone; a tidy a later night took the place of is
/// not offered for Undo, and Undo puts everything back.
#[tokio::test]
async fn ux_graph_7_a_tidy_keeps_what_it_hid_hidden_and_checks_again_when_it_records() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _notes) = two_things(&db, now).await;
    let s = "ses_tidy";
    for (id, text, ago) in [
        ("mem_p", "Pricing is 5 dollars.", 9_000),
        ("mem_n", "Pricing is 6 dollars.", 7_000),
        ("mem_k", "There is no paid plan.", 5_000),
        ("mem_k2", "There is no paid plan for now.", 3_000),
        ("mem_pin", "Pricing is set.", 2_000),
        ("mem_gone", "Pricing was dropped.", 1_000),
    ] {
        said_by(&db, id, text, Some("claude-code"), "wenlan", s, ago).await;
    }
    {
        let conn = db.conn.lock().await;
        for sql in [
            "UPDATE memories SET supersedes = 'mem_p', supersede_mode = 'hide' WHERE source_id = 'mem_n'",
            "UPDATE memories SET pinned = 1 WHERE source_id = 'mem_pin'",
            "UPDATE memories SET supersede_mode = 'evicted' WHERE source_id = 'mem_gone'",
        ] {
            conn.execute(sql, ()).await.unwrap();
        }
    }
    let p_before = mode(&db, "mem_p").await;

    db.record_dream_tidied(
        &site,
        &[],
        &[],
        &Default::default(),
        &[],
        &[],
        &[
            replace("mem_n", "mem_k"),
            replace("mem_pin", "mem_k"),
            replace("mem_k", "mem_gone"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(mode(&db, "mem_n").await, "evicted");
    assert_eq!(
        mode(&db, "mem_p").await,
        "evicted",
        "what mem_n hid stays hidden"
    );
    assert_ne!(
        mode(&db, "mem_pin").await,
        "evicted",
        "pinned since the offer"
    );
    assert_ne!(mode(&db, "mem_k").await, "evicted", "its keep was gone");
    let tidies = db.thing_tidies(&site).await.unwrap();
    assert_eq!(tidies.len(), 1, "{tidies:?}");
    assert_eq!(tidies[0].memory_id, "mem_k");
    assert_eq!(tidies[0].replaced[0].source_id, "mem_n");

    // A later night takes mem_k's place: only that tidy is offered for Undo.
    db.record_dream_tidied(
        &site,
        &[],
        &[],
        &Default::default(),
        &[],
        &[],
        &[replace("mem_k", "mem_k2")],
    )
    .await
    .unwrap();
    let tidies = db.thing_tidies(&site).await.unwrap();
    assert_eq!(
        tidies
            .iter()
            .map(|t| t.memory_id.as_str())
            .collect::<Vec<_>>(),
        ["mem_k2"]
    );
    assert!(db.undo_tidy(&site, "mem_k").await.unwrap().is_none());

    // Undo, newest first, puts every mode back.
    db.undo_thing_dream(&site).await.unwrap();
    assert_ne!(mode(&db, "mem_k").await, "evicted");
    assert_eq!(db.undo_tidy(&site, "mem_k").await.unwrap(), Some(vec![]));
    assert_eq!(mode(&db, "mem_n").await, "hide");
    assert_eq!(mode(&db, "mem_p").await, p_before);
}

/// A conversation known only from its tool's own MCP calls (no transcript
/// was scanned, so it has no label), put in a thing as the sorting does.
async fn put_in_thing(db: &MemoryDB, thing: &str, session: &str, space: &str, last_write_at: i64) {
    let row = db
        .upsert_session_metadata(&wenlan_types::UpsertSessionRequest {
            session: session.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE agent_sessions SET space = ?1, last_write_at = ?2 WHERE id = ?3",
        libsql::params![space, last_write_at, row.id.as_str()],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO thing_sessions (thing_id, session_id, assigned_by, at)
         VALUES (?1, ?2, 'ai', ?3)",
        libsql::params![thing, row.id.as_str(), last_write_at],
    )
    .await
    .unwrap();
}

async fn unread_tools(db: &MemoryDB, thing: &str, scope: &ReadScope) -> Vec<String> {
    let mut tools = db.thing_unread_tools(thing, scope).await.unwrap();
    tools.sort();
    tools
}

/// Design v5 F7: a conversation from an AI tool Wenlan has no transcript
/// reader for never gets a label, so no night reads it. The thing's page says
/// it didn't get it, and names the tool; a gone transcript counts the same.
/// Only ones active since the thing was last read count, and only ones in the
/// caller's Space scope.
#[tokio::test]
async fn ux_chat_5_a_conversation_from_a_tool_with_no_transcript_reader_is_not_got() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _) = two_things(&db, now).await;
    put_in_thing(&db, &site, "cursor:s1", "wenlan", now - 100).await;
    put_in_thing(&db, &site, "codex:s2", "wenlan", now - 100).await;
    put_in_thing(&db, &site, "cursor:other", "private", now - 100).await;
    let global = ReadScope::Global;
    let detail = db.get_thing_detail(&site, &global).await.unwrap().unwrap();
    assert_eq!(detail.sessions.len(), 5, "c0, c1 and the three unlabelled");
    assert_eq!(
        (detail.unread, detail.unread_tools.as_slice()),
        (2, ["cursor".to_string()].as_slice()),
        "only the tool with no reader; codex has one and is not yet read"
    );
    assert_eq!(detail.waiting, 0);
    let wenlan = ReadScope::Space("wenlan".into());
    assert_eq!(unread_tools(&db, &site, &wenlan).await, ["cursor"]);
    let elsewhere = ReadScope::Space("other".into());
    assert!(unread_tools(&db, &site, &elsewhere).await.is_empty());

    // A gone transcript counts too, under its own tool, in its own Space.
    let due = db.things_to_dream(now, 10).await.unwrap();
    let c0 = due
        .iter()
        .find(|d| d.thing_id == site)
        .unwrap()
        .conversations
        .iter()
        .find(|c| c.external_id == "c0")
        .unwrap()
        .clone();
    db.mark_dream_unread(std::slice::from_ref(&c0))
        .await
        .unwrap();
    assert_eq!(
        unread_tools(&db, &site, &global).await,
        ["claude-code", "cursor", "cursor"]
    );
    assert_eq!(
        unread_tools(&db, &site, &wenlan).await,
        ["claude-code", "cursor"]
    );

    // A night reads the thing: the older cursor conversations are not said
    // again; one that went on after it still is.
    let c1 = due
        .iter()
        .find(|d| d.thing_id == site)
        .unwrap()
        .conversations
        .iter()
        .find(|c| c.external_id == "c1")
        .unwrap()
        .clone();
    remember(&db, "mem_f7", "The tagline is settled.", "decision", None).await;
    db.record_dream(
        &site,
        &["mem_f7".into()],
        &[],
        &Default::default(),
        &[],
        std::slice::from_ref(&c1),
    )
    .await
    .unwrap();
    put_in_thing(&db, &site, "cursor:later", "wenlan", now + 60).await;
    assert_eq!(
        unread_tools(&db, &site, &global).await,
        ["claude-code", "cursor"]
    );
}

/// Design v5 F7: what is due and not yet read, with the cutoff of a night
/// that had no AI, is counted for the page; read, gone and still going-on
/// conversations are not, nor are another Space's.
#[tokio::test]
async fn ux_chat_5_a_due_conversation_is_waiting_until_a_night_reads_it() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, notes) = two_things(&db, now).await;
    let global = ReadScope::Global;
    let waiting = |thing: String, since: i64, scope: ReadScope| {
        let db = &db;
        async move {
            db.thing_waiting_conversations(&thing, since, &scope)
                .await
                .unwrap()
        }
    };
    // c2 (Release notes) ended five minutes ago and was not due by then.
    assert_eq!(waiting(site.clone(), now - 1_800, global.clone()).await, 2);
    assert_eq!(waiting(notes.clone(), now - 1_800, global.clone()).await, 0);
    assert_eq!(waiting(notes, now, global.clone()).await, 1);
    let other = ReadScope::Space("other".into());
    assert_eq!(waiting(site.clone(), now - 1_800, other).await, 0);

    let due = db.things_to_dream(now - 1_800, 10).await.unwrap();
    let conv = |id: &str| {
        due[0]
            .conversations
            .iter()
            .find(|c| c.external_id == id)
            .unwrap()
            .clone()
    };
    db.record_dream(
        &site,
        &[],
        &[],
        &Default::default(),
        &[],
        std::slice::from_ref(&conv("c1")),
    )
    .await
    .unwrap();
    assert_eq!(waiting(site.clone(), now - 1_800, global.clone()).await, 1);
    db.mark_dream_unread(std::slice::from_ref(&conv("c0")))
        .await
        .unwrap();
    assert_eq!(waiting(site.clone(), now - 1_800, global.clone()).await, 0);
    let unread = unread_tools(&db, &site, &global).await;
    assert_eq!(unread, ["claude-code"], "gone is not waiting");
}

// ---------------------------------------------------------------------------
// Which Space a derived write goes to (UX-CHAT-5), and nights that only keep
// preferences, tool problems or lessons.
// ---------------------------------------------------------------------------

use crate::db::derived_space::{rewrite_space, SpaceFacts};
use crate::shared_space::{self, HostLink, Scope};
use wenlan_types::WriteSpaceTarget;

fn share_with_team(dir: &std::path::Path, space: &str) -> PathBuf {
    let path = dir.join("shared-spaces.json");
    let scope = Scope::Space(space.into());
    shared_space::join_at(&path, &scope, &dir.join(space), "me").unwrap();
    shared_space::update_share(&path, &scope, |settings| {
        settings.host = Some(HostLink {
            url: "https://team.example".into(),
            key: "k".into(),
            space: space.into(),
            space_id: None,
        });
    })
    .unwrap();
    path
}

/// A rewrite stays exactly where its source is, never in the default Space.
#[test]
fn a_rewrite_keeps_exactly_the_space_of_its_source() {
    assert_eq!(rewrite_space(None), WriteSpaceTarget::Uncategorized);
    assert_eq!(
        rewrite_space(Some("wenlan")),
        WriteSpaceTarget::Named("wenlan".into())
    );
}

/// The old points the one-time pass files are put in their thing's Space,
/// unless that Space is shared with other people and nothing the person set
/// up ties the conversation to it.
#[tokio::test]
async fn the_old_memory_pass_files_a_point_into_a_shared_space_only_with_a_fact() {
    let now = chrono::Utc::now().timestamp();
    for (shared, mapped, filed) in [
        (false, false, true),
        (true, false, false),
        (true, true, true),
    ] {
        let (db, dir) = test_db().await;
        db.create_space("wenlan", None, false).await.unwrap();
        let (site, _) = two_things(&db, now).await;
        let due = db.things_to_dream(now, 10).await.unwrap();
        let session = due
            .iter()
            .find(|d| d.thing_id == site)
            .unwrap()
            .conversations
            .iter()
            .find(|c| c.external_id == "c0")
            .unwrap()
            .session_id
            .clone();
        // One of the thing's points, written before nights filed them.
        remember(&db, "old_point", "The site stays free.", "decision", None).await;
        db.attach_dream_point("old_point", &session, None)
            .await
            .unwrap();
        db.dream_into_map(
            &site,
            "Site redesign",
            &[crate::db::thing_map::DreamMapWrite {
                memory_id: "old_point".into(),
                session_id: session.clone(),
                text: "The site stays free.".into(),
                decided: true,
                node: None,
                replaces: false,
            }],
        )
        .await
        .unwrap();
        // The thing's other conversation is in "wenlan", so that is where the
        // point would go; c0 ran in /w/wenlan.
        let settings = if shared {
            share_with_team(dir.path(), "wenlan")
        } else {
            dir.path().join("none.json")
        };
        let spaces_file = dir.path().join("spaces.toml");
        if mapped {
            std::fs::write(
                &spaces_file,
                "[[mapping]]\nprefix = \"/w/wenlan\"\nspace = \"wenlan\"\n",
            )
            .unwrap();
        }
        let facts = SpaceFacts::read(
            &settings,
            &dir.path().join("repos.json"),
            Some(&spaces_file),
            None,
        );
        db.apply_backlog(&crate::db::night_backlog::BacklogPlan::default(), &facts)
            .await
            .unwrap();
        let space = db
            .get_memory_detail("old_point")
            .await
            .unwrap()
            .unwrap()
            .space
            .filter(|s| !s.is_empty());
        assert_eq!(
            space.is_some(),
            filed,
            "shared {shared}, mapped {mapped}: {space:?}"
        );
    }
}

/// A night that kept only a preference, a tool problem or a lesson is not
/// "the night" of the thing: it has no morning line, no conclusion and no
/// Undo of its own. What it kept is not confirmed and still stands.
#[tokio::test]
async fn a_night_that_kept_only_working_memories_is_not_the_night() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    let site_due = due.iter().find(|d| d.thing_id == site).unwrap();
    let session = site_due.conversations[0].session_id.clone();
    remember(
        &db,
        "pref",
        "Answer in short sentences.",
        "preference",
        None,
    )
    .await;
    db.attach_dream_working("pref", &session, Some("keep it short"))
        .await
        .unwrap();
    let seq = db
        .record_dream_full(
            &site,
            &[],
            &[],
            &["pref".to_string()],
            &Default::default(),
            &[],
            &site_due.conversations,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(seq, None, "no night with a conclusion to write");
    assert!(db.thing_dream(&site).await.unwrap().is_none());
    let thing = db
        .list_things(&ReadScope::Global, Some(&site))
        .await
        .unwrap();
    assert_eq!(thing[0].dreamed_at, None);
    assert!(db.undo_thing_dream(&site).await.unwrap().is_empty());
    // Not confirmed, still standing for later nights.
    let kept = db.get_memory_detail("pref").await.unwrap().unwrap();
    assert!(!kept.confirmed, "it waits for the person");
    assert_eq!(kept.session_id.as_deref(), Some(session.as_str()));
    let standing = db.dream_working_in(&site, "").await.unwrap();
    assert_eq!(standing.len(), 1);
    // The conversations count as read.
    assert!(db
        .things_to_dream(now, 10)
        .await
        .unwrap()
        .iter()
        .all(|d| d.thing_id != site));
}

/// The standing preferences are the person's from every thing, newest kept
/// when there are more than the cap; a night taken back drops its own.
#[tokio::test]
async fn the_standing_preferences_come_from_every_thing() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, notes) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    for (thing, id, text) in [
        (&site, "pref_a", "Answer in short sentences."),
        (&notes, "pref_b", "Show the diff before committing."),
    ] {
        let dueing = due.iter().find(|d| &d.thing_id == thing);
        let session = match dueing {
            Some(d) => d.conversations[0].session_id.clone(),
            None => "none".into(),
        };
        remember(&db, id, text, "preference", None).await;
        db.attach_dream_working(id, &session, None).await.unwrap();
        db.record_dream_full(
            thing,
            &[],
            &[],
            &[id.to_string()],
            &Default::default(),
            &[],
            &[],
            &[],
        )
        .await
        .unwrap();
    }
    let all = db.dream_preferences(10).await.unwrap();
    assert_eq!(
        all.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ["pref_a", "pref_b"]
    );
    let newest = db.dream_preferences(1).await.unwrap();
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].id, "pref_b");
}

/// UX-GRAPH-7: a preference, tool problem or lesson the person has confirmed
/// is never offered to a tidy (so never hidden or replaced); the same two
/// unconfirmed are offered.
#[tokio::test]
async fn ux_graph_7_a_working_memory_the_person_confirmed_is_never_tidied() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    let (site, _notes) = two_things(&db, now).await;
    let due = db.things_to_dream(now, 10).await.unwrap();
    let session = due
        .iter()
        .find(|d| d.thing_id == site)
        .unwrap()
        .conversations[0]
        .session_id
        .clone();
    for (id, text, ago) in [
        ("w_old", "Answer me in short sentences when we talk.", 7_200),
        (
            "w_new",
            "Answer me in short sentences when we talk now.",
            60,
        ),
    ] {
        said_by(&db, id, text, Some("wenlan"), "wenlan", &session, ago).await;
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET memory_type = 'preference', confirmed = 0 WHERE source_id = ?1",
            libsql::params![id],
        )
        .await
        .unwrap();
    }
    let ids = |offer: &crate::db::night_tidy::TidyOffer| {
        let mut ids: Vec<String> = offer.items.iter().map(|i| i.id.clone()).collect();
        ids.sort();
        ids
    };
    let offer = db.tidy_offer(&site, now).await.unwrap();
    assert_eq!(ids(&offer), ["w_new", "w_old"], "{offer:?}");

    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET confirmed = 1 WHERE source_id = 'w_old'",
            (),
        )
        .await
        .unwrap();
    }
    let offer = db.tidy_offer(&site, now).await.unwrap();
    assert!(
        offer.items.iter().all(|i| i.id != "w_old"),
        "a confirmed one is not offered: {offer:?}"
    );
    assert!(offer.groups.is_empty(), "nothing left to merge: {offer:?}");

    // And one confirmed since the offer is not hidden when the night records.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET confirmed = 0 WHERE source_id = 'w_old'",
            (),
        )
        .await
        .unwrap();
    }
    let offer = db.tidy_offer(&site, now).await.unwrap();
    assert_eq!(ids(&offer), ["w_new", "w_old"]);
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET confirmed = 1 WHERE source_id = 'w_old'",
            (),
        )
        .await
        .unwrap();
    }
    db.record_dream_tidied(
        &site,
        &[],
        &[],
        &Default::default(),
        &[],
        &[],
        &[replace("w_old", "w_new")],
    )
    .await
    .unwrap();
    assert_ne!(mode(&db, "w_old").await, "evicted");
}

/// UX-CHAT-5: the preferences a night is shown include the confirmed ones in
/// no Space that the person or their tools saved, not only the ones nights
/// kept, so a newer one can replace them; an unconfirmed one saved by a tool
/// is not one of them.
#[tokio::test]
async fn the_standing_preferences_include_confirmed_ones_the_person_saved() {
    let (db, _dir) = test_db().await;
    remember(
        &db,
        "saved",
        "Answer me in short sentences.",
        "preference",
        None,
    )
    .await;
    remember(
        &db,
        "waiting",
        "Lead reviews with the risks.",
        "preference",
        None,
    )
    .await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET confirmed = 1 WHERE source_id = 'saved'",
            (),
        )
        .await
        .unwrap();
    }
    let standing = db.dream_preferences(40).await.unwrap();
    let ids: Vec<&str> = standing.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["saved"], "{standing:?}");
}
