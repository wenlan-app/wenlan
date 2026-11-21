// SPDX-License-Identifier: Apache-2.0

use super::*;

/// A Design page whose body carries sentence citations the member never sees.
async fn seed_cited_page(db: &Arc<MemoryDB>, title: &str, body: &str, markers: u32) -> String {
    let id = create_folder_test_page(db, title, "authored").await.id;
    db.update_page_content(&id, body, &[], "agent_refresh")
        .await
        .unwrap();
    let page = db.get_page(&id).await.unwrap().unwrap();
    let citations: Vec<Value> = (1..=markers)
        .map(|n| {
            json!({
                "occurrence": n, "marker": n, "source_kind": "memory",
                "locator": format!("round6-private-{n}"), "score": 1.0,
                "status": "verified", "scope": "sentence",
            })
        })
        .collect();
    let changelog = json!([{
        "version": page.version, "at": 1, "edited_by": "agent_refresh", "agent": TOOL,
    }]);
    db.set_page_citations_with_changelog(
        &id,
        Some(&Value::from(citations).to_string()),
        &changelog.to_string(),
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_page(&id).await.unwrap().unwrap().citations.len(),
        markers as usize
    );
    id
}

/// Reads the page as the member, edits the projection, and saves it.
async fn member_save(
    f: &Fixture,
    id: &str,
    edit: impl FnOnce(&str) -> String,
) -> (StatusCode, String) {
    let path = format!("/api/pages/{id}");
    let (status, reply) = forwarded(f, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let projected = reply["page"]["content"].as_str().unwrap().to_owned();
    let version = reply["page"]["version"].as_i64().unwrap();
    forwarded(
        f,
        Method::PUT,
        &path,
        json!({"expected_version": version, "content": edit(&projected)}),
    )
    .await
}

#[tokio::test]
async fn round6_member_edit_keeps_citations_on_repeated_sentences_in_unchanged_paragraphs() {
    let (mut f, db) = fixture_with_daemon().await;
    let root = f._dir.path().join("wiki");
    set_daemon(&mut f, &db, &root);
    let body = "Alpha context. Same claim.[1]\n\nIntro paragraph.\n\nBeta context. Same claim.[2]";
    let id = seed_cited_page(&db, "Round6 repeated claims", body, 2).await;
    let (status, reply) = member_save(&f, &id, |projected| {
        projected.replace("Intro paragraph.", "Intro paragraph, edited.")
    })
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Alpha context. Same claim.[1]\n\nIntro paragraph, edited.\n\nBeta context. Same claim.[2]"
    );
    assert_eq!(
        after
            .citations
            .iter()
            .map(|c| c.locator.as_str())
            .collect::<Vec<_>>(),
        ["round6-private-1", "round6-private-2"],
    );
}

#[tokio::test]
async fn round6_member_save_that_repeats_or_moves_cited_text_is_refused() {
    let (mut f, db) = fixture_with_daemon().await;
    let root = f._dir.path().join("wiki");
    set_daemon(&mut f, &db, &root);
    let body = "First claim.[1]\n\nSecond claim.[2]\n\nClosing words.";
    let cases: [(&str, fn(&str) -> String); 3] = [
        ("repeated", |p| {
            p.replace("First claim.", "First claim.\n\nFirst claim.")
        }),
        ("moved", |_| {
            "Second claim.\n\nFirst claim.\n\nClosing words.".into()
        }),
        ("repeated sentence", |p| {
            p.replace("Second claim.", "Second claim. Second claim.")
        }),
    ];
    for (n, (case, edit)) in cases.into_iter().enumerate() {
        let id = seed_cited_page(&db, &format!("Round6 cited {n}"), body, 2).await;
        let before = db.get_page(&id).await.unwrap().unwrap();
        let (status, reply) = member_save(&f, &id, edit).await;
        assert_eq!(status, StatusCode::CONFLICT, "{case}: {reply}");
        assert!(
            reply.contains("repeats or moves cited text"),
            "{case}: {reply}"
        );
        assert!(!reply.contains("round6-private"), "{case}: {reply}");
        let after = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(after.version, before.version, "{case}");
        assert_eq!(after.content, before.content, "{case}");
        assert_eq!(json!(after.citations), json!(before.citations), "{case}");
    }
}
