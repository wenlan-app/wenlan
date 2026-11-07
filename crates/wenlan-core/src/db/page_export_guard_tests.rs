// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::commit_or_rollback;
use crate::db::tests::test_db;
use wenlan_types::pages::PageCitation;
fn scope() -> ReadScope {
    ReadScope::Space("Research".into())
}
fn citation() -> PageCitation {
    serde_json::from_value(serde_json::json!({"occurrence":1,"marker":1,"source_kind":"memory","locator":"paper","score":1.0,"status":"verified","scope":"sentence"})).unwrap()
}
async fn seed(db: &MemoryDB) -> String {
    let space = db.create_space("Research", None, false).await.unwrap();
    let conn = db.conn.lock().await;
    conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type,source_agent,content_hash) VALUES('paper','file','paper','Paper','The randomized trial measured a substantial improvement in memory after six weeks of training','Research',0,1,'text','zotero','import1')",()).await.unwrap();
    conn.execute("INSERT INTO pages(id,title,content,source_memory_ids,version,status,created_at,last_compiled,last_modified,space,workspace,citations,incarnation) VALUES('page','Title','The randomized trial measured a substantial improvement in memory after six weeks of training [1]','[\"paper\"]',1,'active','2026-01-01','2026-01-01','2026-01-01','Research','Research',?1,'guard-incarnation')",[serde_json::to_string(&vec![citation()]).unwrap()]).await.unwrap();
    space.id
}
async fn impact(db: &MemoryDB, space: &str, reason: &str) {
    let conn = db.conn.lock().await;
    conn.execute("BEGIN", ()).await.unwrap();
    super::super::zotero_scope_impacts::zotero_scope_impact_on(
        &conn,
        space,
        &["paper".into()],
        reason,
    )
    .await
    .unwrap();
    commit_or_rollback(&conn).await.unwrap();
}
async fn stage(db: &MemoryDB) -> String {
    let current = db.get_page("page").await.unwrap().unwrap();
    let revision = db.get_page_source_revision("page").await.unwrap();
    let body="The randomized trial measured a substantial improvement in memory after six weeks of training [1]\n\nHuman review recorded.";
    let crate::citations::WriteCitations::Checked {
        body: checked_body,
        citations_json,
        snapshots,
        ..
    } = crate::citations::check_write_citations(db, body, &["paper".into()], &scope())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("fixture must have checked citations")
    };
    assert_eq!(checked_body, body);
    crate::post_write::stage_page_revision_card_with_citations(
        db,
        &current,
        "The randomized trial measured a substantial improvement in memory after six weeks of training [1]\n\nHuman review recorded.",
        &["paper".into()],
        revision,
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&scope()),
        &snapshots,
        Some(&citations_json),
        None,
    )
    .await
    .unwrap()
    .revision_card_id
    .unwrap()
}
#[tokio::test]
async fn export_pending_membership_guard_clamps_draft_without_mutating_old_bundle() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    let old = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(old.citations.as_ref().unwrap()[0].status, "verified");
    impact(&db, &space, "selection removed").await;
    let pending = stage(&db).await;
    let current = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(current.citations.as_ref().unwrap()[0].status, "unevaluated");
    let versions = db.page_export_versions("page", &scope()).await.unwrap();
    let id = versions
        .versions
        .iter()
        .find(|v| v.kind == PageExportKind::PendingDraft)
        .unwrap()
        .bundle_id
        .as_ref()
        .unwrap();
    let draft = db.page_export_bundle("page", id, &scope()).await.unwrap();
    assert_eq!(draft.pending_revision_id.as_deref(), Some(pending.as_str()));
    assert_eq!(draft.citations.as_ref().unwrap()[0].status, "unevaluated");
    assert_eq!(
        db.page_export_bundle("page", &old.id, &scope())
            .await
            .unwrap()
            .digest,
        old.digest
    );
}
#[tokio::test]
async fn export_actual_review_adoption_clears_only_acknowledged_membership_guard() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    impact(&db, &space, "selection removed").await;
    let pending = stage(&db).await;
    crate::post_write::accept_pending_revision(&db, &pending, "user")
        .await
        .unwrap();
    let page = db.get_page("page").await.unwrap().unwrap();
    assert_eq!(
        page.content,
        "The randomized trial measured a substantial improvement in memory after six weeks of training [1]\n\nHuman review recorded."
    );
    assert_ne!(
        page.refresh_blocked_reason.as_deref(),
        Some(wenlan_types::zotero::ZOTERO_SELECTION_REVIEW_REQUIRED)
    );
    let conn = db.conn.lock().await;
    let state = state_on(&conn, "page").await.unwrap().unwrap();
    let bundles = bundles_on(&conn, &state).await.unwrap();
    let saved = bundles
        .iter()
        .filter(|b| b.kind == PageExportKind::Saved && b.version == 2)
        .max_by_key(|b| b.export_revision)
        .unwrap();
    assert_eq!(
        saved.citations.as_ref().unwrap()[0].status,
        "verified",
        "final history/export capture must follow guard acknowledgement"
    );
}
#[tokio::test]
async fn export_new_membership_event_after_stage_refuses_adoption_and_keeps_guard() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    impact(&db, &space, "first selection removal").await;
    let pending = stage(&db).await;
    impact(&db, &space, "later disconnect").await;
    assert!(
        crate::post_write::accept_pending_revision(&db, &pending, "user")
            .await
            .is_err()
    );
    let page = db.get_page("page").await.unwrap().unwrap();
    assert_eq!(page.content, "The randomized trial measured a substantial improvement in memory after six weeks of training [1]");
    assert_eq!(
        page.refresh_blocked_reason.as_deref(),
        Some(wenlan_types::zotero::ZOTERO_SELECTION_REVIEW_REQUIRED)
    );
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT value FROM app_metadata WHERE key='zotero_scope_impact:page'",
            (),
        )
        .await
        .unwrap();
    let raw: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
