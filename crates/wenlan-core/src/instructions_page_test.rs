// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::events::NoopEmitter;
use std::sync::Arc;

async fn test_db() -> (MemoryDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(&dir.path().join("test.db"), Arc::new(NoopEmitter))
        .await
        .unwrap();
    (db, dir)
}

async fn insert_memory(db: &MemoryDB, source_id: &str, memory_type: &str, content: &str) {
    let conn = db.test_primary_session().await;
    conn.execute(
        "INSERT INTO memories (
            id, content, source, source_id, title, chunk_index, last_modified,
            chunk_type, memory_type, space, source_agent, confidence,
            confirmed, word_count, enrichment_status, quality, is_recap,
            supersede_mode, stability
         )
         VALUES (
            ?1, ?2, 'memory', ?1, ?3, 0, ?4, 'document', ?5,
            ?6, 'test', 0.9, 1, 8, 'enriched', 'high', 0,
            'hide', 'learned'
         )",
        libsql::params![
            source_id,
            content,
            content.chars().take(40).collect::<String>(),
            chrono::Utc::now().timestamp(),
            memory_type,
            crate::db::UNFILED_SPACE_ID,
        ],
    )
    .await
    .unwrap();
}

#[test]
fn titles_follow_the_locale() {
    assert_eq!(instructions_title(Some("zh-TW")), "你希望 AI 怎麼跟你做事");
    assert_eq!(instructions_title(Some("zh-CN")), "你希望 AI 怎么跟你做事");
    assert_eq!(
        instructions_title(Some("es")),
        "Cómo quieres que la IA trabaje contigo"
    );
    assert_eq!(instructions_title(None), "How you want AI to work with you");
}

#[test]
fn locale_is_inferred_from_the_persons_text() {
    assert_eq!(
        infer_locale(["我喜歡簡短的回答，這樣比較好讀。"]),
        "zh-Hant"
    );
    assert_eq!(
        infer_locale(["我喜欢简短的回答，这样比较好读。"]),
        "zh-Hans"
    );
    assert_eq!(
        infer_locale(["Prefiero respuestas cortas. ¿Está bien? Sí, también."]),
        "es"
    );
    assert_eq!(infer_locale(["I prefer short answers."]), "en");
    assert_eq!(infer_locale(std::iter::empty()), "en");
}

#[tokio::test]
async fn seeds_once_from_the_persons_memories_and_is_found_by_role() {
    let (db, _dir) = test_db().await;
    // Nothing to seed from yet: no page, and seeding stays possible.
    assert_eq!(
        ensure_instructions_page(&db, None, None)
            .await
            .unwrap()
            .0
            .map(|p| p.id),
        None
    );

    insert_memory(&db, "mem_pref", "preference", "我喜歡先看結論，再看細節。").await;
    insert_memory(
        &db,
        "mem_gotcha",
        "gotcha",
        "這個專案的測試一定要設 WENLAN_NO_AUTOSTART=1。",
    )
    .await;
    insert_memory(&db, "mem_fact", "fact", "這一行不該出現。").await;

    let (page, seeded) = ensure_instructions_page(&db, None, None).await.unwrap();
    let page = page.expect("seeded page");
    assert!(seeded);
    assert_eq!(page.title, "你希望 AI 怎麼跟你做事");
    assert!(page
        .content
        .starts_with("這一頁寫你希望 AI 工具怎麼跟你做事。"));
    assert!(page
        .content
        .contains("## 你的偏好\n\n- 我喜歡先看結論，再看細節。"));
    assert!(page.content.contains("## 要小心的地方"));
    assert!(!page.content.contains("不該出現"));
    assert!(page.review_required, "AI changes must stage as proposals");
    assert_eq!(page.creation_kind, "authored");
    assert_eq!(
        db.page_role(&page.id).await.unwrap().as_deref(),
        Some("instructions")
    );

    // Found by role even after the person renames it.
    {
        let conn = db.test_primary_session().await;
        conn.execute(
            "UPDATE pages SET title = 'AI 守則' WHERE id = ?1",
            libsql::params![page.id.clone()],
        )
        .await
        .unwrap();
    }
    let (again, seeded_again) = ensure_instructions_page(&db, Some("en"), None)
        .await
        .unwrap();
    assert!(!seeded_again);
    let again = again.unwrap();
    assert_eq!(again.id, page.id);
    assert_eq!(again.title, "AI 守則");

    let payload = instructions_payload(&again);
    assert_eq!(payload.role, "instructions");
    assert_eq!(payload.link, format!("wenlan://page/{}", page.id));

    // A deleted page is not written again.
    db.set_page_role(&page.id, None).await.unwrap();
    assert!(find_instructions_page(&db).await.unwrap().is_none());
    let (none, seeded_none) = ensure_instructions_page(&db, None, None).await.unwrap();
    assert!(none.is_none());
    assert!(!seeded_none);
}

#[tokio::test]
async fn an_explicit_locale_names_the_page() {
    let (db, _dir) = test_db().await;
    insert_memory(&db, "mem_id", "identity", "I am a Rust developer.").await;
    let (page, _) = ensure_instructions_page(&db, Some("es-ES"), None)
        .await
        .unwrap();
    let page = page.unwrap();
    assert_eq!(page.title, "Cómo quieres que la IA trabaje contigo");
    assert!(page
        .content
        .contains("## Sobre ti\n\n- I am a Rust developer."));
}

#[test]
fn seed_body_opens_with_plain_sentences_for_people() {
    let body = seed_body(&[], "en");
    assert!(body.starts_with("This page says how you want AI tools to work with you."));
    assert_eq!(crate::page_body::short_part(&body), body.trim());
}
