// SPDX-License-Identifier: Apache-2.0

use super::{
    claim_derivation_test::publish_supported_page_for_rename_test, tests::test_db, MemoryDB,
    RestorePageError,
};
use crate::{read_scope::ReadScope, WenlanError};
use wenlan_types::responses::RestorePageErrorCode;

/// The refusal code, or a panic naming what came back instead.
fn refusal<T: std::fmt::Debug>(result: Result<T, RestorePageError>) -> (RestorePageErrorCode, i64) {
    match result {
        Err(RestorePageError::Refused {
            code,
            current_version,
        }) => (code, current_version),
        other => panic!("expected a typed restore refusal, got {other:?}"),
    }
}

const ID: &str = "page-restore";
const BODY: &str = "\n\n    indented code\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n```text\na | b\n```\nHard break  \n[[Existing link]]\n\n";

async fn fixture() -> (MemoryDB, tempfile::TempDir) {
    let (mut db, dir) = test_db().await;
    db.embedder = std::sync::OnceLock::new();
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(
            "INSERT INTO spaces (id,name,created_at,updated_at)
             VALUES ('restore-work','work',1,1),('restore-other','other',1,1);
             INSERT INTO pages
                 (id,title,summary,content,space,workspace,source_memory_ids,version,status,
                  kind,creation_kind,created_at,last_compiled,last_modified,user_edited)
             VALUES ('page-restore','Original','Stable summary','','work','work','[\"memory-source\"]',
                     7,'archived','concept','distilled','2026-01-01','2026-01-02','2026-01-03',1);",
        ).await.unwrap();
        conn.execute(
            "UPDATE pages SET content=?1,embedding=vector32(?2) WHERE id=?3",
            libsql::params![BODY, MemoryDB::vec_to_sql(&vec![0.125; 768]), ID],
        )
        .await
        .unwrap();
        MemoryDB::append_page_history(&conn, ID, "archive", 1)
            .await
            .unwrap();
    }
    (db, dir)
}

async fn integer(db: &MemoryDB, sql: &str) -> i64 {
    db.conn
        .lock()
        .await
        .query(sql, ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap()
}

async fn physical_identity(db: &MemoryDB) -> (i64, Vec<u8>) {
    let conn = db.conn.lock().await;
    let row = conn
        .query(
            "SELECT rowid,embedding FROM pages WHERE id='page-restore'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap();
    (row.get(0).unwrap(), row.get(1).unwrap())
}

async fn snapshot(db: &MemoryDB) -> serde_json::Value {
    serde_json::to_value(db.get_page_browse(ID).await.unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn restore_preserves_original_document_identity_scope_embedding_and_history() {
    let (db, _dir) = fixture().await;
    let before = db.get_page(ID).await.unwrap().unwrap();
    let physical = physical_identity(&db).await;
    let history = db.list_page_history(ID, 20).await.unwrap();
    let result = db
        .restore_page(ID, None, 7, &ReadScope::Space("work".into()))
        .await
        .unwrap();
    let after = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(
        (
            result.id.as_str(),
            result.title.as_str(),
            result.status.as_str(),
            result.version
        ),
        (ID, "Original", "active", 8)
    );
    assert_eq!(after.content, BODY);
    assert_eq!(after.source_memory_ids, before.source_memory_ids);
    assert_eq!(after.summary, before.summary);
    assert_eq!(after.space, before.space);
    assert_eq!(after.workspace, before.workspace);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.last_compiled, before.last_compiled);
    assert_eq!(after.user_edited, before.user_edited);
    assert_eq!(physical_identity(&db).await, physical);
    let restored_history = db.list_page_history(ID, 20).await.unwrap();
    assert_eq!(restored_history.len(), history.len() + 1);
    assert!(restored_history.contains(&history[0]));
    let restored = restored_history
        .iter()
        .find(|entry| entry.version == 8)
        .unwrap();
    assert_eq!(restored.edited_by, "restore");
    assert_eq!(restored.content, BODY);
    assert_eq!(restored.source_memory_ids, before.source_memory_ids);
}

#[tokio::test]
async fn restore_cas_and_current_version_replay_do_not_duplicate_history() {
    let (db, _dir) = fixture().await;
    let before = snapshot(&db).await;
    assert!(matches!(
        db.restore_page(ID, None, 0, &ReadScope::Global).await,
        Err(RestorePageError::Failed(WenlanError::Validation(_)))
    ));
    for version in [6, 8] {
        assert_eq!(
            refusal(db.restore_page(ID, None, version, &ReadScope::Global).await),
            (RestorePageErrorCode::PageVersionConflict, 7)
        );
        assert_eq!(snapshot(&db).await, before);
    }
    db.restore_page(ID, None, 7, &ReadScope::Global)
        .await
        .unwrap();
    let restored = snapshot(&db).await;
    let history = db.list_page_history(ID, 20).await.unwrap();
    assert_eq!(
        refusal(db.restore_page(ID, None, 7, &ReadScope::Global).await),
        (RestorePageErrorCode::PageVersionConflict, 8)
    );
    // Already active under its title: a new title is not a restore.
    assert_eq!(
        refusal(
            db.restore_page(ID, Some("Different"), 8, &ReadScope::Global)
                .await
        ),
        (RestorePageErrorCode::PageNotArchived, 8)
    );
    assert_eq!(
        db.restore_page(ID, None, 8, &ReadScope::Global)
            .await
            .unwrap()
            .version,
        8
    );
    assert_eq!(snapshot(&db).await, restored);
    assert_eq!(db.list_page_history(ID, 20).await.unwrap(), history);
    // A stale successful request must not restore a later archival cycle.
    db.archive_page(ID).await.unwrap();
    assert_eq!(
        refusal(db.restore_page(ID, None, 8, &ReadScope::Global).await),
        (RestorePageErrorCode::PageVersionConflict, 9)
    );
    assert_eq!(db.get_page(ID).await.unwrap().unwrap().status, "archived");
}

#[tokio::test]
async fn restore_unicode_title_conflict_is_private_and_an_alternative_title_resolves_it() {
    let (db, _dir) = fixture().await;
    db.conn
        .lock()
        .await
        .execute_batch(
            "UPDATE pages SET title='ÄBC' WHERE id='page-restore';
         INSERT INTO pages (id,title,content,space,workspace,version,status,kind,creation_kind,
                            created_at,last_compiled,last_modified)
         VALUES ('private-conflict-id','äbc','secret other body','work','other',1,'active',
                 'concept','distilled','2026-01-01','2026-01-01','2026-01-01');",
        )
        .await
        .unwrap();
    let before = snapshot(&db).await;
    let error = db
        .restore_page(ID, None, 7, &ReadScope::Space("work".into()))
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        RestorePageError::Refused {
            code: RestorePageErrorCode::PageTitleTaken,
            ..
        }
    ));
    let message = error.to_string();
    assert!(!message.contains("private-conflict-id"));
    assert!(!message.contains("äbc"));
    assert!(!message.contains("secret"));
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(db.list_page_history(ID, 20).await.unwrap().len(), 1);
    let result = db
        .restore_page(ID, Some("  Recovered title  "), 7, &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!(result.title, "Recovered title");
    assert_eq!(result.version, 8);
    assert_eq!(db.get_page(ID).await.unwrap().unwrap().content, BODY);
    assert_eq!(
        integer(
            &db,
            "SELECT version FROM pages WHERE id='private-conflict-id'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn restore_title_uniqueness_is_per_space_and_includes_unfiled() {
    let (db, _dir) = fixture().await;
    db.conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO pages (id,title,content,space,workspace,version,status,kind,creation_kind,
                            created_at,last_compiled,last_modified)
         VALUES ('other-space','Original','other body','other','other',1,'active',
                 'concept','distilled','2026-01-01','2026-01-01','2026-01-01');",
        )
        .await
        .unwrap();
    db.restore_page(ID, None, 7, &ReadScope::Space("work".into()))
        .await
        .unwrap();
    db.archive_page(ID).await.unwrap();
    db.conn.lock().await.execute_batch(
        "UPDATE pages SET space='00000000-0000-4000-8000-000000000001',workspace='00000000-0000-4000-8000-000000000001'
             WHERE id IN ('page-restore','other-space');",
    ).await.unwrap();
    assert_eq!(
        refusal(
            db.restore_page(ID, None, 9, &ReadScope::Uncategorized)
                .await
        ),
        (RestorePageErrorCode::PageTitleTaken, 9)
    );
    db.restore_page(ID, Some("Recovered unfiled"), 9, &ReadScope::Uncategorized)
        .await
        .unwrap();
    assert_eq!(db.get_page(ID).await.unwrap().unwrap().space, None);
}

#[tokio::test]
async fn restore_rejects_wrong_workspace_missing_entity_and_draft_without_writes() {
    let (db, _dir) = fixture().await;
    let before = snapshot(&db).await;
    for (id, scope) in [
        (ID, ReadScope::Space("other".into())),
        ("missing", ReadScope::Global),
    ] {
        assert!(matches!(
            db.restore_page(id, None, 7, &scope).await,
            Err(RestorePageError::Failed(WenlanError::NotFound(_)))
        ));
    }
    assert_eq!(snapshot(&db).await, before);
    for sql in [
        "UPDATE pages SET kind='entity',creation_kind='entity',status='archived' WHERE id='page-restore'",
        "UPDATE pages SET kind='concept',creation_kind='entity',status='archived' WHERE id='page-restore'",
        "UPDATE pages SET kind='concept',creation_kind='authored',status='draft' WHERE id='page-restore'",
    ] {
        db.conn.lock().await.execute(sql, ()).await.unwrap();
        let before = snapshot(&db).await;
        // The scoped read already hides some of these rows (a missing page);
        // the rest are refused as not restorable. Either way nothing is written.
        let result = db.restore_page(ID, None, 7, &ReadScope::Global).await;
        assert!(
            matches!(
                result,
                Err(RestorePageError::Failed(WenlanError::NotFound(_)))
                    | Err(RestorePageError::Refused {
                        code: RestorePageErrorCode::PageNotRestorable,
                        current_version: 7,
                    })
            ),
            "{sql}: {result:?}"
        );
        assert_eq!(snapshot(&db).await, before);
        assert_eq!(db.list_page_history(ID, 20).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn restore_source_and_imported_notes_keep_source_owned_titles() {
    for creation_kind in ["source", "imported"] {
        let (db, _dir) = fixture().await;
        db.conn
            .lock()
            .await
            .execute(
                "UPDATE pages SET creation_kind=?1,kind='source' WHERE id=?2",
                libsql::params![creation_kind, ID],
            )
            .await
            .unwrap();
        let before = snapshot(&db).await;
        assert_eq!(
            refusal(
                db.restore_page(ID, Some("New title"), 7, &ReadScope::Global)
                    .await
            ),
            (RestorePageErrorCode::PageTitleFixed, 7)
        );
        assert_eq!(snapshot(&db).await, before);
        let restored = db
            .restore_page(ID, None, 7, &ReadScope::Global)
            .await
            .unwrap();
        assert_eq!(restored.title, "Original");
        assert_eq!(db.get_page(ID).await.unwrap().unwrap().content, BODY);
    }
}

#[tokio::test]
async fn restore_refuses_preparing_and_unreadable_cutover_fences_atomically() {
    let (db, _dir) = fixture().await;
    let before = snapshot(&db).await;
    let history = db.list_page_history(ID, 20).await.unwrap();
    for fence in ["5:preparing", "corrupt-fence"] {
        db.set_app_metadata("truth_cutover_fence", fence)
            .await
            .unwrap();
        assert!(db
            .restore_page(ID, Some("Must not change"), 7, &ReadScope::Global)
            .await
            .is_err());
        assert_eq!(snapshot(&db).await, before);
        assert_eq!(db.list_page_history(ID, 20).await.unwrap(), history);
        assert_eq!(integer(&db, "SELECT COUNT(*) FROM claim_derivation_jobs WHERE page_id='page-restore' AND page_version=8").await, 0);
    }
    db.set_app_metadata("truth_cutover_fence", "6:committed")
        .await
        .unwrap();
    assert_eq!(
        db.restore_page(ID, None, 7, &ReadScope::Global)
            .await
            .unwrap()
            .version,
        8
    );
}

#[tokio::test]
async fn restore_history_failure_rolls_back_metadata_and_derivation_job() {
    let (db, _dir) = fixture().await;
    let before = snapshot(&db).await;
    let physical = physical_identity(&db).await;
    let history = db.list_page_history(ID, 20).await.unwrap();
    db.conn
        .lock()
        .await
        .execute_batch(
            "CREATE TRIGGER restore_test_history_failure BEFORE INSERT ON page_history
         WHEN NEW.page_id='page-restore' AND NEW.edited_by='restore'
         BEGIN SELECT RAISE(ABORT,'injected restore history failure'); END;",
        )
        .await
        .unwrap();
    assert!(db
        .restore_page(ID, Some("Will roll back"), 7, &ReadScope::Global)
        .await
        .is_err());
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(physical_identity(&db).await, physical);
    assert_eq!(db.list_page_history(ID, 20).await.unwrap(), history);
    assert_eq!(integer(&db, "SELECT COUNT(*) FROM claim_derivation_jobs WHERE page_id='page-restore' AND page_version=8").await, 0);
    db.conn
        .lock()
        .await
        .execute("DROP TRIGGER restore_test_history_failure", ())
        .await
        .unwrap();
    assert_eq!(
        db.restore_page(ID, None, 7, &ReadScope::Global)
            .await
            .unwrap()
            .version,
        8
    );
}

#[tokio::test]
async fn restore_requeues_new_version_without_copying_claims_markers_or_human_approval() {
    let (db, _dir) = fixture().await;
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE pages SET status='active' WHERE id='page-restore'",
            (),
        )
        .await
        .unwrap();
    publish_supported_page_for_rename_test(&db, ID, 7).await;
    let old_claims = integer(
        &db,
        "SELECT COUNT(*) FROM page_version_claims WHERE page_id='page-restore' AND page_version=7",
    )
    .await;
    assert!(old_claims > 0, "the old-version substrate must be real");
    db.conn.lock().await.execute(
        "UPDATE page_truth_state SET human_reviewed=1,reviewed_page_version=7,reviewed_page_digest=?1 WHERE page_id=?2",
        libsql::params![crate::provenance::revision_content_digest(BODY), ID],
    ).await.unwrap();
    db.archive_page(ID).await.unwrap();
    db.restore_page(ID, None, 8, &ReadScope::Global)
        .await
        .unwrap();
    let conn = db.conn.lock().await;
    let job = conn.query("SELECT status FROM claim_derivation_jobs WHERE page_id='page-restore' AND page_version=9", ())
        .await.unwrap().next().await.unwrap().unwrap();
    assert_eq!(job.get::<String>(0).unwrap(), "pending");
    let truth = conn.query("SELECT support_status,reviewed_page_version FROM page_truth_state WHERE page_id='page-restore'", ())
        .await.unwrap().next().await.unwrap().unwrap();
    assert_eq!(truth.get::<String>(0).unwrap(), "provisional");
    assert_eq!(truth.get::<Option<i64>>(1).unwrap(), Some(7));
    drop(conn);
    assert_eq!(integer(&db, "SELECT COUNT(*) FROM page_version_claims WHERE page_id='page-restore' AND page_version=9").await, 0);
    assert_eq!(integer(&db, "SELECT COUNT(*) FROM claim_derivation_markers WHERE page_id='page-restore' AND page_version=9").await, 0);
    assert_eq!(integer(&db, "SELECT COUNT(*) FROM page_version_claims WHERE page_id='page-restore' AND page_version=7").await, old_claims);
}
#[tokio::test]
async fn concurrent_restore_requests_have_one_version_winner() {
    let (db, _dir) = fixture().await;
    let scope = ReadScope::Global;
    let (first, second) = tokio::join!(
        db.restore_page(ID, Some("First"), 7, &scope),
        db.restore_page(ID, Some("Second"), 7, &scope),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = if let Err(error) = first {
        Err::<(), _>(error)
    } else {
        second.map(|_| ())
    };
    assert_eq!(
        refusal(failure),
        (RestorePageErrorCode::PageVersionConflict, 8)
    );
    let page = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(page.version, 8);
    assert_eq!(page.content, BODY);
    assert_eq!(db.list_page_history(ID, 20).await.unwrap().len(), 2);
}
