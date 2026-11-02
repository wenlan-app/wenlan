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
