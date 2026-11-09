// SPDX-License-Identifier: Apache-2.0
//! Entity extraction phase: extract entities/relations from a single memory's content.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::read_scope::ReadScope;
use std::sync::Arc;

/// Extract KG entities from `content` via LLM, create/upsert them in the DB,
/// and return the list of entity ids. Does **not** write to `memory_entities` —
/// the caller is responsible for linking them to a specific memory.
///
/// This is the extraction-only primitive used by `run_enrichment_sweep` (and any
/// other caller that controls when/how linkage happens). `source_id`, when the
/// caller has it in hand, is threaded into each relation's `source_memory_id`
/// so its span payload can resolve back to a source memory; `None` keeps the
/// relation's payload `source_memory_id`-less, matching pre-M3g behavior.
pub async fn extract_entities_for_content(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    content: &str,
    source_id: Option<&str>,
) -> Result<Vec<String>, WenlanError> {
    let truncated: String = content.chars().take(500).collect();
    let numbered = format!("1. {}", truncated);

    let response = llm
        .generate(LlmRequest {
            system_prompt: Some(prompts.extract_knowledge_graph.clone()),
            user_prompt: numbered,
            max_tokens: 512,
            temperature: 0.3,
            label: Some("extract".into()),
            timeout_secs: None,
        })
        .await
        .map_err(|e| WenlanError::Llm(format!("entity extraction: {}", e)))?;

    let batch = [(0usize, content.to_string())];
    let kg_results = crate::extract::parse_kg_response(&response, &batch);
    let _space_write_guard = db.lock_space_writes().await;
    let entity_space = match source_id {
        Some(source_id) => match db.get_memory_space(source_id).await? {
            Some(space) => wenlan_types::WriteSpaceTarget::Named(space),
            None => wenlan_types::WriteSpaceTarget::Uncategorized,
        },
        None => wenlan_types::WriteSpaceTarget::Uncategorized,
    };

    let mut entity_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for kg in &kg_results {
        for entity in &kg.entities {
            let name_lower = entity.name.to_lowercase();
            if entity_cache.contains_key(&name_lower) {
                continue;
            }
            let req = wenlan_types::requests::CreateEntityRequest {
                name: entity.name.clone(),
                entity_type: entity.entity_type.clone(),
                space: entity_space.clone(),
                source_agent: Some("post_ingest".to_string()),
                confidence: None,
            };
            match crate::post_write::create_entity(db, req, "post_ingest").await {
                Ok(result) => {
                    entity_cache.insert(name_lower, result.id);
                }
                Err(e) => log::warn!("[extract_entities_for_content] entity create failed: {e}"),
            }
        }
        for obs in &kg.observations {
            if let Some(entity_id) = entity_cache.get(&obs.entity.to_lowercase()) {
                let req = wenlan_types::requests::AddObservationRequest {
                    entity_id: entity_id.clone(),
                    content: obs.content.clone(),
                    source_agent: Some("post_ingest".to_string()),
                    confidence: None,
                };
                if let Err(e) =
                    crate::post_write::add_observation(db, req, "post_ingest", &ReadScope::Global)
                        .await
                {
                    log::warn!("[extract_entities_for_content] add_observation failed: {e}");
                }
            }
        }
        for rel in &kg.relations {
            let from_id = entity_cache.get(&rel.from.to_lowercase()).cloned();
            let to_id = entity_cache.get(&rel.to.to_lowercase()).cloned();
            if let (Some(from), Some(to)) = (from_id, to_id) {
                let req = wenlan_types::requests::CreateRelationRequest {
                    preserve_existing: false,
                    from_entity: from,
                    to_entity: to,
                    relation_type: rel.relation_type.clone(),
                    source_agent: Some("post_ingest".to_string()),
                    confidence: rel.confidence,
                    explanation: rel.explanation.clone(),
                    source_memory_id: source_id.map(|s| s.to_string()),
                    span: rel.span.clone(),
                    model_version: Some(llm.model_id()),
                    prompt_version: Some(
                        crate::extract::EXTRACT_KNOWLEDGE_GRAPH_PROMPT_VERSION.to_string(),
                    ),
                };
                if let Err(e) = crate::post_write::create_relation_with_span(
                    db,
                    req,
                    "post_ingest",
                    Some(content),
                )
                .await
                {
                    log::warn!("[extract_entities_for_content] create_relation failed: {e}");
                }
            }
        }
    }

    Ok(entity_cache.into_values().collect())
}

/// Pure half: call the LLM and parse the KG response. No DB access.
/// Returns the raw parse result to be committed by `commit_kg`.
pub async fn extract_kg(
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    content: &str,
) -> Result<Vec<crate::extract::KgExtractionResult>, WenlanError> {
    let response = llm
        .generate(kg_extraction_request(prompts, content))
        .await
        .map_err(|e| WenlanError::Llm(format!("entity extraction: {}", e)))?;
    Ok(parse_kg_output(content, &response))
}

/// The one model request entity extraction spends on `content`. An
/// on-demand organize run hands this same request to the user's AI tool.
pub fn kg_extraction_request(prompts: &PromptRegistry, content: &str) -> LlmRequest {
    let truncated: String = content.chars().take(500).collect();
    LlmRequest {
        system_prompt: Some(prompts.extract_knowledge_graph.clone()),
        user_prompt: format!("1. {}", truncated),
        max_tokens: 512,
        temperature: 0.3,
        label: Some("extract".into()),
        timeout_secs: None,
    }
}

/// Parse a reply to [`kg_extraction_request`] for `content`. Never fails: an
/// unreadable reply is an empty result, as it always was.
pub fn parse_kg_output(content: &str, response: &str) -> Vec<crate::extract::KgExtractionResult> {
    let batch = [(0usize, content.to_string())];
    crate::extract::parse_kg_response(response, &batch)
}

/// Serial DB-write half: commit a parsed KG result set to the DB and link
/// the memory row identified by `source_id`. Returns the primary entity id.
/// `content` is the exact text `kg` was extracted from -- threaded through
/// (never re-fetched) so M3g span capture (§2.3) locates each relation's
/// quote against the string the model actually saw. `model_version` is the
/// extraction LLM's provenance for the same capture (§6.6).
pub async fn commit_kg(
    db: &MemoryDB,
    source_id: &str,
    kg: &[crate::extract::KgExtractionResult],
    content: &str,
    model_version: Option<&str>,
) -> Result<Option<String>, WenlanError> {
    let _space_write_guard = db.lock_space_writes().await;
    let entity_space = match db.get_memory_space(source_id).await? {
        Some(space) => wenlan_types::WriteSpaceTarget::Named(space),
        None => wenlan_types::WriteSpaceTarget::Uncategorized,
    };
    let mut entity_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Entity ids in the order the model listed them, so the first specific
    // one becomes the memory's topic.
    let mut listed: Vec<String> = Vec::new();

    for kg_item in kg {
        for entity in &kg_item.entities {
            let name_lower = entity.name.to_lowercase();
            // In-batch cache (per-extraction-pass dedup; capability fn doesn't take a cache)
            if let Some(id) = entity_cache.get(&name_lower) {
                if !listed.contains(id) {
                    listed.push(id.clone());
                }
                continue;
            }
            let req = wenlan_types::requests::CreateEntityRequest {
                name: entity.name.clone(),
                entity_type: entity.entity_type.clone(),
                space: entity_space.clone(),
                source_agent: Some("post_ingest".to_string()),
                confidence: None,
            };
            match crate::post_write::create_entity(db, req, "post_ingest").await {
                Ok(result) => {
                    entity_cache.insert(name_lower, result.id.clone());
                    if !listed.contains(&result.id) {
                        listed.push(result.id);
                    }
                }
                // A name the admission gate refused is expected traffic, not a
                // failure: the extractor returns quantities and paths on most
                // memories, and warning on each would drown the daemon log
                // during a backfill.
                Err(WenlanError::Validation(reason)) => {
                    log::debug!("[post_ingest] entity not admitted: {reason}")
                }
                Err(e) => log::warn!("[post_ingest] entity create failed: {e}"),
            }
        }
        for obs in &kg_item.observations {
            if let Some(entity_id) = entity_cache.get(&obs.entity.to_lowercase()) {
                let req = wenlan_types::requests::AddObservationRequest {
                    entity_id: entity_id.clone(),
                    content: obs.content.clone(),
                    source_agent: Some("post_ingest".to_string()),
                    confidence: None,
                };
                if let Err(e) =
                    crate::post_write::add_observation(db, req, "post_ingest", &ReadScope::Global)
                        .await
                {
                    log::warn!("[post_ingest] add_observation failed: {e}");
                }
            }
        }
        for rel in &kg_item.relations {
            let from_id = entity_cache.get(&rel.from.to_lowercase()).cloned();
            let to_id = entity_cache.get(&rel.to.to_lowercase()).cloned();
            if let (Some(from), Some(to)) = (from_id, to_id) {
                let req = wenlan_types::requests::CreateRelationRequest {
                    preserve_existing: false,
                    from_entity: from,
                    to_entity: to,
                    relation_type: rel.relation_type.clone(),
                    source_agent: Some("post_ingest".to_string()),
                    confidence: rel.confidence,
                    explanation: rel.explanation.clone(),
                    source_memory_id: Some(source_id.to_string()),
                    span: rel.span.clone(),
                    model_version: model_version.map(|s| s.to_string()),
                    prompt_version: Some(
                        crate::extract::EXTRACT_KNOWLEDGE_GRAPH_PROMPT_VERSION.to_string(),
                    ),
                };
                if let Err(e) = crate::post_write::create_relation_with_span(
                    db,
                    req,
                    "post_ingest",
                    Some(content),
                )
                .await
                {
                    log::warn!("[post_ingest] create_relation failed: {e}");
                }
            }
        }
    }

    // UX-GRAPH-8. A hub topic (linked to a large share of all memories) is
    // never linked by extraction: the hub tells the memory apart from
    // nothing, and filing memories under it grew one page from every one of
    // them. A memory that names only hubs gets no topic here and stays
    // unfiled, so the vector link and reweave (which skip hubs) can still
    // file it under something specific. The hub's observations and
    // relations above are kept either way.
    let listed_refs: Vec<&str> = listed.iter().map(String::as_str).collect();
    let hubs = db.hub_entity_ids(&listed_refs).await.unwrap_or_default();
    let ids: Vec<&str> = listed_refs
        .iter()
        .copied()
        .filter(|id| !hubs.contains(*id))
        .collect();

    // Write the extracted entities into the many-to-many junction table.
    // An archived topic the extractor named may be linked (#708 resolves
    // to it on purpose), but is never the main topic.
    if !ids.is_empty() {
        if let Err(e) = db
            .link_memory_entities_as(source_id, &ids, crate::db::topic_filing::writer::KG_EXTRACT)
            .await
        {
            log::warn!("[post_ingest] link_memory_entities failed: {e}");
        }
    }

    // The main topic (1-1 legacy column) is the first shown topic, read
    // after linking: a link can bring an archived topic back (#708).
    let first_entity_id = db
        .active_entity_ids(&ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .next();
    if let Some(ref eid) = first_entity_id {
        let _ = db
            .update_memory_entity_id_as(source_id, eid, crate::db::topic_filing::writer::KG_EXTRACT)
            .await;
    }

    Ok(first_entity_id)
}

/// Extract entities from a single memory via LLM. Returns the primary entity_id if one was created/found.
pub async fn extract_single_memory_entities(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    source_id: &str,
    content: &str,
) -> Result<Option<String>, WenlanError> {
    let kg = extract_kg(llm, prompts, content).await?;
    commit_kg(db, source_id, &kg, content, Some(&llm.model_id())).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_provider::CannedLlmProvider;
    use crate::sources::RawDocument;
    use std::sync::Arc;

    /// Build a test prompt registry using defaults (no override dir needed).
    fn test_prompts() -> PromptRegistry {
        crate::prompts::PromptRegistry::default()
    }

    /// Build a CannedLlmProvider that returns a minimal KG JSON containing
    /// "Alice" as an entity when the extract_knowledge_graph prompt is used.
    fn canned_alice() -> Arc<CannedLlmProvider> {
        let prompts = test_prompts();
        // The key must appear in (or equal) the system_prompt sent by extract_kg.
        // We use a short substring of the prompt as the key.
        let key_fragment = prompts
            .extract_knowledge_graph
            .chars()
            .take(30)
            .collect::<String>();
        let kg_json =
            r#"[{"entities":[{"name":"Alice","type":"person"}],"observations":[],"relations":[]}]"#;
        Arc::new(CannedLlmProvider::new("DEFAULT").with(key_fragment, kg_json))
    }

    #[tokio::test]
    async fn extract_kg_returns_parsed_kg_no_db() {
        let prompts = test_prompts();
        let canned = canned_alice();
        let result = extract_kg(
            &(canned as Arc<dyn LlmProvider>),
            &prompts,
            "Alice joined Acme",
        )
        .await
        .expect("extract_kg should succeed");
        assert!(
            !result.is_empty(),
            "should return at least one KgExtractionResult"
        );
        let first = &result[0];
        assert!(
            first.entities.iter().any(|e| e.name == "Alice"),
            "expected entity 'Alice' in result, got: {:?}",
            first.entities
        );
    }

    #[tokio::test]
    async fn extract_then_commit_creates_and_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        // Seed a memory row so commit_kg's update_memory_entity_id / link_memory_entities have a target.
        let doc = RawDocument {
            source: "memory".to_string(),
            source_id: "m1".to_string(),
            title: "Test memory".to_string(),
            content: "Alice joined Acme".to_string(),
            ..Default::default()
        };
        db.upsert_documents(vec![doc])
            .await
            .expect("upsert_documents failed");

        let prompts = test_prompts();
        let canned = canned_alice();
        let kg = extract_kg(
            &(canned as Arc<dyn LlmProvider>),
            &prompts,
            "Alice joined Acme",
        )
        .await
        .expect("extract_kg failed");

        let eid = commit_kg(&db, "m1", &kg, "Alice joined Acme", None)
            .await
            .expect("commit_kg failed");
        assert!(
            eid.is_some(),
            "expected a primary entity id after commit_kg"
        );

        // Verify the memory is linked to the entity via the legacy 1-1 column.
        let stored_eid = db
            .get_memory_entity_id("m1")
            .await
            .expect("get_memory_entity_id failed");
        assert_eq!(
            stored_eid, eid,
            "stored entity_id should match the one returned by commit_kg"
        );
    }

    /// A topic linked to a large share of all memories is a hub: a new memory
    /// is filed under the first specific topic instead, and is not linked to
    /// the hub at all.
    #[tokio::test]
    async fn ux_graph_8_commit_kg_does_not_file_a_memory_under_a_hub_topic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        for id in ["m1", "m2"] {
            db.upsert_documents(vec![RawDocument {
                source: "memory".to_string(),
                source_id: id.to_string(),
                title: "Test memory".to_string(),
                content: "Alice joined Acme".to_string(),
                ..Default::default()
            }])
            .await
            .expect("upsert_documents failed");
        }
        let prompts = test_prompts();
        let key_fragment = prompts
            .extract_knowledge_graph
            .chars()
            .take(30)
            .collect::<String>();
        let kg_json = r#"[{"entities":[{"name":"Alice","type":"person"},{"name":"Acme","type":"organization"}],"observations":[],"relations":[]}]"#;
        let canned = Arc::new(CannedLlmProvider::new("DEFAULT").with(key_fragment, kg_json));
        let kg = extract_kg(
            &(canned as Arc<dyn LlmProvider>),
            &prompts,
            "Alice joined Acme",
        )
        .await
        .expect("extract_kg failed");
        let first = commit_kg(&db, "m1", &kg, "Alice joined Acme", None)
            .await
            .unwrap()
            .expect("m1 gets a topic");

        // Make the first-listed topic a hub.
        {
            let conn = db.test_primary_session().await;
            for i in 0..crate::db::HUB_MIN_LINKS {
                conn.execute(
                    "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
                    libsql::params![format!("other_{i}"), first.clone()],
                )
                .await
                .unwrap();
            }
        }

        let primary = commit_kg(&db, "m2", &kg, "Alice joined Acme", None)
            .await
            .unwrap()
            .expect("m2 gets a specific topic");
        assert_ne!(primary, first, "the hub is not the memory's topic");
        assert_eq!(
            db.get_memory_entity_id("m2").await.unwrap(),
            Some(primary.clone())
        );
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT entity_id FROM memory_entities WHERE memory_id = 'm2'",
                (),
            )
            .await
            .unwrap();
        let mut linked = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            linked.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(linked, vec![primary], "the hub is not linked");
        drop(rows);
        drop(conn);

        // UX-GRAPH-8: a memory that names only the hub gets no topic at all,
        // so it stays unfiled and the vector link can still file it.
        db.upsert_documents(vec![RawDocument {
            source: "memory".to_string(),
            source_id: "m3".to_string(),
            title: "Test memory".to_string(),
            content: "Alice again".to_string(),
            ..Default::default()
        }])
        .await
        .expect("upsert_documents failed");
        let hub_only = vec![crate::extract::KgExtractionResult {
            index: 0,
            entities: vec![kg[0].entities[0].clone()],
            observations: Vec::new(),
            relations: Vec::new(),
        }];
        assert_eq!(
            commit_kg(&db, "m3", &hub_only, "Alice again", None)
                .await
                .unwrap(),
            None
        );
        assert_eq!(db.get_memory_entity_id("m3").await.unwrap(), None);
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM memory_entities WHERE memory_id = 'm3'",
                (),
            )
            .await
            .unwrap();
        let links = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(links, 0, "the hub is not linked either");
    }

    /// UX-GRAPH-8: an archived topic the extractor names may be linked, but
    /// the memory's main topic is the first shown one, and the links name
    /// their writer.
    #[tokio::test]
    async fn ux_graph_8_commit_kg_never_files_under_an_archived_topic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        for id in ["m1", "m2"] {
            db.upsert_documents(vec![RawDocument {
                source: "memory".to_string(),
                source_id: id.to_string(),
                title: "Test memory".to_string(),
                content: "Alice joined Acme".to_string(),
                ..Default::default()
            }])
            .await
            .expect("upsert_documents failed");
        }
        let kg = vec![crate::extract::KgExtractionResult {
            index: 0,
            entities: vec![
                crate::extract::ExtractedEntity {
                    name: "Alice".to_string(),
                    entity_type: "person".to_string(),
                },
                crate::extract::ExtractedEntity {
                    name: "Acme".to_string(),
                    entity_type: "organization".to_string(),
                },
            ],
            observations: Vec::new(),
            relations: Vec::new(),
        }];
        let alice = commit_kg(&db, "m1", &kg, "Alice joined Acme", None)
            .await
            .unwrap()
            .expect("m1 gets a topic");
        assert!(db.archive_entity(&alice).await.unwrap());

        let primary = commit_kg(&db, "m2", &kg, "Alice joined Acme", None)
            .await
            .unwrap()
            .expect("m2 gets the shown topic");
        assert_ne!(primary, alice, "the archived topic is not the main topic");
        assert_eq!(db.get_memory_entity_id("m2").await.unwrap(), Some(primary));
        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT entity_id, writer FROM memory_entities WHERE memory_id = 'm2' ORDER BY rowid",
                (),
            )
            .await
            .unwrap();
        let mut linked = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            linked.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
        }
        assert_eq!(linked.len(), 2, "both named topics are linked: {linked:?}");
        assert!(
            linked.iter().any(|(id, _)| id == &alice),
            "the archived topic is linked, just not the main topic: {linked:?}"
        );
        assert!(
            linked.iter().all(|(_, writer)| writer == "kg_extract"),
            "{linked:?}"
        );
    }

    #[test]
    fn a_hub_needs_many_links_and_a_large_share() {
        use crate::db::is_hub_entity;
        assert!(is_hub_entity(877, 3700));
        assert!(!is_hub_entity(150, 200), "too few links to be a hub");
        assert!(!is_hub_entity(250, 5000), "a small share is a normal topic");
    }

    #[tokio::test]
    async fn commit_kg_files_new_entities_with_the_source_memory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        db.create_space("work", None, false).await.unwrap();
        let doc = RawDocument {
            source: "memory".to_string(),
            source_id: "m_space".to_string(),
            title: "Space-derived entity".to_string(),
            content: "Space Entity Canary joined the project.".to_string(),
            space: Some("work".to_string()),
            ..Default::default()
        };
        db.upsert_documents(vec![doc]).await.unwrap();

        let kg = vec![crate::extract::KgExtractionResult {
            index: 0,
            entities: vec![crate::extract::ExtractedEntity {
                name: "Space Entity Canary".to_string(),
                entity_type: "person".to_string(),
            }],
            observations: vec![],
            relations: vec![],
        }];
        let entity_id = commit_kg(
            &db,
            "m_space",
            &kg,
            "Space Entity Canary joined the project.",
            None,
        )
        .await
        .unwrap()
        .unwrap();

        let detail = db.get_entity_detail(&entity_id).await.unwrap();
        assert_eq!(
            detail.entity.space.as_deref(),
            Some("work"),
            "new derived entities must inherit the persisted source-memory Space"
        );
    }

    /// The regression this whole gate exists for: the extractor returns
    /// quantities, paths, filenames, shas, and test strings alongside the real
    /// entity, and every one of them used to become a `kind='entity'` row in
    /// `pages`. After the gate only the real entity may reach `pages`; the rest
    /// are dropped and logged, and the commit still succeeds.
    #[tokio::test]
    async fn commit_kg_never_writes_a_junk_name_into_pages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        let content = "Adaptive gap detection computes the median interval between writes; \
                       Alice Chen tuned it to 30 minutes in ~/.claude/CLAUDE.md at bd691cc.";
        let doc = RawDocument {
            source: "memory".to_string(),
            source_id: "m_junk".to_string(),
            title: "junk canary".to_string(),
            content: content.to_string(),
            ..Default::default()
        };
        db.upsert_documents(vec![doc]).await.unwrap();

        let junk = [
            "30 minutes",
            "17%",
            "~/.claude/CLAUDE.md",
            "config.json",
            "bd691cc",
            "v1",
            "burst test",
            ".",
        ];
        let mut entities = vec![crate::extract::ExtractedEntity {
            name: "Alice Chen".to_string(),
            entity_type: "person".to_string(),
        }];
        entities.extend(junk.iter().map(|n| crate::extract::ExtractedEntity {
            name: (*n).to_string(),
            entity_type: "concept".to_string(),
        }));
        let kg = vec![crate::extract::KgExtractionResult {
            index: 0,
            entities,
            observations: vec![],
            relations: vec![],
        }];

        let primary = commit_kg(&db, "m_junk", &kg, content, None)
            .await
            .expect("commit_kg must survive rejected names");
        assert!(
            primary.is_some(),
            "the one real entity should still be committed"
        );

        for name in junk {
            let hits = db.search_entities_by_name(name).await.unwrap();
            assert!(
                hits.is_empty(),
                "junk name {name:?} reached pages as {hits:?}"
            );
        }
        let all = db.list_entities(None, None).await.unwrap();
        assert_eq!(
            all.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["Alice Chen"],
            "only the real entity may exist"
        );
    }

    /// The type the extractor supplies is normalized at the same choke point,
    /// so an off-vocabulary value can never be stored.
    #[tokio::test]
    async fn commit_kg_normalizes_entity_type_to_the_vocabulary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        let content = "Plural Canary and Platform Canary both appeared.";
        let doc = RawDocument {
            source: "memory".to_string(),
            source_id: "m_vocab".to_string(),
            title: "vocab canary".to_string(),
            content: content.to_string(),
            ..Default::default()
        };
        db.upsert_documents(vec![doc]).await.unwrap();

        let kg = vec![crate::extract::KgExtractionResult {
            index: 0,
            entities: vec![
                crate::extract::ExtractedEntity {
                    name: "Plural Canary".to_string(),
                    // Deterministically singularizes.
                    entity_type: "concepts".to_string(),
                },
                crate::extract::ExtractedEntity {
                    name: "Platform Canary".to_string(),
                    // Off-vocabulary, with a reviewed alias: -> technology.
                    entity_type: "platform".to_string(),
                },
            ],
            observations: vec![],
            relations: vec![],
        }];
        commit_kg(&db, "m_vocab", &kg, content, None).await.unwrap();

        let canonicals = db.entity_canonicals().await.unwrap();
        for entity in db.list_entities(None, None).await.unwrap() {
            assert!(
                canonicals.contains(&entity.entity_type),
                "{} stored off-vocabulary type {:?}",
                entity.name,
                entity.entity_type
            );
        }
    }

    /// Minor-1 (M3g stage-a review): the entity-sweep path used to write
    /// `source_memory_id: null` even when the caller had the real source
    /// memory id trivially in hand, leaving the span dead weight (no
    /// promotion path can resolve it). When `source_id` is threaded through,
    /// the payload must carry it.
    #[tokio::test]
    async fn extract_entities_for_content_threads_source_id_into_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .expect("MemoryDB::new failed");
        let content = "Alice works at Acme Corp.";
        let doc = RawDocument {
            source: "memory".to_string(),
            source_id: "mem_sweep".to_string(),
            title: "Test memory".to_string(),
            content: content.to_string(),
            ..Default::default()
        };
        db.upsert_documents(vec![doc])
            .await
            .expect("upsert_documents failed");

        let prompts = test_prompts();
        let key_fragment = prompts
            .extract_knowledge_graph
            .chars()
            .take(30)
            .collect::<String>();
        let kg_json = r#"[{"entities":[{"name":"Alice","type":"person"},{"name":"Acme","type":"org"}],"observations":[],"relations":[{"from":"Alice","to":"Acme","type":"works_on","span":"Alice works at Acme Corp"}]}]"#;
        let canned: Arc<dyn LlmProvider> =
            Arc::new(CannedLlmProvider::new("DEFAULT").with(key_fragment, kg_json));

        extract_entities_for_content(&db, &canned, &prompts, content, Some("mem_sweep"))
            .await
            .expect("extract_entities_for_content failed");

        let conn = db.test_primary_session().await;
        let mut rows = conn
            .query(
                "SELECT e.payload FROM edges e \
                 JOIN entity_page_map fm ON fm.entity_id = e.src_id \
                 JOIN pages fp ON fp.id = fm.page_id \
                     AND fp.kind = 'entity' AND fp.status = 'active' \
                 JOIN entity_page_map tm ON tm.entity_id = e.dst_id \
                 JOIN pages tp ON tp.id = tm.page_id \
                     AND tp.kind = 'entity' AND tp.status = 'active' \
                 WHERE e.edge_type = 'relates' AND fp.title = 'Alice' AND tp.title = 'Acme'",
                libsql::params![],
            )
            .await
            .unwrap();
        let payload_text: String = rows
            .next()
            .await
            .unwrap()
            .expect("relates edge row")
            .get(0)
            .expect(
                "payload must be non-NULL: source_memory_id + model_version + prompt_version are Some",
            );
        let payload: serde_json::Value = serde_json::from_str(&payload_text).unwrap();
        assert_eq!(payload["source_memory_id"], serde_json::json!("mem_sweep"));
    }
}
