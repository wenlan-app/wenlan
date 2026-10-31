// SPDX-License-Identifier: Apache-2.0
//! Keeping the originals of an imported chat (A7: UX-ART-1..3, UX-FIND-2/3).

use super::*;
use crate::chat_import::bulk_ingest::{
    bulk_import_conversations, bulk_import_conversations_keeping_originals, OriginalsTarget,
};
use crate::chat_import::types::{MessageRole, ParsedMessage, Vendor};
use crate::events::{EventEmitter, NoopEmitter};
use std::sync::Arc;
use wenlan_types::requests::UpdatePageRequest;

fn emitter() -> Arc<dyn EventEmitter> {
    Arc::new(NoopEmitter)
}

fn at(secs: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(secs, 0)
}

fn artifact(id: &str, title: &str, kind: &str, content: &str) -> ParsedArtifact {
    ParsedArtifact {
        id: id.into(),
        title: Some(title.into()),
        kind: kind.into(),
        language: None,
        content: content.into(),
        created_at: at(1_775_000_000),
    }
}

fn conversation(id: &str, artifacts: Vec<ParsedArtifact>) -> ParsedConversation {
    ParsedConversation {
        external_id: id.into(),
        vendor: Vendor::Claude,
        title: Some("Launch site".into()),
        created_at: at(1_774_999_000),
        summary: None,
        messages: vec![ParsedMessage {
            role: MessageRole::Assistant,
            content: "User: make a landing page\nAssistant: Here it is.".into(),
            created_at: at(1_775_000_000),
        }],
        artifacts,
    }
}

const HTML: &str = "<!doctype html>\n<h1>Launch</h1>\n<pre>```</pre>\n";

#[tokio::test]
async fn an_import_keeps_each_artifact_as_a_file_and_an_original_page() {
    let (db, _tmp) = crate::db::tests::test_db().await;
    let db = Arc::new(db);
    let root = tempfile::tempdir().unwrap();
    let batch = vec![
        conversation(
            "conv-1",
            vec![
                artifact("landing", "Landing", "text/html", HTML),
                artifact("plan", "Launch plan", "text/markdown", "# Plan\n\nShip it."),
            ],
        ),
        conversation("conv-2", vec![]),
    ];
    let result = bulk_import_conversations_keeping_originals(
        db.clone(),
        &batch,
        emitter(),
        "imp",
        OriginalsTarget {
            data_root: root.path(),
            knowledge_path: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.conversations_ingested, 2);
    assert_eq!(result.originals_kept, 2);
    assert_eq!(result.originals_failed, 0);

    // The files hold the original byte for byte.
    let dir = root.path().join("originals/claude/conv-1");
    assert_eq!(
        std::fs::read_to_string(dir.join("landing.html")).unwrap(),
        HTML
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("plan.md")).unwrap(),
        "# Plan\n\nShip it."
    );
    assert!(
        !root.path().join("originals/claude/conv-2").exists(),
        "a chat without artifacts writes nothing"
    );

    let records = db
        .originals_for_conversation("claude", "conv-1")
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    let landing = records.iter().find(|r| r.artifact_id == "landing").unwrap();
    assert_eq!(landing.file_path, "originals/claude/conv-1/landing.html");
    assert_eq!(landing.conversation_url, "https://claude.ai/chat/conv-1");
    assert_eq!(landing.written_at, Some(1_775_000_000));
    assert_eq!(
        db.get_original(&landing.page_id).await.unwrap().as_ref(),
        Some(landing)
    );

    // The page is the original's, links back to its chat and time.
    let page = db.get_page(&landing.page_id).await.unwrap().unwrap();
    assert_eq!(page.creation_kind, "original");
    assert_eq!(page.title, "Landing");
    assert_eq!(
        page.source_memory_ids,
        vec!["import_claude_conv-1".to_string()]
    );
    assert!(
        page.content.contains("(https://claude.ai/chat/conv-1)"),
        "{}",
        page.content
    );
    assert!(
        page.content.contains("written 2026-03-31"),
        "{}",
        page.content
    );
    assert!(
        page.content.contains(&format!("````html\n{}````", HTML)),
        "a fence longer than the original's backticks: {}",
        page.content
    );
    let plan = records.iter().find(|r| r.artifact_id == "plan").unwrap();
    let plan_page = db.get_page(&plan.page_id).await.unwrap().unwrap();
    assert!(plan_page.content.ends_with("# Plan\n\nShip it.\n"));

    // The manifest lists both with their pages.
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["vendor"], "claude");
    assert_eq!(manifest["conversation_id"], "conv-1");
    assert_eq!(
        manifest["conversation_url"],
        "https://claude.ai/chat/conv-1"
    );
    assert_eq!(manifest["artifacts"].as_array().unwrap().len(), 2);

    // The chat's memories carry the conversation's URL.
    let (with_url, all) = (
        db.originals_scalar_for_test(
            "SELECT COUNT(*) FROM memories WHERE source_id = 'import_claude_conv-1'
               AND url = 'https://claude.ai/chat/conv-1'",
        )
        .await,
        db.originals_scalar_for_test(
            "SELECT COUNT(*) FROM memories WHERE source_id = 'import_claude_conv-1'",
        )
        .await,
    );
    assert!(all > 0);
    assert_eq!(with_url, all);

    // Keeping again (a retry) adds nothing.
    let again = keep_conversation_originals(&db, root.path(), None, &batch[0])
        .await
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(
        db.originals_for_conversation("claude", "conv-1")
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn an_import_without_a_target_keeps_text_only() {
    let (db, _tmp) = crate::db::tests::test_db().await;
    let db = Arc::new(db);
    let batch = vec![conversation(
        "conv-text",
        vec![artifact("a", "A", "text/html", "<p>a</p>")],
    )];
    let result = bulk_import_conversations(db.clone(), &batch, emitter(), "imp")
        .await
        .unwrap();
    assert_eq!(result.memories_stored, 1);
    assert_eq!(result.originals_kept, 0);
    assert!(db
        .originals_for_conversation("claude", "conv-text")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn an_original_never_takes_over_a_page_with_the_same_title() {
    let (db, _tmp) = crate::db::tests::test_db().await;
    let db = Arc::new(db);
    let mine = crate::post_write::create_page(
        &db,
        CreateConceptRequest {
            title: "Landing".into(),
            content: "My own notes on the landing page.".into(),
            summary: None,
            entity_id: None,
            space: Option::<String>::None.into(),
            source_memory_ids: vec![],
            creation_kind: Some("authored".into()),
            workspace: None,
        },
        "test",
        None,
    )
    .await
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let batch = vec![conversation(
        "conv-3",
        vec![artifact("landing", "Landing", "text/html", HTML)],
    )];
    bulk_import_conversations_keeping_originals(
        db.clone(),
        &batch,
        emitter(),
        "imp",
        OriginalsTarget {
            data_root: root.path(),
            knowledge_path: None,
        },
    )
    .await
    .unwrap();
    let record = &db
        .originals_for_conversation("claude", "conv-3")
        .await
        .unwrap()[0];
    assert_ne!(record.page_id, mine.id);
    let page = db.get_page(&record.page_id).await.unwrap().unwrap();
    assert_eq!(page.title, "Landing (Claude Launch site)");
    let my_page = db.get_page(&mine.id).await.unwrap().unwrap();
    assert!(
        my_page.source_memory_ids.is_empty(),
        "my page gained no sources"
    );
}

#[tokio::test]
async fn a_machine_never_rewrites_an_original_but_the_person_may_edit_it() {
    let (db, _tmp) = crate::db::tests::test_db().await;
    let db = Arc::new(db);
    let root = tempfile::tempdir().unwrap();
    let batch = vec![conversation(
        "conv-4",
        vec![artifact("landing", "Landing", "text/html", HTML)],
    )];
    bulk_import_conversations_keeping_originals(
        db.clone(),
        &batch,
        emitter(),
        "imp",
        OriginalsTarget {
            data_root: root.path(),
            knowledge_path: None,
        },
    )
    .await
    .unwrap();
    let record = db
        .originals_for_conversation("claude", "conv-4")
        .await
        .unwrap()
        .remove(0);
    let page = db.get_page(&record.page_id).await.unwrap().unwrap();
    let write = |content: String, version: Option<i64>| UpdatePageRequest {
        content,
        source_memory_ids: page.source_memory_ids.clone(),
        expected_version: version,
        caller_id: None,
        operation_id: None,
    };
    for writer in [
        "agent_refresh",
        "distill",
        "re_distill",
        "page_growth",
        "refinery_merge",
    ] {
        let refused = crate::post_write::update_page(
            &db,
            &page.id,
            write(
                format!("{}\nRewritten by {writer}.", page.content),
                Some(page.version),
            ),
            writer,
            false,
            None,
            None,
        )
        .await
        .expect_err(writer);
        assert!(
            refused.to_string().contains("original_page_is_read_only"),
            "{writer}: {refused}"
        );
    }
    assert_eq!(
        db.get_page(&page.id).await.unwrap().unwrap().content,
        page.content
    );

    let edited = format!("{}\nMy note: this shipped.", page.content);
    let result = crate::post_write::update_page(
        &db,
        &page.id,
        write(edited.clone(), Some(page.version)),
        "manual_edit",
        false,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(result.wrote);
    assert_eq!(
        db.get_page(&page.id).await.unwrap().unwrap().content,
        edited
    );
    // The kept file is the original either way.
    assert_eq!(
        std::fs::read_to_string(root.path().join(&record.file_path)).unwrap(),
        HTML
    );
}

#[test]
fn export_ids_become_safe_path_segments() {
    assert_eq!(safe_segment("3f2a-11ef_x.y"), "3f2a-11ef_x.y");
    assert_eq!(safe_segment("../../etc"), ".._.._etc");
    assert_eq!(safe_segment(".."), "_");
    assert_eq!(safe_segment(""), "_");
    assert_eq!(safe_segment("a/b\\c d"), "a_b_c_d");
}

#[tokio::test]
async fn migration_145_is_idempotent_on_an_upgraded_database() {
    let (db, dir) = crate::db::tests::test_db_at(144).await;
    drop(db);
    for _ in 0..2 {
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
            .await
            .unwrap();
        let found = db
            .originals_scalar_for_test(
                "SELECT
                    (SELECT COUNT(*) FROM sqlite_master WHERE name = 'originals')
                  + (SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'pages'
                       AND sql LIKE '%''entity'',''original'')%')
                  + (SELECT user_version FROM pragma_user_version)",
            )
            .await;
        assert_eq!(found, 2 + 145);
    }
}

#[tokio::test]
async fn page_original_answers_only_for_an_original_page() {
    let (db, _tmp) = crate::db::tests::test_db().await;
    let db = Arc::new(db);
    let root = tempfile::tempdir().unwrap();
    let batch = vec![conversation(
        "conv-5",
        vec![artifact("landing", "Landing", "text/html", HTML)],
    )];
    bulk_import_conversations_keeping_originals(
        db.clone(),
        &batch,
        emitter(),
        "imp",
        OriginalsTarget {
            data_root: root.path(),
            knowledge_path: None,
        },
    )
    .await
    .unwrap();
    let record = db
        .originals_for_conversation("claude", "conv-5")
        .await
        .unwrap()
        .remove(0);
    let original = page_original(&db, &record.page_id).await.unwrap().unwrap();
    assert_eq!(original.vendor, "claude");
    assert_eq!(original.conversation_id, "conv-5");
    assert_eq!(original.conversation_url, "https://claude.ai/chat/conv-5");
    assert_eq!(original.artifact_id, "landing");
    assert_eq!(original.file_path, "originals/claude/conv-5/landing.html");
    assert_eq!(page_original(&db, "page_nope").await.unwrap(), None);
}
