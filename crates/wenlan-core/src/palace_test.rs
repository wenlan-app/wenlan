// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::palace::{PalaceLine, PalaceNode, PalaceNote};

fn point(x: f64, y: f64) -> PalaceNode {
    PalaceNode {
        x,
        y,
        label: None,
        added_by: None,
        accepted: None,
    }
}

fn layout_with(points: &[(&str, f64, f64)]) -> PalaceLayout {
    let mut layout = PalaceLayout::default();
    for (id, x, y) in points {
        layout.nodes.insert((*id).to_string(), point(*x, *y));
    }
    layout
}

fn ai() -> PalaceWriter {
    PalaceWriter::Ai {
        agent: "claude-code".into(),
    }
}

#[tokio::test]
async fn positions_survive_a_new_store_and_writes_are_version_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let store = PalaceStore::new(tmp.path().join("layout"));
    assert_eq!(store.read().unwrap().version, 0);

    let first = store
        .write(
            0,
            layout_with(&[("page_a", 1.0, 2.0)]),
            PalaceWriteReason::Edit,
            &PalaceWriter::Person,
        )
        .await
        .unwrap();
    assert_eq!(first.version, 1);

    // A restart is a new store over the same folder.
    let reopened = PalaceStore::new(tmp.path().join("layout")).read().unwrap();
    assert_eq!(reopened.version, 1);
    assert_eq!(reopened.layout.nodes["page_a"], point(1.0, 2.0));

    // A write based on an old version is refused and changes nothing.
    match store
        .write(
            0,
            layout_with(&[("page_a", 9.0, 9.0)]),
            PalaceWriteReason::Edit,
            &PalaceWriter::Person,
        )
        .await
    {
        Err(PalaceError::VersionConflict { current }) => assert_eq!(current, 1),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(
        store.read().unwrap().layout.nodes["page_a"],
        point(1.0, 2.0)
    );

    // The file is the documented shape.
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.file_path()).unwrap()).unwrap();
    assert_eq!(raw["format"], 1);
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["updated_by"], "person");
    assert_eq!(
        raw["nodes"]["page_a"],
        serde_json::json!({"x": 1.0, "y": 2.0})
    );
}

#[tokio::test]
async fn a_relayout_can_be_reverted() {
    let tmp = tempfile::tempdir().unwrap();
    let store = PalaceStore::new(tmp.path());
    store
        .write(
            0,
            layout_with(&[("a", 1.0, 1.0), ("b", 2.0, 2.0)]),
            PalaceWriteReason::Edit,
            &PalaceWriter::Person,
        )
        .await
        .unwrap();
    let relaid = store
        .write(
            1,
            layout_with(&[("a", 50.0, 50.0), ("b", -50.0, -50.0)]),
            PalaceWriteReason::Relayout,
            &PalaceWriter::Person,
        )
        .await
        .unwrap();
    assert_eq!(relaid.reason, Some(PalaceWriteReason::Relayout));
    assert_eq!(relaid.earlier_versions, vec![1]);

    let back = store.revert(2, None).await.unwrap();
    assert_eq!(back.version, 3);
    assert_eq!(back.reason, Some(PalaceWriteReason::Revert));
    assert_eq!(back.layout.nodes["a"], point(1.0, 1.0));
    assert_eq!(back.earlier_versions, vec![2, 1]);
    // Undoing the undo goes forward again.
    let forward = store.revert(3, Some(2)).await.unwrap();
    assert_eq!(forward.layout.nodes["a"], point(50.0, 50.0));

    assert!(matches!(
        store.revert(4, Some(99)).await,
        Err(PalaceError::NoSuchVersion(99))
    ));
    assert!(matches!(
        store.revert(1, None).await,
        Err(PalaceError::VersionConflict { current: 4 })
    ));
}

#[tokio::test]
async fn only_the_last_versions_are_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let store = PalaceStore::new(tmp.path());
    for version in 0..(KEPT_VERSIONS as u64 + 5) {
        store
            .write(
                version,
                layout_with(&[("a", version as f64, 0.0)]),
                PalaceWriteReason::Edit,
                &PalaceWriter::Person,
            )
            .await
            .unwrap();
    }
    let earlier = store.earlier_versions().unwrap();
    assert_eq!(earlier.len(), KEPT_VERSIONS);
    assert_eq!(earlier[0], KEPT_VERSIONS as u64 + 4);
}

#[tokio::test]
async fn ai_additions_are_suggestions_and_only_the_person_accepts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = PalaceStore::new(tmp.path());
    let mut mine = layout_with(&[("page_a", 0.0, 0.0)]);
    mine.lines.push(PalaceLine {
        id: "l1".into(),
        from: "page_a".into(),
        to: "page_b".into(),
        label: None,
        added_by: Some(PalaceAuthor::Person),
        accepted: None,
    });
    store
        .write(0, mine, PalaceWriteReason::Edit, &PalaceWriter::Person)
        .await
        .unwrap();

    // The AI adds a point and a note, claims they are accepted, and drops
    // the person's line.
    let mut proposal = layout_with(&[("page_a", 0.0, 0.0)]);
    let mut extra = point(5.0, 5.0);
    extra.label = Some("Storage".into());
    extra.accepted = Some(true);
    extra.added_by = Some(PalaceAuthor::Person);
    proposal.nodes.insert("ai_1".into(), extra);
    proposal.notes.push(PalaceNote {
        id: "n1".into(),
        text: "check this".into(),
        x: 1.0,
        y: 1.0,
        node_id: None,
        added_by: None,
        accepted: Some(true),
    });
    let after = store
        .write(1, proposal, PalaceWriteReason::Edit, &ai())
        .await
        .unwrap();
    let suggested = &after.layout.nodes["ai_1"];
    assert_eq!(suggested.added_by, Some(PalaceAuthor::Ai));
    assert_eq!(suggested.accepted, Some(false));
    assert_eq!(after.layout.notes[0].accepted, Some(false));
    assert_eq!(after.layout.lines.len(), 1, "the person's line is kept");

    // The AI cannot accept its own suggestion.
    let mut again = after.layout.clone();
    again.nodes.get_mut("ai_1").unwrap().accepted = Some(true);
    let after = store
        .write(2, again, PalaceWriteReason::Edit, &ai())
        .await
        .unwrap();
    assert_eq!(after.layout.nodes["ai_1"].accepted, Some(false));

    // The person accepts it.
    let mut accepted = after.layout.clone();
    accepted.nodes.get_mut("ai_1").unwrap().accepted = Some(true);
    let after = store
        .write(3, accepted, PalaceWriteReason::Edit, &PalaceWriter::Person)
        .await
        .unwrap();
    assert_eq!(after.layout.nodes["ai_1"].accepted, Some(true));
    assert_eq!(after.layout.nodes["ai_1"].added_by, Some(PalaceAuthor::Ai));

    // And an AI tool cannot ask for a relayout.
    assert!(matches!(
        store
            .write(4, after.layout, PalaceWriteReason::Relayout, &ai())
            .await,
        Err(PalaceError::Invalid(_))
    ));
}

#[tokio::test]
async fn bad_layouts_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let store = PalaceStore::new(tmp.path());
    for layout in [
        layout_with(&[("a", f64::NAN, 0.0)]),
        layout_with(&[("", 0.0, 0.0)]),
        layout_with(&[(&"x".repeat(MAX_ID_CHARS + 1), 0.0, 0.0)]),
    ] {
        assert!(matches!(
            store
                .write(0, layout, PalaceWriteReason::Edit, &PalaceWriter::Person)
                .await,
            Err(PalaceError::Invalid(_))
        ));
    }
    assert!(!store.file_path().exists());
}
