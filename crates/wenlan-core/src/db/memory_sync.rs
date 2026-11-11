// SPDX-License-Identifier: Apache-2.0
//! Writing a memory that another of the person's computers shared (the
//! library share, `shared_space::memories`).
//!
//! The memory is stored the way a new capture is, with no vector: the rows
//! are keyword-searchable at once, the background embedding catch-up
//! ([`MemoryDB::recover_null_memory_embeddings_batch`]) gives them one, and
//! because no `enrichment_steps` row exists for the new version, the ambient
//! classification and extraction lanes pick it up like any other capture.

use std::collections::HashMap;

use super::{EnrichmentOrigin, MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;
use crate::sources::RawDocument;
use crate::store::format::memory::MemoryFile;
use crate::store::rebuild::memories::{fields_text, seconds_of};

/// How long a title made from the text is, when the file names none.
const TITLE_CHARS: usize = 60;

impl MemoryDB {
    /// Makes the memory `file` here: a new one with the file's id, or the
    /// existing one replaced, as a capture would store it (no vector yet,
    /// no enrichment receipt, so the catch-up and the ambient lanes take
    /// it). The kind and stated fields count as stated only when the file
    /// has them; the Space is never re-decided by enrichment.
    pub(crate) async fn apply_shared_memory(&self, file: &MemoryFile) -> Result<(), WenlanError> {
        let source = match file.other.get("source") {
            Some(serde_yaml::Value::String(source)) => source.clone(),
            _ => "memory".to_string(),
        };
        let flag = |key: &str| {
            file.other
                .get(key)
                .and_then(serde_yaml::Value::as_bool)
                .unwrap_or(false)
        };
        let text_key = |key: &str| match file.other.get(key) {
            Some(serde_yaml::Value::String(text)) => Some(text.clone()),
            _ => None,
        };
        let mut metadata = HashMap::new();
        if flag("typed") && source == "memory" {
            // Typed in the app: kept as the person's own writing.
            metadata.insert("capture_source".to_string(), "manual".to_string());
        }
        let title = file
            .title
            .clone()
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| file.text.chars().take(TITLE_CHARS).collect());
        let doc = RawDocument {
            source: source.clone(),
            source_id: file.id.clone(),
            title,
            content: file.text.clone(),
            url: file.url.clone(),
            last_modified: chrono::Utc::now().timestamp(),
            metadata,
            memory_type: Some(file.kind.clone().unwrap_or_else(|| "fact".to_string())),
            // An explicit Unfiled, or the replacement would keep the old Space.
            space: Some(
                file.space
                    .clone()
                    .unwrap_or_else(|| UNFILED_SPACE_ID.to_string()),
            ),
            source_agent: file.agent.clone(),
            confirmed: Some(file.confirmed),
            stability: Some(
                if file.confirmed && source == "memory" {
                    "confirmed"
                } else {
                    "new"
                }
                .to_string(),
            ),
            supersedes: file.supersedes.clone(),
            importance: file.importance.and_then(|n| u8::try_from(n).ok()),
            is_recap: file.recap,
            enrichment_status: "raw".to_string(),
            supersede_mode: file
                .supersede_mode
                .clone()
                .unwrap_or_else(|| "hide".to_string()),
            structured_fields: file.fields.as_ref().map(fields_text),
            retrieval_cue: text_key("cue"),
            source_text: file
                .source_text
                .clone()
                .or_else(|| flag("source_text_is_text").then(|| file.text.clone())),
            ..Default::default()
        };
        if source == "memory" {
            self.upsert_enrichment_origin(
                &file.id,
                EnrichmentOrigin {
                    memory_type_explicit: file.kind.is_some(),
                    structured_fields_explicit: file.fields.is_some(),
                    space_rejected: true,
                },
            )
            .await?;
        }
        let written = self.upsert_documents_without_embeddings(vec![doc]).await?;
        if written == 0 {
            return Err(WenlanError::Validation(format!(
                "memory {} has no text to store",
                file.id
            )));
        }
        // What a capture has no way to say.
        let created = file.created.as_deref().and_then(seconds_of);
        let at = |text: &Option<String>| text.as_deref().and_then(seconds_of);
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE memories SET pinned = ?1, event_date = ?2, event_end = ?3, \
                    session_id = ?4, episode_of = ?5, \
                    created_at = COALESCE(?6, created_at), \
                    origin_class = COALESCE(?7, origin_class) \
             WHERE source = ?8 AND source_id = ?9",
            libsql::params![
                i64::from(file.pinned),
                at(&file.event_date),
                at(&file.event_end),
                file.session.clone(),
                file.episode_of.clone(),
                created,
                file.origin.clone(),
                source,
                file.id.clone()
            ],
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("apply shared memory: {e}")))?;
        Ok(())
    }
}
