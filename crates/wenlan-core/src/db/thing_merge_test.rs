// SPDX-License-Identifier: Apache-2.0

use crate::conversation_intake::{FoundConversation, Grouping, Label};
use crate::db::tests::test_db;
use crate::read_scope::ReadScope;
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

async fn titles(db: &crate::db::MemoryDB) -> Vec<String> {
    let mut out: Vec<String> = db
        .list_things(&ReadScope::Global, None)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.title)
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn ux_thing_3_merging_is_only_a_proposal_until_the_person_presses_it() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    for (i, title) in ["Promo posts", "Launch growth", "Video hosting"]
        .iter()
        .enumerate()
    {
        db.record_conversation_label(
            &found(&format!("c{i}"), now - 600 + i as i64),
            &work(title),
            None,
        )
        .await
        .unwrap();
    }
    let pending = db.intake_pending(50).await.unwrap();
    let by_title = |t: &str| pending.iter().position(|p| p.title == t).unwrap();
    db.apply_intake_grouping(
        &pending,
        &[],
        &Grouping {
            new: ["Promo posts", "Launch growth", "Video hosting"]
                .iter()
                .map(|t| (t.to_string(), vec![by_title(t)]))
                .collect(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let open = db.intake_open_things().await.unwrap();
    let at = |t: &str| open.iter().position(|o| o.title == t).unwrap();
    let id = |t: &str| open[at(t)].id.clone();
    let (promo, growth, video) = (id("Promo posts"), id("Launch growth"), id("Video hosting"));

    // The grouping model says two look like one goal: nothing merges.
    db.apply_intake_grouping(
        &[],
        &open,
        &Grouping {
            same: vec![(at("Promo posts"), at("Launch growth"))],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(titles(&db).await.len(), 3);
    let offered = db.thing_merge_proposals(&promo).await.unwrap();
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].id, growth);
    assert_eq!(
        db.thing_merge_proposals(&growth).await.unwrap()[0].id,
        promo
    );
    assert!(db.thing_merge_proposals(&video).await.unwrap().is_empty());

    // Kept apart: gone, and the same pair is not asked again.
    db.keep_things_apart(&growth, &promo).await.unwrap();
    assert!(db.thing_merge_proposals(&promo).await.unwrap().is_empty());
    db.apply_intake_grouping(
        &[],
        &open,
        &Grouping {
            same: vec![
                (at("Promo posts"), at("Launch growth")),
                (at("Promo posts"), at("Video hosting")),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let offered = db.thing_merge_proposals(&promo).await.unwrap();
    assert_eq!(
        offered.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec![video.as_str()]
    );
    assert_eq!(titles(&db).await.len(), 3);

    // The person presses merge on Promo's page: Video folds into it.
    db.merge_things(&promo, &video).await.unwrap();
    assert_eq!(titles(&db).await, vec!["Launch growth", "Promo posts"]);
    let c2 = pending[by_title("Video hosting")].session_id.clone();
    let things: Vec<_> = db
        .session_things(&c2)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(things, vec![promo.clone()]);
    assert!(db.thing_merge_proposals(&promo).await.unwrap().is_empty());
    // Its conversation is the person's placement now, so grouping leaves it.
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE conversation_labels SET grouped_at = NULL", ())
            .await
            .unwrap();
    }
    assert!(db
        .intake_pending(50)
        .await
        .unwrap()
        .iter()
        .all(|p| p.session_id != c2));
    // A merged thing cannot merge again, or be proposed.
    assert!(db.merge_things(&promo, &video).await.is_err());
    assert!(db.merge_things(&promo, &promo).await.is_err());
}
