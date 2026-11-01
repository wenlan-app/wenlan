// SPDX-License-Identifier: Apache-2.0
//! Bulk ingest helper for chat imports.
//!
//! Takes a batch of `ParsedConversation`s and stores them as raw memories.
//! Does NOT run classification, extraction, or distillation itself — that work
//! happens via post_ingest.rs on ingest and the refinery steep that runs after.

use crate::chat_import::types::{ParsedConversation, Vendor};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::events::EventEmitter;
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Tracks the state of an in-progress (or completed) bulk chat-history import.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportState {
    pub id: String,
    pub vendor: Vendor,
    pub source_path: String,
    pub total_conversations: Option<i64>,
    pub processed_conversations: i64,
    pub stage: ImportStage,
    pub error_message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Discrete stages of a bulk import pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStage {
    Parsing,
    StageA,
    StageB,
    Done,
    Error,
}

impl ImportStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parsing => "parsing",
            Self::StageA => "stage_a",
            Self::StageB => "stage_b",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "parsing" => Some(Self::Parsing),
            "stage_a" => Some(Self::StageA),
            "stage_b" => Some(Self::StageB),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Result of a bulk import pass.
pub struct BulkImportResult {
    /// Number of conversations actually ingested (after dedup).
    pub conversations_ingested: usize,
    /// Number of conversations skipped because they already existed.
    pub conversations_skipped_existing: usize,
    /// Number of individual memories stored.
    pub memories_stored: usize,
    /// Artifacts and canvases kept as originals (files plus pages).
    pub originals_kept: usize,
    /// Conversations whose originals could not all be kept. Their chat text
    /// is stored either way; the failure is logged.
    pub originals_failed: usize,
}

/// Where an import keeps the originals (artifacts, canvases) of the chats it
/// stores. Without one, an import keeps chat text only, as it always did.
#[derive(Debug, Clone, Copy)]
pub struct OriginalsTarget<'a> {
    /// The Wenlan data root; files go under `<data_root>/originals/`.
    pub data_root: &'a std::path::Path,
    /// The pages folder, so each original's page is also written there.
    pub knowledge_path: Option<&'a std::path::Path>,
}

/// Entries a chat import accumulates before it stores them. Each stored slice
/// is one embedding pass and one transaction, so this bounds the vector buffer
/// and how long one slice holds the writer connection. A slice only ends at a
/// conversation boundary, so a conversation longer than this is stored alone,
/// as one slice, rather than split.
const RAW_IMPORT_FLUSH_ENTRIES: usize = 128;

/// Store all memories from a batch of parsed conversations, skipping any
/// conversation whose `external_id` is already in the database.
///
/// Each memory is stored with `source = 'memory'`, `source_id` set to the
/// conversation-level import key (e.g. `import_claude_{conv_external_id}`),
/// and `memory_type = NULL`. All messages in the same conversation share
/// the same `source_id`, with `chunk_index` distinguishing ordinal position.
///
/// Memories are stored with `source = 'memory'` and `memory_type = NULL`;
/// post-ingest classification runs via `post_ingest.rs` on each stored memory.
///
/// Conversations are stored and embedded in slices of whole conversations
/// that each commit on their own. An import that fails part way keeps every
/// finished slice, and a retry resumes through the per-conversation dedup.
/// The dedup reads committed rows once, before any write, so callers that can
/// overlap on the same export must serialize (the daemon's chat-export route
/// holds a process-wide lock for this).
pub async fn bulk_import_conversations(
    db: Arc<MemoryDB>,
    batch: &[ParsedConversation],
    emitter: Arc<dyn EventEmitter>,
    import_id: &str,
) -> Result<BulkImportResult, WenlanError> {
    bulk_import_conversations_with_flush(
        db,
        batch,
        emitter,
        import_id,
        RAW_IMPORT_FLUSH_ENTRIES,
        None,
    )
    .await
}

/// [`bulk_import_conversations`], also keeping each new conversation's
/// artifacts and canvases as originals (see `chat_import::originals`). The
/// originals of a slice are kept once its memories are committed, because
/// each original's page cites the conversation's memory.
pub async fn bulk_import_conversations_keeping_originals(
    db: Arc<MemoryDB>,
    batch: &[ParsedConversation],
    emitter: Arc<dyn EventEmitter>,
    import_id: &str,
    originals: OriginalsTarget<'_>,
) -> Result<BulkImportResult, WenlanError> {
    bulk_import_conversations_with_flush(
        db,
        batch,
        emitter,
        import_id,
        RAW_IMPORT_FLUSH_ENTRIES,
        Some(originals),
    )
    .await
}

/// Keep the originals of conversations whose memories just committed.
/// A failure is logged and counted; the chat text stays imported.
async fn keep_slice_originals(
    db: &MemoryDB,
    target: Option<OriginalsTarget<'_>>,
    slice: &[&ParsedConversation],
    kept: &mut usize,
    failed: &mut usize,
) {
    let Some(target) = target else {
        return;
    };
    for conv in slice.iter().filter(|c| !c.artifacts.is_empty()) {
        match crate::chat_import::originals::keep_conversation_originals(
            db,
            target.data_root,
            target.knowledge_path,
            conv,
        )
        .await
        {
            Ok(originals) => *kept += originals.len(),
            Err(error) => {
                *failed += 1;
                log::warn!(
                    "[chat_import] could not keep the originals of {} conversation {}: {error}",
                    conv.vendor.as_str(),
                    conv.external_id
                );
            }
        }
    }
}

/// [`bulk_import_conversations`] with the slice size as a parameter, so tests
/// can exercise slice boundaries with a handful of messages.
pub(crate) async fn bulk_import_conversations_with_flush(
    db: Arc<MemoryDB>,
    batch: &[ParsedConversation],
    emitter: Arc<dyn EventEmitter>,
    import_id: &str,
    flush_entries: usize,
    originals: Option<OriginalsTarget<'_>>,
) -> Result<BulkImportResult, WenlanError> {
    if batch.is_empty() {
        return Ok(BulkImportResult {
            conversations_ingested: 0,
            conversations_skipped_existing: 0,
            memories_stored: 0,
            originals_kept: 0,
            originals_failed: 0,
        });
    }

    // 1. Compute candidate source_ids for dedup.
    let candidates: Vec<String> = batch
        .iter()
        .map(|c| c.vendor.build_source_id(&c.external_id))
        .collect();
    let existing = db.check_existing_import_source_ids(&candidates).await?;

    // 2. Filter to new conversations only.
    let new_conversations: Vec<&ParsedConversation> = batch
        .iter()
        .filter(|c| {
            let sid = c.vendor.build_source_id(&c.external_id);
            !existing.contains(&sid)
        })
        .collect();

    let skipped = batch.len() - new_conversations.len();

    // 3. Store memories in slices of whole conversations, each one embedded and
    //    committed as a unit. A slice is stored before the next conversation
    //    would push it past `flush_entries`. It never splits a conversation:
    //    dedup would then skip a half-stored conversation on every retry.
    //    Emit progress events every 10 memories accumulated (conversation boundary).
    let total_estimate: usize = new_conversations.iter().map(|c| c.messages.len()).sum();
    #[allow(clippy::type_complexity)]
    let mut entries: Vec<(
        String,
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
        i64,
    )> = Vec::new();
    let mut memories_stored: usize = 0;
    let mut last_emitted_count: usize = 0;
    let mut slice: Vec<&ParsedConversation> = Vec::new();
    let mut originals_kept = 0usize;
    let mut originals_failed = 0usize;

    for conv in &new_conversations {
        if !entries.is_empty() && entries.len() + conv.messages.len() > flush_entries {
            memories_stored += db.store_raw_import_memories_batch(&entries).await?;
            entries.clear();
            keep_slice_originals(
                &db,
                originals,
                &slice,
                &mut originals_kept,
                &mut originals_failed,
            )
            .await;
            slice.clear();
        }
        slice.push(*conv);
        let source_id = conv.vendor.build_source_id(&conv.external_id);
        for (ordinal, msg) in conv.messages.iter().enumerate() {
            entries.push((
                source_id.clone(),
                msg.content.clone(),
                conv.title.clone(),
                msg.created_at,
                ordinal as i64,
            ));
        }

        // Emit progress if we crossed a multiple-of-10 boundary since last emission.
        let current_count = memories_stored + entries.len();
        if current_count / 10 > last_emitted_count / 10 {
            let payload = serde_json::json!({
                "import_id": import_id,
                "stage": "stage_a",
                "memories_processed": current_count,
                "memories_total": total_estimate,
                "entity_counts": {"people": 0, "projects": 0, "pages": 0, "decisions": 0, "tools": 0},
                "pages_written": 0,
                "pages_total": 0,
                "latest_page_titles": []
            });
            let _ = emitter.emit("chat-import-progress", &payload.to_string());
            last_emitted_count = current_count;
        }
    }

    if !entries.is_empty() {
        memories_stored += db.store_raw_import_memories_batch(&entries).await?;
    }
    keep_slice_originals(
        &db,
        originals,
        &slice,
        &mut originals_kept,
        &mut originals_failed,
    )
    .await;

    // Final progress emission — always fire so the frontend sees the
    // definitive 100% signal regardless of whether the count landed on an
    // exact multiple of 10 during the loop above.
    if total_estimate > 0 {
        let payload = serde_json::json!({
            "import_id": import_id,
            "stage": "stage_a",
            "memories_processed": memories_stored,
            "memories_total": total_estimate,
            "entity_counts": {"people": 0, "projects": 0, "pages": 0, "decisions": 0, "tools": 0},
            "pages_written": 0,
            "pages_total": 0,
            "latest_page_titles": []
        });
        let _ = emitter.emit("chat-import-progress", &payload.to_string());
    }

    Ok(BulkImportResult {
        conversations_ingested: new_conversations.len(),
        conversations_skipped_existing: skipped,
        memories_stored,
        originals_kept,
        originals_failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_import::types::{MessageRole, ParsedMessage, Vendor};
    use crate::events::NoopEmitter;

    fn make_convo(external_id: &str, contents: &[&str]) -> ParsedConversation {
        ParsedConversation {
            external_id: external_id.into(),
            vendor: Vendor::Claude,
            title: Some("Test".into()),
            created_at: None,
            summary: None,
            messages: contents
                .iter()
                .map(|c| ParsedMessage {
                    role: MessageRole::Assistant,
                    content: c.to_string(),
                    created_at: None,
                })
                .collect(),
            artifacts: vec![],
        }
    }

    fn noop_emitter() -> Arc<dyn EventEmitter> {
        Arc::new(NoopEmitter)
    }

    #[tokio::test]
    async fn bulk_import_stores_all_memories() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);

        let batch = vec![
            make_convo("c1", &["m1", "m2", "m3"]),
            make_convo("c2", &["m4"]),
        ];
        let result = bulk_import_conversations(db_arc.clone(), &batch, noop_emitter(), "imp_test")
            .await
            .unwrap();
        assert_eq!(result.conversations_ingested, 2);
        assert_eq!(result.conversations_skipped_existing, 0);
        assert_eq!(result.memories_stored, 4);
    }

    /// Chunk indexes stored for one imported conversation, in order.
    async fn stored_chunk_indexes(db: &MemoryDB, external_id: &str) -> Vec<i32> {
        db.get_memories_by_source_id("memory", &Vendor::Claude.build_source_id(external_id))
            .await
            .unwrap()
            .iter()
            .map(|row| row.chunk_index)
            .collect()
    }

    async fn unembedded(db: &MemoryDB, external_id: &str) -> usize {
        db.count_unembedded_chunks("memory", &Vendor::Claude.build_source_id(external_id))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn bulk_import_embeds_every_stored_row() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);

        let batch = vec![
            make_convo(
                "c1",
                &["m1", "my dog Biscuit loves running on the beach", "m3"],
            ),
            make_convo("c2", &["m4"]),
        ];
        let result = bulk_import_conversations(db_arc.clone(), &batch, noop_emitter(), "imp_test")
            .await
            .unwrap();
        assert_eq!(result.memories_stored, 4);

        // The rows must exist before "zero unembedded" means anything.
        assert_eq!(stored_chunk_indexes(&db_arc, "c1").await, vec![0, 1, 2]);
        assert_eq!(stored_chunk_indexes(&db_arc, "c2").await, vec![0]);
        assert_eq!(unembedded(&db_arc, "c1").await, 0);
        assert_eq!(unembedded(&db_arc, "c2").await, 0);

        // Vector-only proof: the row is in the DiskANN index, so a query with
        // no word overlap reaches it without FTS.
        let hits = db_arc
            .naive_vector_search("pet canine seaside", 10, None)
            .await
            .unwrap();
        assert!(
            hits.iter()
                .any(|hit| hit.content == "my dog Biscuit loves running on the beach"),
            "imported row missing from vector-only search: {:?}",
            hits.iter().map(|hit| &hit.content).collect::<Vec<_>>()
        );
    }

    async fn import_with_fault(
        db: &Arc<MemoryDB>,
        batch: &[ParsedConversation],
        flush_entries: usize,
        marker: &'static str,
        fault: crate::db::RawImportEmbeddingFault,
    ) -> Result<BulkImportResult, WenlanError> {
        crate::db::with_raw_import_embedding_fault(
            marker,
            fault,
            bulk_import_conversations_with_flush(
                db.clone(),
                batch,
                noop_emitter(),
                "imp_test",
                flush_entries,
                None,
            ),
        )
        .await
    }

    #[tokio::test]
    async fn bulk_import_flushes_only_at_conversation_boundaries() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);

        // Slice size 2 over conversations of 3, 1, 3 and 1 messages. The fault
        // fires on the slice holding c's last message. A slice cut at the
        // flush size would already have committed c's first messages; whole
        // conversation slices commit a and b and leave no row of c or d.
        let batch = vec![
            make_convo("a", &["a0", "a1", "a2"]),
            make_convo("b", &["b0"]),
            make_convo("c", &["c0", "c1", "c2 boom"]),
            make_convo("d", &["d0"]),
        ];
        let failed = import_with_fault(
            &db_arc,
            &batch,
            2,
            "boom",
            crate::db::RawImportEmbeddingFault::Fail,
        )
        .await;
        assert!(
            matches!(failed, Err(WenlanError::Embedding(_))),
            "the faulted slice must fail the import"
        );
        assert_eq!(stored_chunk_indexes(&db_arc, "a").await, vec![0, 1, 2]);
        assert_eq!(stored_chunk_indexes(&db_arc, "b").await, vec![0]);
        assert_eq!(stored_chunk_indexes(&db_arc, "c").await, Vec::<i32>::new());
        assert_eq!(stored_chunk_indexes(&db_arc, "d").await, Vec::<i32>::new());

        // The retry resumes through dedup and completes every conversation.
        let retry = bulk_import_conversations_with_flush(
            db_arc.clone(),
            &batch,
            noop_emitter(),
            "imp",
            2,
            None,
        )
        .await
        .unwrap();
        assert_eq!(retry.conversations_skipped_existing, 2);
        assert_eq!(retry.conversations_ingested, 2);
        assert_eq!(retry.memories_stored, 4);
        for (id, len) in [("a", 3), ("b", 1), ("c", 3), ("d", 1)] {
            assert_eq!(
                stored_chunk_indexes(&db_arc, id).await,
                (0..len).collect::<Vec<i32>>(),
                "conversation {id} must be stored whole"
            );
            assert_eq!(unembedded(&db_arc, id).await, 0, "conversation {id}");
        }
    }

    #[tokio::test]
    async fn bulk_import_stores_an_oversized_conversation_as_its_own_slice() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);
        let big: Vec<String> = (0..5).map(|i| format!("big{i}")).collect();
        let big: Vec<&str> = big.iter().map(String::as_str).collect();

        // Fault after the oversized conversation: it was committed whole, so it
        // shared no slice with the faulted conversation after it.
        let after = vec![
            make_convo("x1", &["x1"]),
            make_convo("big1", &big),
            make_convo("y1", &["y1 boom"]),
        ];
        let failed = import_with_fault(
            &db_arc,
            &after,
            2,
            "boom",
            crate::db::RawImportEmbeddingFault::Fail,
        )
        .await;
        assert!(failed.is_err());
        assert_eq!(stored_chunk_indexes(&db_arc, "x1").await, vec![0]);
        assert_eq!(
            stored_chunk_indexes(&db_arc, "big1").await,
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(unembedded(&db_arc, "big1").await, 0);
        assert_eq!(stored_chunk_indexes(&db_arc, "y1").await, Vec::<i32>::new());

        // Fault inside the oversized conversation: the conversation before it
        // stays committed, and no part of the oversized one lands.
        let mut big_boom = big.clone();
        big_boom[4] = "big4 boom";
        let inside = vec![
            make_convo("x2", &["x2"]),
            make_convo("big2", &big_boom),
            make_convo("y2", &["y2"]),
        ];
        let failed = import_with_fault(
            &db_arc,
            &inside,
            2,
            "boom",
            crate::db::RawImportEmbeddingFault::Fail,
        )
        .await;
        assert!(failed.is_err());
        assert_eq!(stored_chunk_indexes(&db_arc, "x2").await, vec![0]);
        assert_eq!(
            stored_chunk_indexes(&db_arc, "big2").await,
            Vec::<i32>::new()
        );
        assert_eq!(stored_chunk_indexes(&db_arc, "y2").await, Vec::<i32>::new());
    }

    #[tokio::test]
    async fn bulk_import_embedding_failure_leaves_no_rows() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);
        let batch = vec![
            make_convo("c1", &["m1", "m2 boom"]),
            make_convo("c2", &["m3"]),
        ];

        for fault in [
            crate::db::RawImportEmbeddingFault::Fail,
            crate::db::RawImportEmbeddingFault::MissingVector,
        ] {
            let failed =
                import_with_fault(&db_arc, &batch, RAW_IMPORT_FLUSH_ENTRIES, "boom", fault).await;
            assert!(
                matches!(failed, Err(WenlanError::Embedding(_))),
                "{fault:?} must fail the import"
            );
            assert_eq!(stored_chunk_indexes(&db_arc, "c1").await, Vec::<i32>::new());
            assert_eq!(stored_chunk_indexes(&db_arc, "c2").await, Vec::<i32>::new());
        }

        // Nothing was half-written, so a clean retry imports both whole.
        let retry = bulk_import_conversations(db_arc.clone(), &batch, noop_emitter(), "imp")
            .await
            .unwrap();
        assert_eq!(retry.conversations_skipped_existing, 0);
        assert_eq!(retry.memories_stored, 3);
        assert_eq!(unembedded(&db_arc, "c1").await, 0);
    }

    #[tokio::test]
    async fn bulk_import_registers_restart_safe_scheduler_origin() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);
        let source_id = Vendor::Claude.build_source_id("scheduler-origin");

        bulk_import_conversations(
            db_arc.clone(),
            &[make_convo(
                "scheduler-origin",
                &["Imported conversation body"],
            )],
            noop_emitter(),
            "imp_scheduler_origin",
        )
        .await
        .unwrap();

        assert_eq!(
            db_arc.resolve_enrichment_origin(&source_id).await.unwrap(),
            crate::db::EnrichmentOrigin {
                memory_type_explicit: false,
                structured_fields_explicit: false,
                space_rejected: false,
            }
        );
        assert_eq!(
            db_arc
                .get_classification_candidate(3)
                .await
                .unwrap()
                .map(|candidate| candidate.source_id),
            Some(source_id)
        );
    }

    #[tokio::test]
    async fn bulk_import_skips_existing_conversations() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);

        let first = vec![make_convo("c1", &["m1"])];
        bulk_import_conversations(db_arc.clone(), &first, noop_emitter(), "imp_test")
            .await
            .unwrap();

        // Re-import the same conversation plus a new one.
        let second = vec![make_convo("c1", &["m1"]), make_convo("c2", &["m2"])];
        let result = bulk_import_conversations(db_arc.clone(), &second, noop_emitter(), "imp_test")
            .await
            .unwrap();
        assert_eq!(result.conversations_ingested, 1);
        assert_eq!(result.conversations_skipped_existing, 1);
        assert_eq!(result.memories_stored, 1);
    }

    #[tokio::test]
    async fn bulk_import_empty_batch_returns_zeros() {
        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);

        let result = bulk_import_conversations(db_arc.clone(), &[], noop_emitter(), "imp_test")
            .await
            .unwrap();
        assert_eq!(result.conversations_ingested, 0);
        assert_eq!(result.conversations_skipped_existing, 0);
        assert_eq!(result.memories_stored, 0);
    }

    #[tokio::test]
    async fn import_state_roundtrip() {
        use super::ImportStage;

        let (db, _tmp) = crate::db::tests::test_db().await;
        let db_arc = Arc::new(db);

        let id = "imp_xyz".to_string();
        db_arc
            .start_import_state(&id, Vendor::Claude, "/tmp/test.zip")
            .await
            .unwrap();

        let loaded = db_arc.load_import_state(&id).await.unwrap().expect("row");
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.vendor, Vendor::Claude);
        assert_eq!(loaded.stage, ImportStage::Parsing);

        db_arc
            .update_import_state_stage(&id, ImportStage::StageA, Some(42), Some(5))
            .await
            .unwrap();
        let loaded = db_arc.load_import_state(&id).await.unwrap().expect("row");
        assert_eq!(loaded.stage, ImportStage::StageA);
        assert_eq!(loaded.total_conversations, Some(42));
        assert_eq!(loaded.processed_conversations, 5);

        db_arc
            .update_import_state_stage(&id, ImportStage::Done, None, None)
            .await
            .unwrap();
        let loaded = db_arc.load_import_state(&id).await.unwrap().expect("row");
        assert_eq!(loaded.stage, ImportStage::Done);
        // total_conversations should be preserved from prior update
        assert_eq!(loaded.total_conversations, Some(42));
    }

    #[tokio::test]
    async fn fail_unfinished_imports_ends_only_non_terminal_rows() {
        use super::ImportStage;

        let (db, _tmp) = crate::db::tests::test_db().await;
        for (id, stage, error) in [
            ("imp_parsing", ImportStage::Parsing, None),
            ("imp_stage_a", ImportStage::StageA, None),
            ("imp_done", ImportStage::Done, None),
            (
                "imp_error",
                ImportStage::Error,
                Some("bulk ingest: disk full"),
            ),
        ] {
            db.start_import_state(id, Vendor::Claude, "/tmp/export.zip")
                .await
                .unwrap();
            db.update_import_state_stage_with_error(id, stage, Some(3), Some(1), error)
                .await
                .unwrap();
        }
        let done_before = db.load_import_state("imp_done").await.unwrap().unwrap();
        assert_eq!(db.list_pending_imports().await.unwrap().len(), 2);

        let message = "interrupted: the daemon restarted before this import finished";
        assert_eq!(db.fail_unfinished_imports(message).await.unwrap(), 2);

        for id in ["imp_parsing", "imp_stage_a"] {
            let row = db.load_import_state(id).await.unwrap().unwrap();
            assert_eq!(row.stage, ImportStage::Error, "{id}");
            assert_eq!(row.error_message.as_deref(), Some(message), "{id}");
            assert_eq!(row.processed_conversations, 1, "{id} keeps its progress");
        }
        let done = db.load_import_state("imp_done").await.unwrap().unwrap();
        assert_eq!(done.stage, ImportStage::Done);
        assert_eq!(done.error_message, None);
        assert_eq!(done.updated_at, done_before.updated_at);
        let error = db.load_import_state("imp_error").await.unwrap().unwrap();
        assert_eq!(error.stage, ImportStage::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("bulk ingest: disk full"),
            "an already failed import keeps its own reason"
        );
        assert!(db.list_pending_imports().await.unwrap().is_empty());

        // Idempotent: a second boot finds nothing left to end.
        assert_eq!(db.fail_unfinished_imports(message).await.unwrap(), 0);
    }
}
