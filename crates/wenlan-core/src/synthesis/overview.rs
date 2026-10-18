// SPDX-License-Identifier: Apache-2.0
//! Compatibility maintenance for Overview pages from older versions.
//! Never creates an Overview: new libraries grow topic pages from actual user
//! sources. Untouched legacy placeholders are archived; useful existing pages
//! retain their content and citations if a refresh is rejected.

use std::path::Path;
use std::sync::Arc;

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::LlmProvider;
use crate::prompts::PromptRegistry;
use crate::synthesis::distill::{
    refresh_page_with_candidate_sources, RefreshOutcome, RefreshReason,
};

/// Legacy title used only to find an existing page for maintenance.
pub const OVERVIEW_PAGE_TITLE: &str = "Overview";

/// How many of the most recently touched active pages feed the overview's
/// evidence set. Small and fixed on purpose -- spec says "no new machinery".
const OVERVIEW_TOP_PAGES: i64 = 5;

/// Do not rewrite a legacy summary when there is too little source context.
const OVERVIEW_MIN_CONCEPT_PAGES: usize = 2;

/// Placeholder body minted by the pre-fix flow. Used only for conditional
/// archival; new pages never contain this text.
pub(crate) const OVERVIEW_PLACEHOLDER_CONTENT: &str =
    "This page is refreshed automatically to summarize the wiki's current top pages.";

/// Whether a page counts as Overview evidence: an active, non-empty,
/// derived concept page that is not the Overview itself nor a shell row.
fn is_overview_evidence_page(page: &crate::pages::Page) -> bool {
    if page.title.eq_ignore_ascii_case(OVERVIEW_PAGE_TITLE) {
        return false;
    }
    // The entity kind is a trusted mutation/read fence. All other routing is
    // derived from the authoritative title, creation kind, and status below.
    if page.kind == "entity" {
        return false;
    }
    if crate::pages::page_kind_for(&page.title, &page.creation_kind, &page.status) != "concept" {
        return false;
    }
    if page.content.trim().is_empty() {
        return false;
    }
    true
}

/// Count of qualifying evidence pages among the current top pages, plus
/// their source memory ids. A page qualifies only when its sources resolve
/// to real memory contents. `exclude_page_id` keeps the overview from citing
/// itself.
async fn qualifying_top_evidence(
    db: &MemoryDB,
    exclude_page_id: Option<&str>,
) -> Result<(usize, Vec<String>), WenlanError> {
    let pages = db.list_pages("active", OVERVIEW_TOP_PAGES + 1, 0).await?;
    let mut ids = Vec::new();
    let mut qualifying = 0usize;
    for page in pages {
        if qualifying as i64 >= OVERVIEW_TOP_PAGES {
            break;
        }
        if Some(page.id.as_str()) == exclude_page_id || !is_overview_evidence_page(&page) {
            continue;
        }
        let sources = db.get_page_sources(&page.id).await?;
        let page_source_ids: Vec<String> = if sources.is_empty() {
            page.source_memory_ids.clone()
        } else {
            sources.into_iter().map(|s| s.memory_source_id).collect()
        };
        if page_source_ids.is_empty() {
            continue;
        }
        if db
            .get_memory_contents_by_ids(&page_source_ids)
            .await?
            .is_empty()
        {
            continue;
        }
        ids.extend(page_source_ids);
        qualifying += 1;
    }
    Ok((qualifying, ids))
}

/// Whether two source sets cover the same evidence, ignoring order and
/// duplicates.
fn same_source_set(a: &[String], b: &[String]) -> bool {
    let mut x = a.to_vec();
    x.sort();
    x.dedup();
    let mut y = b.to_vec();
    y.sort();
    y.dedup();
    x == y
}

/// Whether a row is a pre-fix empty placeholder: exact placeholder body,
/// machine-owned, never user-edited, not review-locked.
fn is_legacy_placeholder(page: &crate::pages::Page) -> bool {
    !page.user_edited
        && !page.review_required
        && page.creation_kind == "research"
        && page.content.trim() == OVERVIEW_PLACEHOLDER_CONTENT
}

/// Refresh an existing, machine-owned, already-populated Overview in place.
/// Skips silently while evidence is thin or unchanged since the last refresh.
async fn refresh_existing_overview_page(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    _agent: &str,
    knowledge_path: Option<&Path>,
    page_id: &str,
) -> Result<RefreshOutcome, WenlanError> {
    let (qualifying, top_sources) = qualifying_top_evidence(db, Some(page_id)).await?;
    if qualifying < OVERVIEW_MIN_CONCEPT_PAGES || top_sources.is_empty() {
        log::info!("[overview] skipping refresh: only {qualifying} qualifying concept pages");
        return Ok(RefreshOutcome::default());
    }
    // No repeated LLM refresh while the evidence set is unchanged.
    let current = db.get_page_sources(page_id).await?;
    let current_ids: Vec<String> = if current.is_empty() {
        db.get_page(page_id)
            .await?
            .map(|p| p.source_memory_ids)
            .unwrap_or_default()
    } else {
        current.into_iter().map(|s| s.memory_source_id).collect()
    };
    if same_source_set(&current_ids, &top_sources) {
        let Some(page) = db.get_page(page_id).await? else {
            return Ok(RefreshOutcome::default());
        };
        if !db.has_page_sources_changed(&page).await? {
            if let Some(reason) = page.refresh_blocked_reason {
                return Ok(RefreshOutcome {
                    discard_reason: Some(reason),
                    ..RefreshOutcome::default()
                });
            }
            if page.stale_reason.is_none() {
                return Ok(RefreshOutcome::default());
            }
        }
    }

    let outcome = refresh_page_with_candidate_sources(
        db,
        llm,
        &prompts.overview_summary,
        page_id,
        RefreshReason::SourceChanged,
        knowledge_path,
        Some(&top_sources),
    )
    .await?;
    Ok(outcome)
}

/// Maintain an existing legacy overview. A missing row is never created.
/// User namesakes stay intact; untouched placeholders are safely archived.
pub async fn refresh_overview_page(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    _agent: &str,
    knowledge_path: Option<&Path>,
) -> Result<RefreshOutcome, WenlanError> {
    if let Some(page_id) = db.find_active_machine_overview_id().await? {
        let source_revision = db.get_page_source_revision(&page_id).await?;
        let page = db.get_page(&page_id).await?;
        if let Some(page) = page {
            if page.user_edited || page.creation_kind == "authored" {
                return Ok(RefreshOutcome::default());
            }
            if is_legacy_placeholder(&page) {
                if !db
                    .archive_legacy_overview_placeholder(
                        &page_id,
                        page.version,
                        source_revision,
                        OVERVIEW_PLACEHOLDER_CONTENT,
                    )
                    .await?
                {
                    return Ok(RefreshOutcome::default());
                }
                return Ok(RefreshOutcome::default());
            }
            return refresh_existing_overview_page(
                db,
                llm,
                prompts,
                _agent,
                knowledge_path,
                &page_id,
            )
            .await;
        }
    }
    Ok(RefreshOutcome::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDB;
    use crate::events::NoopEmitter;
    use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest, MockProvider};
    use crate::prompts::PromptRegistry;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    /// Records the system prompt of every `generate` call, so a test can
    /// assert the Overview refresh used the dedicated `overview_summary`
    /// prompt rather than the generic `distill_page` prompt every other page
    /// refresh uses.
    struct RecordingProvider {
        response: String,
        system_prompt: StdMutex<Option<String>>,
    }

    impl RecordingProvider {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                system_prompt: StdMutex::new(None),
            }
        }

        fn captured_system_prompt(&self) -> Option<String> {
            self.system_prompt.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for RecordingProvider {
        async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
            *self.system_prompt.lock().unwrap() = request.system_prompt.clone();
            Ok(self.response.clone())
        }

        fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "overview-recording"
        }

        fn backend(&self) -> LlmBackend {
            LlmBackend::OnDevice
        }

        fn kind(&self) -> &'static str {
            "mock"
        }
    }

    async fn test_db() -> (MemoryDB, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = MemoryDB::new(&path, Arc::new(NoopEmitter)).await.unwrap();
        (db, dir)
    }

    fn make_doc(source_id: &str, content: &str) -> crate::sources::RawDocument {
        crate::sources::RawDocument {
            source: "memory".to_string(),
            source_id: source_id.to_string(),
            title: content.chars().take(40).collect(),
            summary: None,
            content: content.to_string(),
            url: None,
            last_modified: chrono::Utc::now().timestamp(),
            metadata: std::collections::HashMap::new(),
            memory_type: Some("fact".to_string()),
            space: None,
            source_agent: Some("test".to_string()),
            confidence: Some(0.7),
            confirmed: Some(false),
            stability: None,
            supersedes: None,
            pending_revision: false,
            entity_id: None,
            quality: None,
            importance: None,
            is_recap: false,
            enrichment_status: "raw".to_string(),
            supersede_mode: "hide".to_string(),
            structured_fields: None,
            retrieval_cue: None,
            source_text: None,
            content_hash: None,
        }
    }

    async fn create_research_page(db: &MemoryDB, title: &str, mem_id: &str, content: &str) {
        db.upsert_documents(vec![make_doc(mem_id, content)])
            .await
            .unwrap();
        let req = wenlan_types::requests::CreateConceptRequest {
            title: title.to_string(),
            content: content.to_string(),
            summary: None,
            entity_id: None,
            space: None.into(),
            source_memory_ids: vec![mem_id.to_string()],
            creation_kind: Some("research".to_string()),
            workspace: None,
        };
        crate::post_write::create_page(db, req, "test", None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn never_creates_overview_even_with_meaningful_sources() {
        let (db, _dir) = test_db().await;
        let provider = Arc::new(RecordingProvider::new("must not be called"));
        let llm: Arc<dyn LlmProvider> = provider.clone();
        let prompts = PromptRegistry::default();
        assert!(
            !refresh_overview_page(&db, &llm, &prompts, "test", None)
                .await
                .unwrap()
                .wrote
        );
        create_research_page(
            &db,
            "Rust",
            "rust",
            "Rust is a systems programming language with memory safety guarantees",
        )
        .await;
        create_research_page(
            &db,
            "Python",
            "python",
            "Python is a dynamically typed programming language emphasizing readability",
        )
        .await;
        assert!(
            !refresh_overview_page(&db, &llm, &prompts, "test", None)
                .await
                .unwrap()
                .wrote
        );
        assert!(db
            .find_active_page_id_by_title("Overview")
            .await
            .unwrap()
            .is_none());
        assert!(
            provider.captured_system_prompt().is_none(),
            "do not spend inference on an invented starting page"
        );
    }

    async fn seed_overview(db: &MemoryDB, id: &str, body: &str, creation_kind: &str) {
        db.insert_page_with_kind(
            id,
            "Overview",
            None,
            body,
            None,
            None,
            &[],
            &chrono::Utc::now().to_rfc3339(),
            creation_kind,
            "unconfirmed",
            None,
            None,
        )
        .await
        .unwrap();
    }

    async fn seed_legacy(db: &MemoryDB, body: &str) {
        seed_overview(db, "legacy-overview", body, "research").await;
    }

    #[tokio::test]
    async fn legacy_placeholder_is_archived_without_replacement() {
        let (db, _dir) = test_db().await;
        seed_legacy(&db, OVERVIEW_PLACEHOLDER_CONTENT).await;
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::unavailable());
        refresh_overview_page(&db, &llm, &PromptRegistry::default(), "test", None)
            .await
            .unwrap();
        assert!(db
            .find_active_page_id_by_title("Overview")
            .await
            .unwrap()
            .is_none());
        let saved = db.get_page("legacy-overview").await.unwrap().unwrap();
        assert_eq!(saved.status, "archived");
        assert_eq!(saved.content, OVERVIEW_PLACEHOLDER_CONTENT);
    }

    #[tokio::test]
    async fn authored_overview_namesake_does_not_hide_legacy_placeholder() {
        let (db, _dir) = test_db().await;
        seed_overview(
            &db,
            "authored-overview",
            "My own overview stays active.",
            "authored",
        )
        .await;
        seed_legacy(&db, OVERVIEW_PLACEHOLDER_CONTENT).await;
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::unavailable());

        refresh_overview_page(&db, &llm, &PromptRegistry::default(), "test", None)
            .await
            .unwrap();

        assert_eq!(
            db.get_page("authored-overview")
                .await
                .unwrap()
                .unwrap()
                .status,
            "active",
            "an authored namesake must remain active"
        );
        assert_eq!(
            db.get_page("legacy-overview")
                .await
                .unwrap()
                .unwrap()
                .status,
            "archived",
            "the eligible legacy placeholder must still be archived"
        );
    }

    #[tokio::test]
    async fn overview_evidence_uses_authoritative_fields_when_kind_is_stale() {
        let (db, _dir) = test_db().await;
        create_research_page(
            &db,
            "Research",
            "research-memory",
            "Research page body supplies evidence for the maintenance summary.",
        )
        .await;
        let page_id = db
            .find_active_page_id_by_title("Research")
            .await
            .unwrap()
            .expect("research page id");
        let mut page = db
            .get_page(&page_id)
            .await
            .unwrap()
            .expect("research page seeded");
        assert_eq!(page.creation_kind, "research");
        page.kind = "source".to_string();
        assert!(
            is_overview_evidence_page(&page),
            "stored non-entity kind must not hide authoritative research evidence"
        );
        page.kind = "entity".to_string();
        assert!(
            !is_overview_evidence_page(&page),
            "the trusted entity fence must still exclude entity shadow pages"
        );
    }

    #[tokio::test]
    async fn rejected_refresh_preserves_existing_body_sources_and_citations() {
        let (db, _dir) = test_db().await;
        create_research_page(
            &db,
            "Rust",
            "rust",
            "Rust is a systems programming language with memory safety guarantees",
        )
        .await;
        create_research_page(
            &db,
            "Python",
            "python",
            "Python is a dynamically typed programming language emphasizing readability",
        )
        .await;
        seed_legacy(
            &db,
            "Existing useful overview with its original evidence.[1]",
        )
        .await;
        db.link_page_source("legacy-overview", "rust", "test")
            .await
            .unwrap();
        let saved = db.get_page("legacy-overview").await.unwrap().unwrap();
        let sources = db
            .get_page_sources("legacy-overview")
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.memory_source_id)
            .collect::<Vec<_>>();
        let llm: Arc<dyn LlmProvider> =
            Arc::new(MockProvider::new("The moon is made of green cheese. [1]"));
        let result = refresh_overview_page(&db, &llm, &PromptRegistry::default(), "test", None)
            .await
            .unwrap();
        assert!(!result.wrote);
        assert!(result.discard_reason.is_some());
        let after = db.get_page("legacy-overview").await.unwrap().unwrap();
        assert_eq!(after.content, saved.content);
        assert_eq!(
            serde_json::to_value(after.citations).unwrap(),
            serde_json::to_value(saved.citations).unwrap()
        );
        assert_eq!(after.stale_reason, saved.stale_reason);
        assert_eq!(
            db.get_page_sources("legacy-overview")
                .await
                .unwrap()
                .into_iter()
                .map(|s| s.memory_source_id)
                .collect::<Vec<_>>(),
            sources
        );
    }

    #[tokio::test]
    async fn user_edited_overview_is_untouched() {
        let (db, _dir) = test_db().await;
        seed_legacy(&db, OVERVIEW_PLACEHOLDER_CONTENT).await;
        {
            let conn = db.test_primary_session().await;
            conn.execute(
                "UPDATE pages SET user_edited=1 WHERE id='legacy-overview'",
                (),
            )
            .await
            .unwrap();
        }
        let llm: Arc<dyn LlmProvider> = Arc::new(MockProvider::unavailable());
        refresh_overview_page(&db, &llm, &PromptRegistry::default(), "test", None)
            .await
            .unwrap();
        assert_eq!(
            db.get_page("legacy-overview")
                .await
                .unwrap()
                .unwrap()
                .status,
            "active"
        );
    }
}
