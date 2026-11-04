// SPDX-License-Identifier: Apache-2.0

use super::tests::{test_db, test_db_at};
use crate::WenlanError;
use wenlan_types::{ThingMapAuthor, ThingMapOp, ThingMapSource, ThingMapState};

fn add(id: &str, parent: Option<&str>, kind: &str, label: &str) -> ThingMapOp {
    ThingMapOp::Add {
        id: Some(id.into()),
        parent_id: parent.map(str::to_string),
        rank: None,
        kind: kind.into(),
        state: None,
        label: label.into(),
        sources: Vec::new(),
    }
}

fn edit_label(id: &str, label: &str) -> ThingMapOp {
    ThingMapOp::Edit {
        id: id.into(),
        kind: None,
        state: None,
        label: Some(label.into()),
        sources: None,
    }
}

#[tokio::test]
async fn an_empty_map_grows_in_order_and_keeps_its_sources() {
    let (db, _dir) = test_db().await;
    let empty = db.get_thing_map("thg_a").await.unwrap();
    assert_eq!((empty.revision, empty.nodes.len()), (0, 0));

    let ops = vec![
        add("root", None, "question", "Which tagline?"),
        ThingMapOp::Add {
            id: Some("b".into()),
            parent_id: Some("root".into()),
            rank: None,
            kind: "option".into(),
            state: Some(ThingMapState::Open),
            label: "  Option B  ".into(),
            sources: vec![ThingMapSource {
                session_id: "ses_1".into(),
                from: Some(4),
                to: Some(6),
                memory_id: None,
            }],
        },
        add("a", Some("root"), "option", "Option A"),
        ThingMapOp::Link {
            from: "a".into(),
            to: "root".into(),
            kind: "answers".into(),
        },
    ];
    let (map, proposed) = db
        .apply_thing_map_ops("thg_a", ThingMapAuthor::Ai, 0, &ops)
        .await
        .unwrap();
    assert_eq!((map.revision, proposed), (1, 0));
    let labels: Vec<_> = map.nodes.iter().map(|n| n.label.as_str()).collect();
    // The root first, then its children in the order they were added.
    assert_eq!(labels, ["Which tagline?", "Option B", "Option A"]);
    assert_eq!(map.nodes[1].sources[0].from, Some(4));
    assert_eq!(map.nodes[1].by, ThingMapAuthor::Ai);
    assert_eq!(map.edges.len(), 1);
    assert_eq!(db.get_thing_map("thg_a").await.unwrap(), map);
}

#[tokio::test]
async fn a_stale_revision_or_a_bad_op_changes_nothing() {
    let (db, _dir) = test_db().await;
    db.apply_thing_map_ops(
        "thg_a",
        ThingMapAuthor::You,
        0,
        &[add("r", None, "topic", "Plan")],
    )
    .await
    .unwrap();
    let stale = db
        .apply_thing_map_ops("thg_a", ThingMapAuthor::You, 0, &[edit_label("r", "x")])
        .await;
    assert!(matches!(stale, Err(WenlanError::Conflict(_))), "{stale:?}");

    // The second op fails, so the first does not land either.
    let bad = db
        .apply_thing_map_ops(
            "thg_a",
            ThingMapAuthor::You,
            1,
            &[
                add("c", Some("r"), "topic", "Child"),
                add("d", Some("nope"), "topic", "Orphan"),
            ],
        )
        .await;
    assert!(matches!(bad, Err(WenlanError::Validation(_))), "{bad:?}");
    for op in [
        add("e", None, "topic", "A second root"),
        add("f", Some("r"), "Not A Kind", "Label"),
        add("g", Some("r"), "topic", "   "),
        ThingMapOp::Move {
            id: "r".into(),
            parent_id: "r".into(),
            rank: None,
        },
    ] {
        let refused = db
            .apply_thing_map_ops("thg_a", ThingMapAuthor::You, 1, std::slice::from_ref(&op))
            .await;
        assert!(matches!(refused, Err(WenlanError::Validation(_))), "{op:?}");
    }
    let map = db.get_thing_map("thg_a").await.unwrap();
    assert_eq!((map.revision, map.nodes.len()), (1, 1));
}

#[tokio::test]
async fn ux_who_2_the_ai_proposes_on_the_persons_points_and_the_person_answers() {
    let (db, _dir) = test_db().await;
    db.apply_thing_map_ops(
        "thg_a",
        ThingMapAuthor::Ai,
        0,
        &[
            add("r", None, "topic", "Launch"),
            add("ai", Some("r"), "reason", "Cheaper"),
        ],
    )
    .await
    .unwrap();
    // The person rewrites the AI's point, so it is theirs now.
    let (map, _) = db
        .apply_thing_map_ops(
            "thg_a",
            ThingMapAuthor::You,
            1,
            &[edit_label("ai", "Cheaper to run")],
        )
        .await
        .unwrap();
    assert_eq!(map.nodes[1].by, ThingMapAuthor::You);

    // The AI's edit and removal of it wait; its own new point lands.
    let (map, proposed) = db
        .apply_thing_map_ops(
            "thg_a",
            ThingMapAuthor::Local,
            2,
            &[
                edit_label("ai", "Cheaper"),
                ThingMapOp::Remove { id: "ai".into() },
                add("new", Some("r"), "reason", "Faster"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(proposed, 2);
    assert_eq!(map.nodes.len(), 3);
    assert_eq!(map.nodes[1].label, "Cheaper to run");
    assert_eq!(map.proposals.len(), 2);
    assert_eq!(map.proposals[0].by, ThingMapAuthor::Local);

    // Only the person answers.
    let answer = |id: &str| ThingMapOp::Accept {
        proposal_id: id.to_string(),
    };
    let refused = db
        .apply_thing_map_ops(
            "thg_a",
            ThingMapAuthor::Ai,
            3,
            &[answer(&map.proposals[0].id)],
        )
        .await;
    assert!(matches!(refused, Err(WenlanError::Validation(_))));
    let (map, _) = db
        .apply_thing_map_ops(
            "thg_a",
            ThingMapAuthor::You,
            3,
            &[
                answer(&map.proposals[0].id),
                ThingMapOp::Reject {
                    proposal_id: map.proposals[1].id.clone(),
                },
            ],
        )
        .await
        .unwrap();
    assert!(map.proposals.is_empty());
    let point = map.nodes.iter().find(|n| n.id == "ai").unwrap();
    assert_eq!(
        (point.label.as_str(), point.by),
        ("Cheaper", ThingMapAuthor::Local)
    );
}

#[tokio::test]
async fn removing_a_point_removes_what_is_under_it_and_its_links() {
    let (db, _dir) = test_db().await;
    let (map, _) = db
        .apply_thing_map_ops(
            "thg_a",
            ThingMapAuthor::Ai,
            0,
            &[
                add("r", None, "topic", "Root"),
                add("q", Some("r"), "question", "Q"),
                add("o", Some("q"), "option", "O"),
                add("p", Some("r"), "topic", "P"),
                ThingMapOp::Link {
                    from: "p".into(),
                    to: "o".into(),
                    kind: "answers".into(),
                },
                ThingMapOp::Remove { id: "q".into() },
            ],
        )
        .await
        .unwrap();
    let ids: Vec<_> = map.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["r", "p"]);
    assert!(map.edges.is_empty());
}

#[tokio::test]
async fn migration_152_adds_the_thing_map_tables() {
    let (db, _temp) = test_db_at(151).await;
    db.migrate_152_thing_map(151).await.unwrap();
    db.migrate_152_thing_map(152).await.unwrap();
    let conn = db.conn.lock().await;
    for table in [
        "thing_maps",
        "thing_map_nodes",
        "thing_map_edges",
        "thing_map_proposals",
    ] {
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

fn night(
    memory: &str,
    text: &str,
    decided: bool,
    node: Option<&str>,
    replaces: bool,
) -> super::thing_map::DreamMapWrite {
    super::thing_map::DreamMapWrite {
        memory_id: memory.into(),
        session_id: "ses_1".into(),
        text: text.into(),
        decided,
        node: node.map(str::to_string),
        replaces,
    }
}

/// UX-CHAT-5: what a night keeps is the same point on the thing's map. A
/// point already on the map names its memory and takes the night's state;
/// a new one joins under the top point; a point the person wrote keeps
/// their words and gets a proposal (UX-WHO-2).
#[tokio::test]
async fn ux_chat_5_night_links_points_to_the_map() {
    let (db, _dir) = test_db().await;
    let ops = vec![
        add("root", None, "topic", "Site redesign"),
        ThingMapOp::Add {
            id: Some("tag".into()),
            parent_id: Some("root".into()),
            rank: None,
            kind: "question".into(),
            state: Some(ThingMapState::Open),
            label: "Which tagline?".into(),
            sources: vec![ThingMapSource {
                session_id: "ses_1".into(),
                from: Some(2),
                to: Some(3),
                memory_id: None,
            }],
        },
    ];
    db.apply_thing_map_ops("thg_a", ThingMapAuthor::Ai, 0, &ops)
        .await
        .unwrap();
    let mine = vec![ThingMapOp::Add {
        id: Some("price".into()),
        parent_id: Some("root".into()),
        rank: None,
        kind: "point".into(),
        state: Some(ThingMapState::Open),
        label: "Pricing page?".into(),
        sources: Vec::new(),
    }];
    db.apply_thing_map_ops("thg_a", ThingMapAuthor::You, 1, &mine)
        .await
        .unwrap();

    let view = db.dream_map_view("thg_a").await.unwrap();
    assert_eq!(
        view.unkept
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["tag", "price"],
        "the top point is not offered"
    );
    let changes = db
        .dream_into_map(
            "thg_a",
            "Site redesign",
            &[
                night(
                    "mem_tag",
                    "The tagline is You do the thinking.",
                    true,
                    Some("tag"),
                    false,
                ),
                night(
                    "mem_price",
                    "No pricing page for now.",
                    true,
                    Some("price"),
                    false,
                ),
                night("mem_docs", "Docs move to the site.", true, None, false),
            ],
        )
        .await
        .unwrap();
    let map = db.get_thing_map("thg_a").await.unwrap();
    let node = |id: &str| map.nodes.iter().find(|n| n.id == id).unwrap().clone();

    let tag = node("tag");
    assert_eq!(tag.label, "Which tagline?", "a link keeps the map's words");
    assert_eq!(tag.state, Some(ThingMapState::Decided));
    assert_eq!(tag.sources[0].memory_id.as_deref(), Some("mem_tag"));
    assert_eq!(tag.sources[0].from, Some(2), "the lines it cites stay");

    let price = node("price");
    assert_eq!(price.by, ThingMapAuthor::You);
    assert_eq!(
        price.state,
        Some(ThingMapState::Open),
        "the person's point waits"
    );
    assert_eq!(price.sources[0].memory_id.as_deref(), Some("mem_price"));
    assert_eq!(map.proposals.len(), 1);
    assert!(matches!(
        &map.proposals[0].change,
        ThingMapOp::Edit { id, state: Some(ThingMapState::Decided), label: None, .. } if id == "price"
    ));

    let docs = map
        .nodes
        .iter()
        .find(|n| {
            n.sources
                .iter()
                .any(|s| s.memory_id.as_deref() == Some("mem_docs"))
        })
        .expect("a new point joins the map");
    assert_eq!(docs.parent_id.as_deref(), Some("root"));
    assert_eq!(docs.by, ThingMapAuthor::Local);
    assert_eq!(docs.label, "Docs move to the site.");
    assert_eq!(
        (
            changes.added.len(),
            changes.changed.len(),
            changes.proposals.len()
        ),
        (1, 2, 1)
    );

    // The next night sees these as kept, and a replacement changes the
    // point's words on the same point.
    let view = db.dream_map_view("thg_a").await.unwrap();
    assert!(view.unkept.is_empty());
    assert_eq!(
        view.by_memory.get("mem_tag").map(String::as_str),
        Some("tag")
    );
    db.dream_into_map(
        "thg_a",
        "Site redesign",
        &[night(
            "mem_tag_2",
            "The tagline is Your AI keeps the notes.",
            true,
            Some("tag"),
            true,
        )],
    )
    .await
    .unwrap();
    let map = db.get_thing_map("thg_a").await.unwrap();
    let tag = map.nodes.iter().find(|n| n.id == "tag").unwrap();
    assert_eq!(tag.label, "The tagline is Your AI keeps the notes.");
    assert_eq!(tag.sources[0].memory_id.as_deref(), Some("mem_tag_2"));
}

/// UX-CHAT-5: on an empty map, a night's points hang under a top point named
/// after the thing.
#[tokio::test]
async fn ux_chat_5_night_starts_an_empty_map() {
    let (db, _dir) = test_db().await;
    db.dream_into_map(
        "thg_b",
        "Release notes",
        &[night("mem_a", "Notes ship weekly.", true, None, false)],
    )
    .await
    .unwrap();
    let map = db.get_thing_map("thg_b").await.unwrap();
    assert_eq!(map.nodes.len(), 2);
    let root = map.nodes.iter().find(|n| n.parent_id.is_none()).unwrap();
    assert_eq!(root.label, "Release notes");
    assert_eq!(map.revision, 1);
}

/// UX-CHAT-5: taking a night back puts the map back; what someone changed
/// since stays.
#[tokio::test]
async fn ux_chat_5_undo_puts_the_map_back() {
    let (db, _dir) = test_db().await;
    let ops = vec![
        add("root", None, "topic", "Site redesign"),
        ThingMapOp::Add {
            id: Some("tag".into()),
            parent_id: Some("root".into()),
            rank: None,
            kind: "question".into(),
            state: Some(ThingMapState::Open),
            label: "Which tagline?".into(),
            sources: Vec::new(),
        },
    ];
    db.apply_thing_map_ops("thg_a", ThingMapAuthor::Ai, 0, &ops)
        .await
        .unwrap();
    let before = db.get_thing_map("thg_a").await.unwrap();
    let changes = db
        .dream_into_map(
            "thg_a",
            "Site redesign",
            &[
                night("mem_tag", "Tagline settled.", true, Some("tag"), false),
                night("mem_new", "A new point.", false, None, false),
                night("mem_kept", "Another new point.", false, None, false),
            ],
        )
        .await
        .unwrap();
    // The person writes under one of the night's points: it stays.
    let kept = changes.added[1].clone();
    let rev = db.get_thing_map("thg_a").await.unwrap().revision;
    db.apply_thing_map_ops(
        "thg_a",
        ThingMapAuthor::You,
        rev,
        &[add("child", Some(&kept), "reason", "Because.")],
    )
    .await
    .unwrap();

    db.undo_dream_map("thg_a", &changes).await.unwrap();
    let after = db.get_thing_map("thg_a").await.unwrap();
    let tag = after.nodes.iter().find(|n| n.id == "tag").unwrap();
    assert_eq!(tag, before.nodes.iter().find(|n| n.id == "tag").unwrap());
    let ids: Vec<&str> = after.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        !ids.contains(&changes.added[0].as_str()),
        "an untouched new point goes"
    );
    assert!(
        ids.contains(&kept.as_str()) && ids.contains(&"child"),
        "{ids:?}"
    );
}
