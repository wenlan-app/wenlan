//! Private citation provenance for review-gated page refreshes. Card JSON is
//! wire-writable; this receipt is written only from the pipeline's verified
//! citation argument and its pre-generation page fence.
use super::{MemoryDB, PageFence};
use crate::{citations::NumberedSource, WenlanError};
use serde::{Deserialize, Serialize};
use wenlan_types::pages::PageCitation;

pub(crate) fn revision_citations_key(revision_id: &str) -> String {
    format!("page_revision_citations_v1:{revision_id}")
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RevisionCitations {
    pub page_id: String,
    pub page_version: i64,
    pub source_revision: i64,
    pub incarnation: String,
    pub content: String,
    pub source_ids: Vec<String>,
    pub citations: Vec<PageCitation>,
}

impl MemoryDB {
    /// Saving after the card is safe: a crash can lose annotations, never the
    /// proposal. The receipt cannot follow a replaced page or changed sources.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn record_revision_citations(
        &self,
        revision_id: &str,
        page_id: &str,
        page_version: i64,
        fence: &PageFence,
        content: &str,
        source_ids: &[String],
        citations_json: &str,
    ) -> Result<(), WenlanError> {
        let receipt = RevisionCitations {
            page_id: page_id.into(),
            page_version,
            source_revision: fence.source_revision,
            incarnation: fence.incarnation.clone(),
            content: content.into(),
            source_ids: source_ids.to_vec(),
            citations: serde_json::from_str(citations_json)?,
        };
        let value = serde_json::to_string(&receipt)?;
        let conn = self.conn.lock().await;
        let Some(pending) =
            Self::pending_memory_revision_payload_on_conn(&conn, revision_id).await?
        else {
            return Ok(());
        };
        let fields = pending
            .structured_fields
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
        let same_metadata = fields.as_ref().is_some_and(|fields| {
            fields.get("page_version").and_then(|v| v.as_i64()) == Some(page_version)
                && fields.get("source_revision").and_then(|v| v.as_i64())
                    == Some(fence.source_revision)
                && fields.get("source_memory_ids") == Some(&serde_json::json!(source_ids))
        });
        if pending.revision_id != revision_id
            || pending.supersedes != page_id
            || pending.full_body.as_ref().unwrap_or(&pending.content) != content
            || !same_metadata
        {
            return Ok(());
        }
        // Dedup may return an existing card. Bind only the exact proposal and
        // source snapshot produced here; a later edit cannot acquire a receipt.
        conn.execute(
            "INSERT INTO app_metadata(key, value)
             SELECT ?1, ?2 WHERE EXISTS (
                 SELECT 1 FROM pages WHERE id=?3 AND version=?4
                 AND COALESCE(source_revision,0)=?5 AND incarnation=?6 AND status='active'
             ) AND EXISTS (
                 SELECT 1 FROM memories WHERE source='memory' AND source_id=?7
                 AND supersedes=?3 AND pending_revision=1 AND chunk_index=0
                 AND COALESCE(version,1)=?8
             ) ON CONFLICT(key) DO NOTHING",
            libsql::params![
                revision_citations_key(revision_id),
                value,
                page_id,
                page_version,
                fence.source_revision,
                fence.incarnation.as_str(),
                revision_id,
                pending.revision_version
            ],
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("record revision citations: {e}")))?;
        Ok(())
    }

    pub(crate) async fn revision_citations(
        &self,
        revision_id: &str,
    ) -> Result<Option<RevisionCitations>, WenlanError> {
        let Some(raw) = self
            .get_app_metadata(&revision_citations_key(revision_id))
            .await?
        else {
            return Ok(None);
        };
        Ok(serde_json::from_str(&raw).ok())
    }
}

impl RevisionCitations {
    /// Recompute metadata, never approved prose. Keep numbers from the trusted
    /// generation receipt: DB result order and caller/card JSON do not define
    /// the original prompt's [N] identity. Missing/ambiguous markers fail closed.
    pub(crate) async fn verify(
        &self,
        db: &MemoryDB,
        content: &str,
        source_ids: &[String],
    ) -> Result<Option<String>, WenlanError> {
        if self.content != content || self.source_ids != source_ids || self.citations.is_empty() {
            return Ok(None);
        }
        let max = self
            .citations
            .iter()
            .map(|c| c.marker as usize)
            .max()
            .unwrap_or(0);
        if max == 0 || max > source_ids.len() {
            return Ok(None);
        }
        let texts: std::collections::HashMap<_, _> = db
            .get_refresh_source_contents_by_ids(source_ids)
            .await?
            .into_iter()
            .collect();
        let mut numbered: Vec<NumberedSource> = (0..max)
            .map(|n| NumberedSource {
                index: (n + 1) as u32,
                source_kind: "memory".into(),
                locator: String::new(),
                text: String::new(),
            })
            .collect();
        for citation in &self.citations {
            if citation.marker == 0 || !source_ids.contains(&citation.locator) {
                return Ok(None);
            }
            let source = &mut numbered[citation.marker as usize - 1];
            if !source.locator.is_empty()
                && (source.locator != citation.locator
                    || source.source_kind != citation.source_kind)
            {
                return Ok(None);
            }
            let Some(text) = texts.get(&citation.locator) else {
                return Ok(None);
            };
            source.locator.clone_from(&citation.locator);
            source.source_kind.clone_from(&citation.source_kind);
            source.text.clone_from(text);
        }
        let (body, citations, stats) =
            crate::citations::process_citation_output(content, &numbered);
        if body != content
            || stats.stripped > 0
            || stats.unverified > 0
            || stats.verified == 0
            || citations.iter().any(|c| c.locator.is_empty())
        {
            return Ok(None);
        }
        Ok(Some(serde_json::to_string(&citations)?))
    }
}
