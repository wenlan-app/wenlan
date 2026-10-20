// SPDX-License-Identifier: Apache-2.0

use super::{
    repair_page_rename::{
        recover_rename_page_title_apply_receipt, rename_page_title_cas_inner,
        with_rename_page_title_test_checkpoints, RenamePageTitleTestCheckpoint,
        RenamePageTitleTestCheckpoints,
    },
    MemoryDB,
};
use crate::{
    error::WenlanError,
    export::knowledge::KnowledgeProjectionWrite,
    lint::{
        context::{CancellationToken, LintClock},
        runner::LintRunner,
    },
    post_write::RepairWriteProof,
    repair::{prepare_memory_reclassification_with_pages, repair_digest, RepairArtifactStore},
};
use std::{
    future::{poll_fn, Future},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
    time::{Duration, Instant},
};
use tokio::{sync::Notify, task::JoinHandle, time::timeout};
use wenlan_types::{
    lint::{LintProfile, LintQuery},
    repair::{
        PrepareRepairRequest, RepairApplyReceipt, RepairApplyReceiptDraft, RepairChoice,
        RepairDigest, RepairLintScope, RepairManifest, RepairMutation, RepairRollbackPayloadV2,
        StoredRepairRollbackArtifact,
    },
    RefinementPayload,
};

const OCCURRENCE: &str = "edededededededededededededededededededededededededededededededed";
const PENDING_FILE: &str = ".apply-receipt.json.pending";
const FINAL_FILE: &str = "apply-receipt.json";

struct RenameFixture {
    db: Arc<MemoryDB>,
    _db_dir: tempfile::TempDir,
    page_root: tempfile::TempDir,
    _repair_root: tempfile::TempDir,
    store: RepairArtifactStore,
    manifest: RepairManifest,
    rollback: RepairRollbackPayloadV2,
}

async fn rename_fixture() -> RenameFixture {
    let (db, db_dir) = super::tests::test_db().await;
    db.conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO spaces (id,name,created_at,updated_at)
             VALUES ('space-work','work',1,1);
             INSERT INTO pages
                 (id,title,summary,content,space,source_memory_ids,version,status,embedding,
                  created_at,last_compiled,last_modified,workspace,creation_kind,review_status)
             VALUES
                 ('page-a','Origin','A summary','A body','work','[]',4,'active',
                  NULL,'2026-07-17T00:00:00Z','2026-07-17T00:00:00Z',
                  '2026-07-17T00:00:00Z','work','distilled','confirmed'),
                 ('page-b','origin','B summary','B body','work','[]',2,'active',
                  NULL,'2026-07-17T00:00:00Z','2026-07-17T00:00:00Z',
                  '2026-07-17T00:00:00Z','work','distilled','confirmed');",
        )
        .await
        .unwrap();
    let db = Arc::new(db);
    let page_root = tempfile::tempdir().unwrap();
    for page_id in ["page-a", "page-b"] {
        let page = db.get_page(page_id).await.unwrap().unwrap();
        KnowledgeProjectionWrite::new(page_root.path().to_path_buf(), &db)
            .write_page(&page)
            .unwrap();
    }

    let occurrence = RepairDigest::parse(OCCURRENCE).unwrap();
    let review_id = format!("lint_review_{OCCURRENCE}");
    let source_ids = vec!["page-a".to_string()];
    let payload = RefinementPayload::LintRepairReview {
        check_id: "pages.duplicate_active_titles".to_string(),
        occurrence_digest: occurrence.clone(),
        owner_binding_digest: crate::repair::lint_review_owner_binding_digest(
            &occurrence,
            &source_ids,
        )
        .unwrap(),
        issue: "Rename the narrower duplicate Page.".to_string(),
        choices: vec!["Origin Git Workflow Gotchas".to_string()],
        suggested_research_queries: vec![],
    };
    db.insert_lint_review_if_absent(
        &review_id,
        &source_ids,
        &serde_json::to_string(&payload).unwrap(),
    )
    .await
    .unwrap();

    let general = LintRunner::new(LintClock::fixed(), CancellationToken::new())
        .run(
            &db,
            &LintQuery::new(Some(LintProfile::General), None),
            Some(page_root.path()),
            true,
        )
        .await
        .unwrap();
    let request = PrepareRepairRequest::try_new_with_choice(
        RepairLintScope::global(),
        general,
        None,
        RepairChoice::rename_page_title(
            review_id,
            "page-a".to_string(),
            "Origin".to_string(),
            "Origin Git Workflow Gotchas".to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    let repair_root = tempfile::tempdir().unwrap();
    let store = RepairArtifactStore::new(repair_root.path().to_path_buf());
    let manifest = prepare_memory_reclassification_with_pages(
        &db,
        &store,
        request,
        Some(page_root.path()),
        1_721_000_001,
    )
    .await
    .unwrap();
    let rollback_path = store
        .manifest_dir(manifest.manifest_id())
        .unwrap()
        .join(manifest.rollback().relative_path());
    let StoredRepairRollbackArtifact::V2(rollback) =
        StoredRepairRollbackArtifact::from_slice(&std::fs::read(rollback_path).unwrap()).unwrap()
    else {
        panic!("expected v2 rollback")
    };

    RenameFixture {
        db,
        _db_dir: db_dir,
        page_root,
        _repair_root: repair_root,
        store,
        manifest,
        rollback: rollback.payload().clone(),
    }
}

fn artifact_paths(fixture: &RenameFixture) -> (PathBuf, PathBuf) {
    let dir = fixture
        .store
        .manifest_dir(fixture.manifest.manifest_id())
        .unwrap();
    (dir.join(PENDING_FILE), dir.join(FINAL_FILE))
}

fn create_empty_pending(fixture: &RenameFixture) {
    let (pending, _) = artifact_paths(fixture);
    std::fs::write(pending, []).unwrap();
}

fn target_filename(root: &Path, page_id: &str) -> String {
    serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(root.join(".wenlan/state.json")).unwrap(),
    )
    .unwrap()["pages"][page_id]["file"]
        .as_str()
        .unwrap()
        .to_string()
}

fn receipt_from_proof(manifest: &RepairManifest, proof: &RepairWriteProof) -> RepairApplyReceipt {
    let draft = RepairApplyReceiptDraft::try_new(
        manifest.manifest_id().to_string(),
        manifest.manifest_digest().clone(),
        1_721_000_002,
        proof.before_target_receipt().clone(),
        proof.after_target_receipt().clone(),
        proof.non_target_before().clone(),
        proof.non_target_after().clone(),
        proof.post_apply_db_digest().clone(),
        manifest.allowed_effects().clone(),
        manifest.writer(),
    )
    .unwrap();
    RepairApplyReceipt::from_draft(
        draft.clone(),
        repair_digest(&draft.canonical_bytes().unwrap()),
    )
}

async fn rename_page_title_cas_with_checkpoints<F>(
    fixture: &RenameFixture,
    page_root: &Path,
    checkpoints: RenamePageTitleTestCheckpoints,
    before_commit: F,
) -> Result<RepairWriteProof, WenlanError>
where
    F: FnOnce(&RepairWriteProof) -> Result<(), WenlanError>,
{
    with_rename_page_title_test_checkpoints(
        checkpoints,
        rename_page_title_cas_inner(
            &fixture.db,
            &fixture.manifest,
            &fixture.rollback,
            page_root,
            before_commit,
        ),
    )
    .await
}

async fn recover_rename_page_title_with_checkpoints(
    fixture: &RenameFixture,
    page_root: &Path,
    checkpoints: RenamePageTitleTestCheckpoints,
) -> Result<Option<RepairApplyReceipt>, WenlanError> {
    with_rename_page_title_test_checkpoints(
        checkpoints,
        recover_rename_page_title_apply_receipt(
            &fixture.db,
            &fixture.store,
            &fixture.manifest,
            page_root,
        ),
    )
    .await
}

async fn write_post_projection(fixture: &RenameFixture) -> (Vec<u8>, Vec<u8>) {
    let mut page = fixture.db.get_page("page-a").await.unwrap().unwrap();
    let RepairMutation::RenamePageTitle { after_title, .. } = fixture.manifest.mutation() else {
        panic!("expected rename mutation")
    };
    page.title = after_title.clone();
    page.version = page.version.saturating_add(1);
    KnowledgeProjectionWrite::new(fixture.page_root.path().to_path_buf(), &fixture.db)
        .write_page(&page)
        .unwrap();
    let filename = target_filename(fixture.page_root.path(), "page-a");
    (
        std::fs::read(fixture.page_root.path().join(filename)).unwrap(),
        std::fs::read(fixture.page_root.path().join(".wenlan/state.json")).unwrap(),
    )
}

struct DbContender {
    start: Arc<Notify>,
    pending: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl DbContender {
    fn prestarted(db: Arc<MemoryDB>) -> Self {
        let start = Arc::new(Notify::new());
        let pending = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        let task_start = Arc::clone(&start);
        let task_pending = Arc::clone(&pending);
        let task_entered = Arc::clone(&entered);
        let task = tokio::spawn(async move {
            task_start.notified().await;
            let mut lock = Box::pin(db.conn.lock());
            let guard = poll_fn(|cx| match Future::poll(Pin::as_mut(&mut lock), cx) {
                Poll::Pending => {
                    task_pending.store(true, Ordering::SeqCst);
                    Poll::Pending
                }
                Poll::Ready(guard) => Poll::Ready(guard),
            })
            .await;
            task_entered.store(true, Ordering::SeqCst);
            drop(guard);
        });
        Self {
            start,
            pending,
            entered,
            task,
        }
    }

    fn release_checkpoint(&self) -> RenamePageTitleTestCheckpoint {
        let start = Arc::clone(&self.start);
        let pending = Arc::clone(&self.pending);
        let entered = Arc::clone(&self.entered);
        Box::new(move || {
            start.notify_one();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !pending.load(Ordering::SeqCst) {
                if Instant::now() >= deadline {
                    return Err(WenlanError::Conflict(
                        "db contender did not observe Pending".to_string(),
                    ));
                }
                std::thread::yield_now();
            }
            if entered.load(Ordering::SeqCst) {
                return Err(WenlanError::Conflict(
                    "db contender entered while rename transaction was active".to_string(),
                ));
            }
            Ok(())
        })
    }

    async fn assert_enters(self) {
        timeout(Duration::from_secs(2), self.task)
            .await
            .expect("DB contender must enter after the rename transaction returns")
            .unwrap();
        assert!(self.entered.load(Ordering::SeqCst));
    }
}

fn assert_projection_and_db_still_locked(
    db: &MemoryDB,
    page_root: &Path,
) -> Result<(), WenlanError> {
    if db.conn.try_lock().is_ok() {
        return Err(WenlanError::Conflict(
            "DB mutex released before rename recovery artifact action".to_string(),
        ));
    }
    match KnowledgeProjectionWrite::begin_owned_repair_session(page_root.to_path_buf(), db) {
        Err(WenlanError::Conflict(message)) if message == "page_projection_locked" => Ok(()),
        Err(error) => Err(WenlanError::Conflict(format!(
            "unexpected projection lock error: {error}"
        ))),
        Ok(_) => Err(WenlanError::Conflict(
            "projection lock released before rename recovery artifact action".to_string(),
        )),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn projection_session_precedes_db_mutex_for_apply_and_pending_recovery() {
    let fixture = rename_fixture().await;
    let invalid_root = tempfile::NamedTempFile::new().unwrap();
    let db_guard = fixture.db.conn.lock().await;

    let apply = timeout(
        Duration::from_millis(500),
        rename_page_title_cas_inner(
            &fixture.db,
            &fixture.manifest,
            &fixture.rollback,
            invalid_root.path(),
            |_| Ok(()),
        ),
    )
    .await
    .expect("projection validation must run before waiting for DB");
    assert!(matches!(
        apply,
        Err(WenlanError::Validation(message)) if message == "repair_projection_root_invalid"
    ));

    create_empty_pending(&fixture);
    let recovery = timeout(
        Duration::from_millis(500),
        recover_rename_page_title_apply_receipt(
            &fixture.db,
            &fixture.store,
            &fixture.manifest,
            invalid_root.path(),
        ),
    )
    .await
    .expect("recovery projection validation must run before waiting for DB");
    assert!(matches!(
        recovery,
        Err(WenlanError::Validation(message)) if message == "repair_projection_root_invalid"
    ));
    drop(db_guard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn apply_keeps_the_same_db_contender_pending_across_projection_write() {
    let fixture = rename_fixture().await;
    let contender = DbContender::prestarted(Arc::clone(&fixture.db));

    let result = rename_page_title_cas_with_checkpoints(
        &fixture,
        fixture.page_root.path(),
        RenamePageTitleTestCheckpoints {
            before_projection_write: Some(contender.release_checkpoint()),
            after_target_write: Some(Box::new({
                let entered = Arc::clone(&contender.entered);
                move || {
                    if entered.load(Ordering::SeqCst) {
                        return Err(WenlanError::Conflict(
                            "db contender entered before rename transaction returned".to_string(),
                        ));
                    }
                    Err(WenlanError::Conflict(
                        "forced after target write".to_string(),
                    ))
                }
            })),
            ..Default::default()
        },
        |_| Ok(()),
    )
    .await;

    assert!(matches!(
        result,
        Err(WenlanError::Conflict(message)) if message == "forced after target write"
    ));
    contender.assert_enters().await;
    let page = fixture.db.get_page("page-a").await.unwrap().unwrap();
    assert_eq!(page.title, "Origin");
    assert_eq!(page.version, 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn apply_compensation_holds_db_mutex_from_restore_entry_through_midpoint() {
    let fixture = rename_fixture().await;
    let contender = DbContender::prestarted(Arc::clone(&fixture.db));
    let filename = target_filename(fixture.page_root.path(), "page-a");
    let target_before = std::fs::read(fixture.page_root.path().join(&filename)).unwrap();

    let result = rename_page_title_cas_with_checkpoints(
        &fixture,
        fixture.page_root.path(),
        RenamePageTitleTestCheckpoints {
            after_target_write: Some(Box::new(|| {
                Err(WenlanError::Conflict(
                    "force apply compensation".to_string(),
                ))
            })),
            before_projection_restore: Some(contender.release_checkpoint()),
            after_target_restore: Some(Box::new({
                let entered = Arc::clone(&contender.entered);
                let root = fixture.page_root.path().to_path_buf();
                let filename = filename.clone();
                move || {
                    if entered.load(Ordering::SeqCst) {
                        return Err(WenlanError::Conflict(
                            "db contender entered before rename transaction returned".to_string(),
                        ));
                    }
                    assert_eq!(std::fs::read(root.join(&filename)).unwrap(), target_before);
                    Ok(())
                }
            })),
            ..Default::default()
        },
        |_| Ok(()),
    )
    .await;

    assert!(matches!(
        result,
        Err(WenlanError::Conflict(message)) if message == "force apply compensation"
    ));
    contender.assert_enters().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn recovery_restore_holds_db_mutex_from_entry_through_midpoint() {
    let fixture = rename_fixture().await;
    let filename = target_filename(fixture.page_root.path(), "page-a");
    let target_before = std::fs::read(fixture.page_root.path().join(&filename)).unwrap();
    create_empty_pending(&fixture);
    write_post_projection(&fixture).await;
    let contender = DbContender::prestarted(Arc::clone(&fixture.db));

    let result = recover_rename_page_title_with_checkpoints(
        &fixture,
        fixture.page_root.path(),
        RenamePageTitleTestCheckpoints {
            before_projection_restore: Some(contender.release_checkpoint()),
            after_target_restore: Some(Box::new({
                let entered = Arc::clone(&contender.entered);
                let root = fixture.page_root.path().to_path_buf();
                let filename = filename.clone();
                move || {
                    if entered.load(Ordering::SeqCst) {
                        return Err(WenlanError::Conflict(
                            "db contender entered before rename transaction returned".to_string(),
                        ));
                    }
                    assert_eq!(std::fs::read(root.join(&filename)).unwrap(), target_before);
                    Ok(())
                }
            })),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(result.is_none());
    contender.assert_enters().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn recovery_publish_runs_after_commit_while_db_and_projection_stay_locked() {
    let fixture = rename_fixture().await;
    let (pending, final_path) = artifact_paths(&fixture);
    rename_page_title_cas_inner(
        &fixture.db,
        &fixture.manifest,
        &fixture.rollback,
        fixture.page_root.path(),
        |proof| {
            std::fs::write(
                &pending,
                serde_json::to_vec_pretty(&receipt_from_proof(&fixture.manifest, proof))?,
            )?;
            Ok(())
        },
    )
    .await
    .unwrap();

    let recovered = recover_rename_page_title_with_checkpoints(
        &fixture,
        fixture.page_root.path(),
        RenamePageTitleTestCheckpoints {
            after_commit_before_artifact: Some(Box::new({
                let pending = pending.clone();
                let final_path = final_path.clone();
                let db = Arc::clone(&fixture.db);
                let page_root = fixture.page_root.path().to_path_buf();
                move || {
                    assert!(pending.is_file());
                    assert!(!final_path.exists());
                    assert_projection_and_db_still_locked(&db, &page_root)
                }
            })),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(recovered.is_some());
    assert!(!pending.exists());
    assert!(final_path.is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn recovery_clear_runs_after_commit_while_db_and_projection_stay_locked() {
    let fixture = rename_fixture().await;
    let (pending, final_path) = artifact_paths(&fixture);
    create_empty_pending(&fixture);

    let recovered = recover_rename_page_title_with_checkpoints(
        &fixture,
        fixture.page_root.path(),
        RenamePageTitleTestCheckpoints {
            after_commit_before_artifact: Some(Box::new({
                let pending = pending.clone();
                let final_path = final_path.clone();
                let db = Arc::clone(&fixture.db);
                let page_root = fixture.page_root.path().to_path_buf();
                move || {
                    assert!(pending.is_file());
                    assert!(!final_path.exists());
                    assert_projection_and_db_still_locked(&db, &page_root)
                }
            })),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(recovered.is_none());
    assert!(!pending.exists());
    assert!(!final_path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn split_projection_states_fail_closed_and_retain_pending() {
    for target_post in [true, false] {
        let fixture = rename_fixture().await;
        let filename = target_filename(fixture.page_root.path(), "page-a");
        let target_pre = std::fs::read(fixture.page_root.path().join(&filename)).unwrap();
        let state_pre = std::fs::read(fixture.page_root.path().join(".wenlan/state.json")).unwrap();
        let (target_post_bytes, state_post_bytes) = write_post_projection(&fixture).await;
        std::fs::write(
            fixture.page_root.path().join(&filename),
            if target_post {
                &target_post_bytes
            } else {
                &target_pre
            },
        )
        .unwrap();
        std::fs::write(
            fixture.page_root.path().join(".wenlan/state.json"),
            if target_post {
                &state_pre
            } else {
                &state_post_bytes
            },
        )
        .unwrap();
        create_empty_pending(&fixture);
        let (pending, _) = artifact_paths(&fixture);

        let error = recover_rename_page_title_apply_receipt(
            &fixture.db,
            &fixture.store,
            &fixture.manifest,
            fixture.page_root.path(),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            WenlanError::Conflict(message) if message == "repair_apply_recovery_required"
        ));
        assert!(pending.is_file());
    }
}

#[tokio::test]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn interrupted_rename_recovery_accepts_legacy_post_state_without_projection_hash() {
    let fixture = rename_fixture().await;
    let filename = target_filename(fixture.page_root.path(), "page-a");
    let target_path = fixture.page_root.path().join(&filename);
    let state_path = fixture.page_root.path().join(".wenlan/state.json");
    let target_before = std::fs::read(&target_path).unwrap();
    let state_before = std::fs::read(&state_path).unwrap();
    let page_before = fixture.db.get_page("page-a").await.unwrap().unwrap();
    create_empty_pending(&fixture);
    let (target_post, state_post) = write_post_projection(&fixture).await;
    assert_ne!(
        target_post, target_before,
        "positive interrupted-write control"
    );

    // Model an older binary that wrote the renamed Markdown and state, then
    // stopped before committing the database rename.
    let mut legacy_state: serde_json::Value = serde_json::from_slice(&state_post).unwrap();
    assert!(legacy_state["pages"]["page-a"]
        .as_object_mut()
        .unwrap()
        .remove("content_sha256")
        .is_some());
    std::fs::write(
        &state_path,
        serde_json::to_vec_pretty(&legacy_state).unwrap(),
    )
    .unwrap();

    let recovered = recover_rename_page_title_apply_receipt(
        &fixture.db,
        &fixture.store,
        &fixture.manifest,
        fixture.page_root.path(),
    )
    .await
    .expect("verified legacy Markdown permits restore and retry after upgrade");

    assert!(
        recovered.is_none(),
        "the interrupted DB write must be retried"
    );
    assert_eq!(std::fs::read(&target_path).unwrap(), target_before);
    assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
    let page_after = fixture.db.get_page("page-a").await.unwrap().unwrap();
    assert_eq!(page_after.title, page_before.title);
    assert_eq!(page_after.version, page_before.version);
    let (pending, final_path) = artifact_paths(&fixture);
    assert!(
        !pending.exists(),
        "successful recovery clears the interrupted marker"
    );
    assert!(
        !final_path.exists(),
        "an uncommitted rename has no success receipt"
    );
}

/// A note that relies on the renamed note keeps its snapshot of the title in
/// step, in the same repair transaction, and still reads `ok`: a title change
/// is not a change to the words it relied on.
#[tokio::test]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn a_repair_rename_re_evaluates_the_notes_that_rely_on_the_renamed_note() {
    let fixture = rename_fixture().await;
    // `page-b` relies on `page-a` at its current version.
    fixture
        .db
        .mark_page_basis("page-b", "page-a", 4, "human")
        .await
        .unwrap();
    let before = fixture.db.list_page_basis("page-b").await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].title, "Origin");
    assert_eq!(before[0].current_version, Some(4));
    assert_eq!(before[0].state, "ok");

    rename_page_title_cas_inner(
        &fixture.db,
        &fixture.manifest,
        &fixture.rollback,
        fixture.page_root.path(),
        |_| Ok(()),
    )
    .await
    .expect("the rename applies");

    let renamed = fixture.db.get_page("page-a").await.unwrap().unwrap();
    assert_eq!(renamed.title, "Origin Git Workflow Gotchas");
    assert_eq!(renamed.version, 5);
    let after = fixture.db.list_page_basis("page-b").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].title, "Origin Git Workflow Gotchas",
        "the citer's snapshot of the basis title follows the rename"
    );
    assert_eq!(
        after[0].current_version,
        Some(5),
        "the re-evaluation saw the bumped version"
    );
    assert_eq!(
        after[0].state, "ok",
        "same words under a new title is not a change to rely on"
    );
    assert_eq!(after[0].confirmed_version, 4);

    // The live join would show the new title whatever was stored, so look at
    // the stored snapshot itself, and at the one place it is the only source:
    // once the basis is deleted the citer keeps naming it by its last title.
    let stored: String = {
        let conn = fixture.db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT basis_title FROM page_basis \
                 WHERE page_id = 'page-b' AND basis_page_id = 'page-a'",
                (),
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap()
    };
    assert_eq!(stored, "Origin Git Workflow Gotchas");
    fixture.db.delete_page("page-a").await.unwrap();
    let gone = fixture.db.list_page_basis("page-b").await.unwrap();
    assert_eq!(gone[0].state, "deleted");
    assert_eq!(gone[0].title, "Origin Git Workflow Gotchas");
}

#[tokio::test]
#[cfg_attr(not(unix), ignore = "repair artifacts are unix-only")]
async fn interrupted_rename_recovery_refuses_wrong_hash_changed_bytes_and_other_state() {
    for case in ["wrong_hash", "changed_bytes", "other_state"] {
        let fixture = rename_fixture().await;
        let filename = target_filename(fixture.page_root.path(), "page-a");
        let target_path = fixture.page_root.path().join(&filename);
        let state_path = fixture.page_root.path().join(".wenlan/state.json");
        let page_before = fixture.db.get_page("page-a").await.unwrap().unwrap();
        create_empty_pending(&fixture);
        let (target_post, state_post) = write_post_projection(&fixture).await;
        let mut current_state: serde_json::Value = serde_json::from_slice(&state_post).unwrap();
        let page_state = current_state["pages"]["page-a"].as_object_mut().unwrap();
        match case {
            "wrong_hash" => {
                // Exact expected Markdown alone cannot excuse a present bad hash.
                page_state.insert(
                    "content_sha256".to_string(),
                    serde_json::json!("0".repeat(64)),
                );
            }
            "changed_bytes" => {
                // Missing legacy hash cannot excuse even one changed Markdown byte.
                assert!(page_state.remove("content_sha256").is_some());
                let mut changed = target_post;
                changed.push(b'!');
                std::fs::write(&target_path, changed).unwrap();
            }
            "other_state" => {
                // The legacy exception applies to the hash only.
                assert!(page_state.remove("content_sha256").is_some());
                page_state.insert(
                    "version".to_string(),
                    serde_json::json!(page_before.version + 2),
                );
            }
            _ => unreachable!(),
        }
        std::fs::write(
            &state_path,
            serde_json::to_vec_pretty(&current_state).unwrap(),
        )
        .unwrap();
        let target_before_recovery = std::fs::read(&target_path).unwrap();
        let state_before_recovery = std::fs::read(&state_path).unwrap();

        let error = recover_rename_page_title_apply_receipt(
            &fixture.db,
            &fixture.store,
            &fixture.manifest,
            fixture.page_root.path(),
        )
        .await
        .expect_err(case);

        assert!(
            matches!(error, WenlanError::Conflict(ref message) if message == "repair_apply_recovery_required"),
            "{case}: {error}"
        );
        assert_eq!(
            std::fs::read(&target_path).unwrap(),
            target_before_recovery,
            "{case}"
        );
        assert_eq!(
            std::fs::read(&state_path).unwrap(),
            state_before_recovery,
            "{case}"
        );
        let page_after = fixture.db.get_page("page-a").await.unwrap().unwrap();
        assert_eq!(page_after.title, page_before.title, "{case}");
        assert_eq!(page_after.version, page_before.version, "{case}");
        let (pending, final_path) = artifact_paths(&fixture);
        assert!(
            pending.is_file(),
            "{case}: refused recovery preserves its marker"
        );
        assert!(
            !final_path.exists(),
            "{case}: refused recovery cannot publish success"
        );
    }
}
