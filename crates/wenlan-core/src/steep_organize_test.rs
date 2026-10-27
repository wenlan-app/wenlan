// SPDX-License-Identifier: Apache-2.0
//! Organizing on demand lands exactly what Wenlan's own model lands: the same
//! model reply, through the ambient slice on one database and through a
//! claim's organize tasks on another, leaves the same rows behind.
//!
//! Holds are process-wide, so every test uses its own source ids.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wenlan_types::steep::SteepCodeAnswer;
use wenlan_types::RawDocument;

use super::*;
use crate::db::EnrichmentOrigin;
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::read_scope::ReadScope;
use crate::steep_new::{claim_new, commit_new, organize_new};

/// Entity names under six characters: the cloud test embedder gives every
/// text the same vector, and only longer, varied names merge on distance.
const MIRA_NOTE: &str = "Mira fires the kiln in Oslo every Sunday. She keeps the glaze \
recipes in a red notebook and tests each glaze on a small tile first.";
const KILN_NOTE: &str = "Mira says the kiln needs eight hours to cool before anyone opens it. \
Opening it early cracked two bowls last winter.";

const CLASSIFY_REPLY: &str = r#"{"memory_type": "lesson", "domain": "ceramics", "tags": ["kiln", "glaze"], "quality": "high", "importance": 7}"#;

/// An extraction naming Mira and her kiln. The observation quotes the
/// memory, so two memories never share one (shared observations dedupe by
/// which memory went first).
fn kg_reply(prompt: &str) -> String {
    let fact: String = prompt.trim_start_matches("1. ").chars().take(40).collect();
    serde_json::json!([{
        "i": 1,
        "entities": [{"name": "Mira", "type": "person"}, {"name": "kiln", "type": "technology"}],
        "observations": [{"entity": "Mira", "content": fact}],
        "relations": [{"from": "Mira", "to": "kiln", "type": "uses", "confidence": 0.9,
                       "explanation": "She fires it.", "span": "Mira"}]
    }])
    .to_string()
}

/// Answers by request label, as one small model would, and records each
/// request it saw.
struct Scripted {
    seen: Mutex<Vec<LlmRequest>>,
}

impl Scripted {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }
}

fn reply_for(label: Option<&str>, prompt: &str) -> String {
    match label {
        Some("classify") => CLASSIFY_REPLY.to_string(),
        Some("extract") => kg_reply(prompt),
        other => panic!("unexpected request {other:?}"),
    }
}

#[async_trait::async_trait]
impl LlmProvider for Scripted {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        let reply = reply_for(request.label.as_deref(), &request.user_prompt);
        self.seen.lock().unwrap().push(request);
        Ok(reply)
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "scripted"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::OnDevice
    }
}

async fn memory(db: &MemoryDB, source_id: &str, content: &str) {
    db.upsert_enrichment_origin(
        source_id,
        EnrichmentOrigin {
            memory_type_explicit: false,
            structured_fields_explicit: true,
            space_rejected: false,
        },
    )
    .await
    .unwrap();
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: source_id.to_string(),
        title: source_id.to_string(),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".to_string()),
        source_agent: Some("obsidian".to_string()),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
}

/// Every row the classification and entity lanes write for `prefix`'s
/// memories, with entity ids replaced by entity names (ids are random).
async fn snapshot(db: &MemoryDB, prefix: &str) -> BTreeMap<&'static str, Vec<String>> {
    let like = format!("{prefix}%");
    let queries: [(&'static str, &str); 7] = [
        (
            "memories",
            "SELECT m.source_id || '|' || quote(m.memory_type) || '|' || quote(m.quality) || '|' \
             || quote(m.supersede_mode) || '|' || quote(m.importance) || '|' || quote(m.space) \
             || '|' || quote(m.version) || '|' || quote(ep.title) \
             FROM memories m \
             LEFT JOIN entity_page_map epm ON epm.entity_id = m.entity_id \
             LEFT JOIN pages ep ON ep.id = epm.page_id \
             WHERE m.source = 'memory' AND m.source_id LIKE ?1 ORDER BY 1",
        ),
        (
            "tags",
            "SELECT source_id || '|' || tag FROM document_tags \
             WHERE source = 'memory' AND source_id LIKE ?1 ORDER BY 1",
        ),
        (
            "steps",
            "SELECT source_id || '|' || step_name || '|' || status || '|' || attempts || '|' \
             || quote(input_version) || '|' || quote(error) \
             FROM enrichment_steps WHERE source_id LIKE ?1 ORDER BY 1",
        ),
        (
            "memory_entities",
            "SELECT me.memory_id || '|' || p.title FROM memory_entities me \
             JOIN entity_page_map epm ON epm.entity_id = me.entity_id \
             JOIN pages p ON p.id = epm.page_id \
             WHERE me.memory_id LIKE ?1 ORDER BY 1",
        ),
        (
            "entity_pages",
            "SELECT p.title || '|' || p.status || '|' || COALESCE(p.entity_confirmed, 0) \
             FROM pages p WHERE p.kind = 'entity' AND ?1 IS NOT NULL ORDER BY 1",
        ),
        (
            "relations",
            "SELECT ps.title || '|' || e.edge_type || '|' || pd.title || '|' || e.lineage \
             || '|' || e.grounded || '|' || (e.valid_until IS NULL) \
             FROM edges e \
             JOIN entity_page_map ms ON ms.entity_id = e.src_id \
             JOIN pages ps ON ps.id = ms.page_id \
             JOIN entity_page_map md ON md.entity_id = e.dst_id \
             JOIN pages pd ON pd.id = md.page_id \
             WHERE e.edge_type = 'relates' AND ?1 IS NOT NULL ORDER BY 1",
        ),
        (
            "observations",
            "SELECT o.source_memory_id || '|' || o.content || '|' || p.title \
             FROM observations o \
             JOIN entity_page_map epm ON epm.entity_id = o.entity_id \
             JOIN pages p ON p.id = epm.page_id \
             WHERE o.source_memory_id LIKE ?1 ORDER BY 1",
        ),
    ];
    let session = db.test_primary_session().await;
    let mut out = BTreeMap::new();
    for (name, sql) in queries {
        let mut lines = Vec::new();
        match session.query(sql, libsql::params![like.as_str()]).await {
            Ok(mut rows) => {
                while let Some(row) = rows.next().await.unwrap() {
                    lines.push(row.get::<Option<String>>(0).unwrap().unwrap_or_default());
                }
            }
            // A table a later schema dropped reads the same on both sides.
            Err(_) => lines.push("<no table>".to_string()),
        }
        out.insert(name, lines);
    }
    out
}

/// No vector auto-link (see the parity test).
const NO_AUTO_LINK: f32 = -1.0;

fn options(distance: f32) -> OrganizeOptions {
    OrganizeOptions {
        prompts: PromptRegistry::default(),
        entity_link_distance: distance,
    }
}

/// Run the ambient classification and entity lanes until they have nothing
/// left, as Automatic mode does.
async fn run_ambient(db: &MemoryDB, llm: Arc<Scripted>, distance: f32) {
    let provider: Arc<dyn LlmProvider> = llm;
    let prompts = PromptRegistry::default();
    while crate::ingest::run_classification_enrichment_slice(db, &provider, &prompts)
        .await
        .unwrap()
        .selected
    {}
    loop {
        let provider = provider.clone();
        let prompts = prompts.clone();
        let selected = db
            .run_entity_enrichment_slice_with_auto_link(distance, move |content: String| {
                let provider = provider.clone();
                let prompts = prompts.clone();
                async move {
                    crate::kg::entity_extraction::extract_kg(&provider, &prompts, &content).await
                }
            })
            .await
            .unwrap();
        if selected == 0 {
            break;
        }
    }
}

/// The small model's answers to `work`, from the same script.
fn answers(work: &SteepOrganizeWork) -> Vec<SteepCodeAnswer> {
    work.tasks
        .iter()
        .map(|task| SteepCodeAnswer {
            n: task.n,
            answer: reply_for(
                Some(match task.stage.as_str() {
                    STEEP_ORGANIZE_STAGE_CLASSIFY => "classify",
                    _ => "extract",
                }),
                &task.prompt,
            ),
        })
        .collect()
}

#[tokio::test]
async fn organize_lands_the_same_rows_as_the_ambient_lanes() {
    // A negative distance keeps the vector auto-link out of both runs: the
    // test embedder puts every text at (about) distance 0 from every entity,
    // so the link would depend on which memory went first.
    let distance = NO_AUTO_LINK;
    let (automatic, _a) = crate::db::tests::test_db().await;
    let (on_demand, _b) = crate::db::tests::test_db().await;
    for db in [&automatic, &on_demand] {
        memory(db, "par_mira", MIRA_NOTE).await;
        memory(db, "par_kiln", KILN_NOTE).await;
    }

    let script = Scripted::new();
    run_ambient(&automatic, script.clone(), distance).await;

    let claim = claim_new(
        &on_demand,
        &ReadScope::Global,
        &["par_mira".into(), "par_kiln".into()],
        Some(&options(distance)),
    )
    .await
    .unwrap();
    let work = claim
        .organize
        .clone()
        .expect("both sources wait on organizing");
    assert_eq!(work.tasks.len(), 4, "classify and entities for each source");
    assert_eq!(
        work.stages
            .iter()
            .map(|s| s.stage.as_str())
            .collect::<Vec<_>>(),
        vec![STEEP_ORGANIZE_STAGE_CLASSIFY, STEEP_ORGANIZE_STAGE_ENTITIES]
    );

    // The tasks are the requests the lanes sent, word for word.
    let seen = script.seen.lock().unwrap().clone();
    for task in &work.tasks {
        let label = match task.stage.as_str() {
            STEEP_ORGANIZE_STAGE_CLASSIFY => "classify",
            _ => "extract",
        };
        let stage = work.stages.iter().find(|s| s.stage == task.stage).unwrap();
        assert!(
            seen.iter()
                .any(|request| request.label.as_deref() == Some(label)
                    && request.user_prompt == task.prompt
                    && request.system_prompt.as_deref() == Some(stage.instructions.as_str())),
            "task {} ({}) is not a request the ambient lane sent: {:?}\nseen: {:?}",
            task.n,
            task.stage,
            task.prompt,
            seen.iter()
                .map(|r| (r.label.clone(), r.user_prompt.clone()))
                .collect::<Vec<_>>()
        );
    }

    let done = organize_new(&on_demand, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert_eq!(done.classified, 2);
    assert_eq!(done.detected, 2);
    assert_eq!(done.linked, 2);
    assert_eq!(done.remaining, 0);
    assert_eq!(done.failed + done.stale, 0);

    let automatic_rows = snapshot(&automatic, "par_").await;
    let on_demand_rows = snapshot(&on_demand, "par_").await;
    assert_eq!(automatic_rows, on_demand_rows);
    assert!(
        automatic_rows["memory_entities"]
            .iter()
            .any(|row| row == "par_mira|Mira"),
        "the parity is over real rows: {automatic_rows:?}"
    );
    assert!(!automatic_rows["tags"].is_empty());
}

#[tokio::test]
async fn claim_planning_does_not_wait_for_the_shared_embedder() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(
        &db,
        "claim_busy_embedder",
        "A long uncached memory about a ceramic kiln, glaze tests, and a notebook.",
    )
    .await;
    db.clear_embedding_cache_for_test();

    let embedder = db.shared_embedder_for_test().expect("test embedder");
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let busy_worker = std::thread::spawn(move || {
        let _guard = embedder.lock().unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(750));
    });
    locked_rx.recv().unwrap();

    let started = Instant::now();
    let tasks = plan(&db, &options(0.8), &["claim_busy_embedder".to_string()])
        .await
        .unwrap();
    let elapsed = started.elapsed();
    println!("claim organize plan with busy embedder: {elapsed:?}");

    assert!(
        elapsed < Duration::from_millis(300),
        "claim planning waited for the shared embedder: {elapsed:?}"
    );
    assert!(
        tasks
            .iter()
            .any(|task| task.stage() == STEEP_ORGANIZE_STAGE_ENTITIES),
        "a busy embedder must leave entity extraction pending"
    );
    busy_worker.join().unwrap();
}

#[tokio::test]
async fn an_answer_lands_once_and_unknown_numbers_are_reported() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "once_a", MIRA_NOTE).await;
    memory(&db, "once_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["once_a".into(), "once_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.unwrap();
    let first = work.tasks[0].clone();
    let one = vec![SteepCodeAnswer {
        n: first.n,
        answer: CLASSIFY_REPLY.into(),
    }];
    let done = organize_new(&db, &claim.token, &one).await.unwrap();
    assert_eq!(done.classified, 1);
    assert_eq!(done.remaining, 3);

    let mut again = one.clone();
    again.push(SteepCodeAnswer {
        n: 99,
        answer: CLASSIFY_REPLY.into(),
    });
    let done = organize_new(&db, &claim.token, &again).await.unwrap();
    assert_eq!(done.classified, 0);
    assert_eq!(done.unknown, vec![first.n, 99]);

    assert!(matches!(
        organize_new(&db, "no-such-token", &one).await,
        Err(crate::steep::SteepError::TokenGone)
    ));
}

#[tokio::test]
async fn a_memory_changed_since_the_claim_takes_no_answer() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "stale_a", MIRA_NOTE).await;
    memory(&db, "stale_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["stale_a".into(), "stale_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    db.update_memory("stale_a", "Mira moved the kiln to Bergen.")
        .await
        .unwrap();

    let done = organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert_eq!(done.stale, 2, "both of stale_a's answers: {done:?}");
    assert_eq!(done.classified, 1);
    assert_eq!(done.detected, 1);
    let rows = snapshot(&db, "stale_a").await;
    assert!(
        !rows["steps"]
            .iter()
            .any(|row| row.starts_with("stale_a|classify|ok|")),
        "no receipt for the old version: {rows:?}"
    );
    assert!(rows["memory_entities"].is_empty(), "{rows:?}");
}

#[tokio::test]
async fn an_unreadable_classification_waits_for_a_retry() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "bad_a", MIRA_NOTE).await;
    memory(&db, "bad_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["bad_a".into(), "bad_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.unwrap();
    let bad: Vec<SteepCodeAnswer> = work
        .tasks
        .iter()
        .filter(|task| task.stage == STEEP_ORGANIZE_STAGE_CLASSIFY)
        .map(|task| SteepCodeAnswer {
            n: task.n,
            answer: "I think this is a lesson about kilns.".into(),
        })
        .collect();
    let done = organize_new(&db, &claim.token, &bad).await.unwrap();
    assert_eq!(done.failed, 2);
    let rows = snapshot(&db, "bad_").await;
    assert_eq!(
        rows["steps"]
            .iter()
            .filter(|row| row.contains("|classify|needs_retry|1|1|"))
            .count(),
        2,
        "{rows:?}"
    );
    // Still waiting, so the next claim offers it again.
    assert!(db
        .get_classification_candidate_for("bad_a", crate::post_ingest::ENRICHMENT_MAX_ATTEMPTS)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn without_organize_options_a_claim_hands_out_no_work() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "auto_a", MIRA_NOTE).await;
    memory(&db, "auto_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["auto_a".into(), "auto_b".into()],
        None,
    )
    .await
    .unwrap();
    assert!(claim.organize.is_none());
    let rows = snapshot(&db, "auto_").await;
    assert!(rows["steps"].is_empty(), "nothing ran: {rows:?}");
}

#[tokio::test]
async fn the_note_is_about_the_entity_most_sources_mention_and_links_its_pages() {
    let (db, _dir) = crate::db::tests::test_db().await;
    // An existing note about Mira's kiln, citing a memory that names her.
    memory(&db, "link_old", MIRA_NOTE).await;
    let old = claim_new(
        &db,
        &ReadScope::Global,
        &["link_old".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await;
    // One short source is too little for a note; organize it by hand.
    assert!(old.is_err());
    let pending = match db
        .entity_enrichment_for("link_old", NO_AUTO_LINK)
        .await
        .unwrap()
    {
        Some(crate::db::EntityEnrichmentPrep::NeedsExtraction(pending)) => pending,
        other => panic!("link_old waits for extraction: {other:?}"),
    };
    let kg = crate::kg::entity_extraction::parse_kg_output(
        &pending.content,
        &kg_reply(&pending.content),
    );
    assert!(db.apply_entity_extraction(&pending, Ok(kg)).await.unwrap());
    let existing = crate::post_write::page_write(
        &db,
        crate::post_write::PageWrite::Create {
            page_id: None,
            req: wenlan_types::requests::CreateConceptRequest {
                title: "Kiln care".into(),
                content: "Mira fires the kiln in Oslo every Sunday [1].".into(),
                summary: None,
                entity_id: None,
                space: wenlan_types::WriteSpaceTarget::Uncategorized,
                source_memory_ids: vec!["link_old".into()],
                creation_kind: Some("authored".into()),
                workspace: None,
            },
            agent: "test",
            knowledge_path: None,
            page_min_cluster_size: 1,
            page_match_threshold: 2.0,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
            skip_protected_title_match: false,
        },
    )
    .await
    .unwrap();

    memory(&db, "link_a", MIRA_NOTE).await;
    memory(&db, "link_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["link_a".into(), "link_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    assert!(claim.link_targets.is_empty(), "nothing organized yet");
    let work = claim.organize.clone().unwrap();
    let done = organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert_eq!(done.linked, 2);
    assert_eq!(done.link_targets, vec!["Kiln care".to_string()]);

    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2]. See [[Kiln care]].";
    let written = commit_new(&db, &claim.token, "Mira's kiln", body, "test", None)
        .await
        .unwrap();
    assert_eq!(written.outcome, "created");
    let page = db
        .get_page(written.page_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    let mira = db
        .source_entity_links(&["link_a".to_string()])
        .await
        .unwrap()
        .into_iter()
        .map(|(_, entity, _)| entity)
        .collect::<Vec<_>>();
    assert!(
        page.entity_id.as_ref().is_some_and(|id| mira.contains(id)),
        "the note is about an entity both sources mention: {:?}",
        page.entity_id
    );
    let confirmed = db
        .source_entity_links(&["link_a".to_string()])
        .await
        .unwrap()
        .into_iter()
        .any(|(_, entity, confirmed)| Some(&entity) == page.entity_id.as_ref() && confirmed);
    assert!(confirmed, "a cited entity earns its Wiki page");
    let links = db
        .get_page_outbound_links_scoped(&page.id, &ReadScope::Global)
        .await
        .unwrap();
    assert!(
        links
            .iter()
            .any(|link| link.target_page_id.as_deref() == Some(existing.id.as_str())),
        "the [[link]] resolves to the existing note: {links:?}"
    );
}

#[tokio::test]
async fn a_note_is_about_the_next_majority_entity_when_the_first_has_a_note() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "next_a", MIRA_NOTE).await;
    memory(&db, "next_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["next_a".into(), "next_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    let entity = |name: &'static str| {
        let db = &db;
        async move {
            db.search_entities_by_name(name)
                .await
                .unwrap()
                .into_iter()
                .find(|e| e.name == name)
                .unwrap()
                .id
        }
    };
    let (mira, kiln) = (entity("Mira").await, entity("kiln").await);
    // Mira already has a note. An authored one: the page match never folds
    // a new note into it, so this note is written either way.
    crate::post_write::page_write(
        &db,
        crate::post_write::PageWrite::Create {
            page_id: None,
            req: wenlan_types::requests::CreateConceptRequest {
                title: "About Mira".into(),
                content: "Mira is a potter in Oslo.".into(),
                summary: None,
                entity_id: Some(mira.clone()),
                space: wenlan_types::WriteSpaceTarget::Uncategorized,
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".into()),
                workspace: None,
            },
            agent: "test",
            knowledge_path: None,
            page_min_cluster_size: 0,
            page_match_threshold: 2.0,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
            skip_protected_title_match: false,
        },
    )
    .await
    .unwrap();

    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2].";
    let written = commit_new(&db, &claim.token, "Kiln days", body, "test", None)
        .await
        .unwrap();
    assert_eq!(written.outcome, "created");
    let page = db
        .get_page(written.page_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        page.entity_id.as_deref(),
        Some(kiln.as_str()),
        "Mira has a note, so this one is about the kiln both sources also mention"
    );
}

/// A plain-folder file is never classified by the memory lane, in Automatic
/// mode either (the document lane owns it), so organizing hands out only its
/// entity task, and `classified` stays 0 while its entities still land.
#[tokio::test]
async fn a_folder_file_is_organized_for_entities_only() {
    let (db, _dir) = crate::db::tests::test_db().await;
    for (id, content) in [
        ("orgf::notes/mira.md", MIRA_NOTE),
        ("orgf::notes/kiln.md", KILN_NOTE),
    ] {
        db.upsert_documents(vec![RawDocument {
            source: "memory".to_string(),
            source_id: id.to_string(),
            title: id.to_string(),
            content: content.to_string(),
            last_modified: chrono::Utc::now().timestamp(),
            source_agent: Some("folder".to_string()),
            content_hash: Some(format!("hash-{id}")),
            ..Default::default()
        }])
        .await
        .unwrap();
    }
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["orgf::notes/mira.md".into(), "orgf::notes/kiln.md".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().expect("entity detection waits");
    assert!(
        work.tasks
            .iter()
            .all(|task| task.stage == STEEP_ORGANIZE_STAGE_ENTITIES),
        "no classification task for a folder file: {:?}",
        work.tasks
    );
    assert_eq!(work.tasks.len(), 2);
    let done = organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert_eq!(done.classified, 0);
    assert_eq!(done.detected, 2);
    assert_eq!(done.linked, 2);
}

/// An authored note in the unfiled Space, about `entity_id` when given.
async fn authored_note(db: &MemoryDB, title: &str, entity_id: Option<String>) -> String {
    authored_note_in(db, title, entity_id, None).await
}

/// [`authored_note`] in the Space named `space` (created first), or unfiled.
async fn authored_note_in(
    db: &MemoryDB,
    title: &str,
    entity_id: Option<String>,
    space: Option<&str>,
) -> String {
    if let Some(space) = space {
        if db.get_space(space).await.unwrap().is_none() {
            db.create_space(space, None, false).await.unwrap();
        }
    }
    crate::post_write::page_write(
        db,
        crate::post_write::PageWrite::Create {
            page_id: None,
            req: wenlan_types::requests::CreateConceptRequest {
                title: title.into(),
                content: format!("{title} is a subject of these notes."),
                summary: None,
                entity_id,
                space: match space {
                    Some(space) => wenlan_types::WriteSpaceTarget::Named(space.into()),
                    None => wenlan_types::WriteSpaceTarget::Uncategorized,
                },
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".into()),
                workspace: space.map(str::to_string),
            },
            agent: "test",
            knowledge_path: None,
            page_min_cluster_size: 0,
            page_match_threshold: 2.0,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
            skip_protected_title_match: false,
        },
    )
    .await
    .unwrap()
    .id
}

/// A group whose sources are mostly about an entity that already has a note
/// goes into that note instead of a second one: `links` sees the note
/// written after the claim, and `attach` adds the sources to it and leaves
/// it waiting for a rewrite that uses them.
#[tokio::test]
async fn a_group_about_a_subject_with_a_note_attaches_to_it() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "att_a", MIRA_NOTE).await;
    memory(&db, "att_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["att_a".into(), "att_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    assert!(claim.existing_page.is_none(), "nothing organized yet");
    let work = claim.organize.clone().unwrap();
    let done = organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert!(done.existing_page.is_none(), "no note about Mira yet");

    // An earlier group of this run writes the note about Mira meanwhile.
    let mira = db
        .search_entities_by_name("Mira")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.name == "Mira")
        .unwrap()
        .id;
    let note = authored_note(&db, "About Mira", Some(mira)).await;
    let links = crate::steep_new::links_new(&db, &claim.token)
        .await
        .unwrap();
    assert!(
        links.link_targets.is_empty(),
        "the note the sources go into is no link target: {:?}",
        links.link_targets
    );
    let existing = links.existing_page.expect("the note about Mira");
    assert_eq!(existing.page_id, note);
    assert_eq!(existing.title, "About Mira");

    let attached = crate::steep_new::attach_new(&db, &claim.token, "test")
        .await
        .unwrap();
    assert_eq!(attached.outcome, "attached");
    assert_eq!(attached.title.as_deref(), Some("About Mira"));
    let page = db.get_page(&note).await.unwrap().unwrap();
    for id in ["att_a", "att_b"] {
        assert!(
            page.source_memory_ids.iter().any(|s| s == id),
            "{id} is a source of the note now: {:?}",
            page.source_memory_ids
        );
    }
    assert_eq!(
        page.stale_reason.as_deref(),
        Some("source_updated"),
        "the note waits for a rewrite with its new sources"
    );
    let left = crate::steep_new::new_material(&db, &ReadScope::Global)
        .await
        .unwrap();
    assert!(
        !left
            .iter()
            .any(|m| m.source_ids.iter().any(|s| s.starts_with("att_"))),
        "the attached sources are in a page now"
    );
    assert!(matches!(
        crate::steep_new::links_new(&db, &claim.token).await,
        Err(crate::steep::SteepError::TokenGone)
    ));
}

/// A topic that names an existing note, years and versions aside, offers
/// that note. Without one, `attach` refuses and keeps the claim, so the
/// page can still be written.
#[tokio::test]
async fn a_topic_naming_an_existing_note_offers_it() {
    let (db, _dir) = crate::db::tests::test_db().await;
    let note = authored_note(&db, "Kiln renovation", None).await;
    memory(&db, "topic_a", MIRA_NOTE).await;
    memory(&db, "topic_b", KILN_NOTE).await;
    let claim = crate::steep_new::claim_new_on(
        &db,
        &ReadScope::Global,
        &["topic_a".into(), "topic_b".into()],
        None,
        Some("Kiln Renovation 2024 (v2)"),
    )
    .await
    .unwrap();
    let existing = claim.existing_page.expect("the title matches");
    assert_eq!(existing.page_id, note);
    crate::steep_new::release_new(&claim.token).unwrap();

    let other = crate::steep_new::claim_new_on(
        &db,
        &ReadScope::Global,
        &["topic_a".into(), "topic_b".into()],
        None,
        Some("Kiln glazes"),
    )
    .await
    .unwrap();
    assert!(other.existing_page.is_none());
    let refused = crate::steep_new::attach_new(&db, &other.token, "test").await;
    assert!(matches!(refused, Err(crate::steep::SteepError::Invalid(_))));
    assert!(
        crate::steep_new::links_new(&db, &other.token).await.is_ok(),
        "the claim is kept for the write"
    );
    crate::steep_new::release_new(&other.token).unwrap();
}

/// The helper needs only the token: it fetches the claim's open tasks
/// itself, and none are left once their answers land. A caller locked to
/// another Space reads nothing.
#[tokio::test]
async fn the_helper_fetches_the_open_tasks_with_the_token_alone() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "tasks_a", MIRA_NOTE).await;
    memory(&db, "tasks_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["tasks_a".into(), "tasks_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let fetched = crate::steep_new::organize_tasks_new(&claim.token, &ReadScope::Global)
        .unwrap()
        .expect("tasks wait");
    assert_eq!(Some(&fetched), claim.organize.as_ref());
    assert!(matches!(
        crate::steep_new::organize_tasks_new(&claim.token, &ReadScope::Space("elsewhere".into())),
        Err(crate::steep::SteepError::TokenGone)
    ));
    organize_new(&db, &claim.token, &answers(&fetched))
        .await
        .unwrap();
    assert!(
        crate::steep_new::organize_tasks_new(&claim.token, &ReadScope::Global)
            .unwrap()
            .is_none(),
        "every task was answered"
    );
    crate::steep_new::release_new(&claim.token).unwrap();
}

/// A dated note about an entity its sources mention becomes that entity's
/// page without losing the date from its strict title.
#[tokio::test]
async fn a_dated_note_about_an_entity_keeps_its_strict_title() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "stub_a", MIRA_NOTE).await;
    memory(&db, "stub_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["stub_a".into(), "stub_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    let kiln = db
        .search_entities_by_name("kiln")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.name == "kiln")
        .unwrap()
        .id;

    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2].";
    let written = commit_new(&db, &claim.token, "Kiln 2024", body, "test", None)
        .await
        .unwrap();
    assert_eq!(written.outcome, "created");
    assert_eq!(
        written.title.as_deref(),
        Some("Kiln 2024"),
        "the loose entity match must not rewrite committed title digits"
    );
    let page_id = written.page_id.unwrap();
    let page = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(page.title, "Kiln 2024");
    assert_eq!(page.entity_id.as_deref(), Some(kiln.as_str()));
    let established = db
        .source_entity_links(&["stub_b".to_string()])
        .await
        .unwrap()
        .into_iter()
        .any(|(_, entity, confirmed)| entity == kiln && confirmed);
    assert!(
        established,
        "the dated note still establishes the entity it is about"
    );

    let listed: Vec<_> = db
        .notes_titled("Kiln 2024")
        .await
        .unwrap()
        .into_iter()
        .map(|(id, ..)| id)
        .collect();
    assert_eq!(listed, vec![page_id], "one note keeps the dated title");
}

const ONBOARDING_A: &str = "The onboarding v2 setup checklist now asks new staff to enable \
two-factor sign-in on day one and to book a laptop pickup before the first standup.";
const ONBOARDING_B: &str = "Week two of the onboarding v2 setup checklist covers the billing \
dashboard tour and a shadowing session with the support rotation lead.";

/// Answers naming one entity, "Onboarding v2 - Setup Checklist".
fn onboarding_answers(work: &SteepOrganizeWork) -> Vec<SteepCodeAnswer> {
    work.tasks
        .iter()
        .map(|task| SteepCodeAnswer {
            n: task.n,
            answer: if task.stage == STEEP_ORGANIZE_STAGE_CLASSIFY {
                CLASSIFY_REPLY.to_string()
            } else {
                let fact: String = task
                    .prompt
                    .trim_start_matches("1. ")
                    .chars()
                    .take(40)
                    .collect();
                serde_json::json!([{
                    "i": 1,
                    "entities": [{"name": "Onboarding v2 - Setup Checklist", "type": "project"}],
                    "observations": [{"entity": "Onboarding v2 - Setup Checklist", "content": fact}],
                    "relations": []
                }])
                .to_string()
            },
        })
        .collect()
}

/// A page that carries an entity it is not about (most of its sources
/// mentioned it in passing) is no home for that entity's material: the
/// onboarding notes do not go into "Billing Migration" just because that
/// page is tagged with the onboarding checklist.
#[tokio::test]
async fn a_page_tagged_with_an_entity_it_is_not_about_takes_none_of_its_material() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "onb_a", ONBOARDING_A).await;
    memory(&db, "onb_b", ONBOARDING_B).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["onb_a".into(), "onb_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    organize_new(&db, &claim.token, &onboarding_answers(&work))
        .await
        .unwrap();
    let onboarding = db
        .search_entities_by_name("Onboarding v2 - Setup Checklist")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.name == "Onboarding v2 - Setup Checklist")
        .unwrap()
        .id;
    let billing = authored_note(&db, "Billing Migration", Some(onboarding)).await;

    let links = crate::steep_new::links_new(&db, &claim.token)
        .await
        .unwrap();
    assert!(
        links.existing_page.is_none(),
        "Billing Migration is not about onboarding: {:?}",
        links.existing_page
    );
    let body = "The onboarding v2 setup checklist now asks new staff to enable two-factor \
                sign-in on day one [1]. Week two of the onboarding v2 setup checklist covers \
                the billing dashboard tour [2].";
    let written = commit_new(
        &db,
        &claim.token,
        "Onboarding checklist",
        body,
        "test",
        None,
    )
    .await
    .unwrap();
    assert_eq!(written.outcome, "created");
    assert_ne!(written.page_id.as_deref(), Some(billing.as_str()));
    let page = db
        .get_page(written.page_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.entity_id, None, "the entity already has a page");
}

/// A note written under the title of a note already in the Space goes into
/// that note: one "Recipes", not two.
#[tokio::test]
async fn a_note_under_an_existing_title_goes_into_that_note() {
    let (db, _dir) = crate::db::tests::test_db().await;
    authored_note(&db, "Glaze recipes", None).await;
    let recipes = authored_note(&db, "Recipes", None).await;
    memory(&db, "rec_old", MIRA_NOTE).await;
    crate::post_write::page_write(
        &db,
        crate::post_write::PageWrite::Attach {
            page_id: &recipes,
            source_memory_ids: &["rec_old".into()],
            link_reason: "test",
            agent: "test",
        },
    )
    .await
    .unwrap();
    memory(
        &db,
        "rec_a",
        "Family recipes keep weeknight meals repeatable.",
    )
    .await;
    memory(
        &db,
        "rec_b",
        "The recipe collection also records holiday dinners.",
    )
    .await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["rec_a".into(), "rec_b".into()],
        None,
    )
    .await
    .unwrap();
    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2].";
    let written = commit_new(&db, &claim.token, "recipes", body, "test", None)
        .await
        .unwrap();
    assert_eq!(written.outcome, "attached");
    assert_eq!(written.page_id.as_deref(), Some(recipes.as_str()));
    assert_eq!(written.title.as_deref(), Some("Recipes"));
    assert_eq!(
        written.link_targets,
        vec!["Glaze recipes".to_string()],
        "post-attach hints use the page's full accumulated source set"
    );
}

#[tokio::test]
async fn organize_commit_keeps_dated_titles_separate() {
    let (db, _dir) = crate::db::tests::test_db().await;
    let earlier = authored_note(&db, "Standup 2026-10-26", None).await;
    memory(&db, "dated_commit_a", MIRA_NOTE).await;
    memory(&db, "dated_commit_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["dated_commit_a".into(), "dated_commit_b".into()],
        None,
    )
    .await
    .unwrap();
    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2].";

    let written = commit_new(&db, &claim.token, "Standup 2026-10-27", body, "test", None)
        .await
        .unwrap();

    assert_eq!(written.outcome, "created");
    assert_ne!(written.page_id.as_deref(), Some(earlier.as_str()));
    assert_eq!(db.list_pages("active", 10, 0).await.unwrap().len(), 2);
}

#[tokio::test]
async fn claim_time_suggestion_still_uses_the_loose_title_match() {
    let (db, _dir) = crate::db::tests::test_db().await;
    let lisbon = authored_note(&db, "Lisbon trip 2026", None).await;
    memory(&db, "loose_suggestion_a", MIRA_NOTE).await;
    memory(&db, "loose_suggestion_b", KILN_NOTE).await;

    let claim = crate::steep_new::claim_new_on(
        &db,
        &ReadScope::Global,
        &["loose_suggestion_a".into(), "loose_suggestion_b".into()],
        None,
        Some("Lisbon trip 2025"),
    )
    .await
    .unwrap();

    assert_eq!(
        claim.existing_page.map(|page| page.page_id).as_deref(),
        Some(lisbon.as_str())
    );
    crate::steep_new::release_new(&claim.token).unwrap();
}

/// A host wrote "Running" next to an existing "Running": it
/// claimed a group without naming its topic, called `links`, and wrote
/// under the existing note's title. The note is in another Space than the
/// claim (its sources are unfiled), so a same-Space check missed it. The
/// write goes into that note, and only one note has the title.
#[tokio::test]
async fn a_write_under_an_existing_title_in_another_space_goes_into_that_note() {
    let (db, _dir) = crate::db::tests::test_db().await;
    let running = authored_note_in(&db, "Running", None, Some("Personal")).await;
    memory(&db, "dup_a", MIRA_NOTE).await;
    memory(&db, "dup_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["dup_a".into(), "dup_b".into()],
        None,
    )
    .await
    .unwrap();
    assert!(
        claim.existing_page.is_none(),
        "no topic, no entity: nothing to offer"
    );
    let links = crate::steep_new::links_new(&db, &claim.token)
        .await
        .unwrap();
    assert!(links.existing_page.is_none());
    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2].";
    let written = commit_new(&db, &claim.token, "running", body, "test", None)
        .await
        .unwrap();
    assert_eq!(written.outcome, "attached");
    assert_eq!(written.page_id.as_deref(), Some(running.as_str()));
    assert_eq!(written.title.as_deref(), Some("Running"));
    assert_eq!(
        db.notes_titled("Running").await.unwrap().len(),
        1,
        "one note under the title"
    );

    // Named as the group's topic, the note is offered before the write.
    memory(&db, "dup_c", MIRA_NOTE).await;
    memory(&db, "dup_d", KILN_NOTE).await;
    let named = crate::steep_new::claim_new_on(
        &db,
        &ReadScope::Global,
        &["dup_c".into(), "dup_d".into()],
        None,
        Some("Running 2026"),
    )
    .await
    .unwrap();
    assert_eq!(
        named.existing_page.map(|page| page.page_id).as_deref(),
        Some(running.as_str())
    );
    crate::steep_new::release_new(&named.token).unwrap();
}

/// `link_targets` holds the notes the sources' text names, as the shortest
/// label that resolves from the new note's Space.
#[tokio::test]
async fn link_targets_are_the_notes_the_sources_name() {
    let (db, _dir) = crate::db::tests::test_db().await;
    authored_note(&db, "Glaze recipes", None).await;
    authored_note_in(&db, "Oslo", None, Some("Travel")).await;
    authored_note(&db, "Sourdough", None).await;
    memory(&db, "lt_a", MIRA_NOTE).await;
    memory(&db, "lt_b", KILN_NOTE).await;
    db.create_space("Studio", None, false).await.unwrap();
    db.update_memory_space("lt_a", "Studio").await.unwrap();
    db.update_memory_space("lt_b", "Studio").await.unwrap();
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["lt_a".into(), "lt_b".into()],
        None,
    )
    .await
    .unwrap();
    let mut targets = claim.link_targets.clone();
    targets.sort();
    assert_eq!(
        targets,
        vec!["Glaze recipes".to_string(), "Oslo".to_string()],
        "the unfiled note and the uniquely titled note in another Space both use bare labels"
    );
    let body = "Mira fires the kiln in [[Oslo]] every Sunday [1]. Mira says the \
                kiln needs eight hours to cool before anyone opens it [2]. See \
                [[Glaze recipes]].";
    let written = commit_new(&db, &claim.token, "Kiln cooling", body, "test", None)
        .await
        .unwrap();
    assert!(
        written.link_targets.is_empty(),
        "commit must not re-offer links already used: {:?}",
        written.link_targets
    );
    let links = db
        .get_page_outbound_links_scoped(written.page_id.as_deref().unwrap(), &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!(
        links
            .iter()
            .filter(|link| link.target_page_id.is_some())
            .count(),
        2,
        "both labels resolve: {links:?}"
    );
}

#[tokio::test]
async fn commit_returns_only_unused_link_targets_and_never_the_page_itself() {
    let (db, _dir) = crate::db::tests::test_db().await;
    authored_note(&db, "Glaze recipes", None).await;
    memory(&db, "unused_link_a", MIRA_NOTE).await;
    memory(&db, "unused_link_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["unused_link_a".into(), "unused_link_b".into()],
        None,
    )
    .await
    .unwrap();
    assert!(claim.link_targets.contains(&"Glaze recipes".to_string()));

    let written = commit_new(
        &db,
        &claim.token,
        "Mira",
        "Mira fires the kiln in Oslo every Sunday and keeps glaze recipes in a red notebook [1]. \
         Mira says the kiln needs eight hours to cool before anyone opens it [2].",
        "test",
        None,
    )
    .await
    .unwrap();

    assert!(written.link_targets.contains(&"Glaze recipes".to_string()));
    assert!(!written.link_targets.contains(&"Mira".to_string()));
}

/// "Lisbon and Porto 2026" from sources that mostly mention Lisbon is about
/// both cities, so about no one entity; here Mira stands in for Lisbon.
#[tokio::test]
async fn a_title_joining_two_names_is_about_no_entity() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "two_a", MIRA_NOTE).await;
    memory(&db, "two_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["two_a".into(), "two_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    let body = "Mira fires the kiln in Oslo every Sunday [1]. Mira says the kiln needs \
                eight hours to cool before anyone opens it [2].";
    let written = commit_new(&db, &claim.token, "Mira and Tove 2026", body, "test", None)
        .await
        .unwrap();
    assert_eq!(written.outcome, "created");
    let page = db
        .get_page(written.page_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.entity_id, None);
}

/// On-demand organizing files a memory only into a Space the user already
/// has, as Automatic does (both land through
/// `ingest::apply_classification_output`): a classifier `domain` that names
/// no Space creates none and leaves the memory unfiled.
#[tokio::test]
async fn organizing_files_into_existing_spaces_only_and_creates_none() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "sp_a", MIRA_NOTE).await;
    memory(&db, "sp_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["sp_a".into(), "sp_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    let done = organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert_eq!(done.classified, 2);
    assert_eq!(db.get_memory_space("sp_a").await.unwrap(), None);
    assert!(
        db.get_space("ceramics").await.unwrap().is_none(),
        "the domain the classifier named is no Space"
    );
    crate::steep_new::release_new(&claim.token).unwrap();

    // A Space the user made with that name takes the memory.
    db.create_space("ceramics", None, false).await.unwrap();
    memory(&db, "sp_c", MIRA_NOTE).await;
    memory(&db, "sp_d", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["sp_c".into(), "sp_d".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    assert_eq!(
        db.get_memory_space("sp_c").await.unwrap().as_deref(),
        Some("ceramics")
    );
    crate::steep_new::release_new(&claim.token).unwrap();
}

/// What Claude Code saw: `links` answered `{}` for every group, because the
/// notes that shared its entities were in other Spaces (one per group, after
/// classification) and only same-Space notes counted. A note in another
/// Space that shares an entity is offered as `Space/Title`.
#[tokio::test]
async fn a_note_in_another_space_that_shares_an_entity_is_a_link_target() {
    let (db, _dir) = crate::db::tests::test_db().await;
    db.create_space("Studio", None, false).await.unwrap();
    memory(&db, "xs_old", MIRA_NOTE).await;
    let pending = match db
        .entity_enrichment_for("xs_old", NO_AUTO_LINK)
        .await
        .unwrap()
    {
        Some(crate::db::EntityEnrichmentPrep::NeedsExtraction(pending)) => pending,
        other => panic!("xs_old waits for extraction: {other:?}"),
    };
    let kg = crate::kg::entity_extraction::parse_kg_output(
        &pending.content,
        &kg_reply(&pending.content),
    );
    assert!(db.apply_entity_extraction(&pending, Ok(kg)).await.unwrap());
    crate::post_write::page_write(
        &db,
        crate::post_write::PageWrite::Create {
            page_id: None,
            req: wenlan_types::requests::CreateConceptRequest {
                title: "Firing schedule".into(),
                content: "Mira fires the kiln in Oslo every Sunday [1].".into(),
                summary: None,
                entity_id: None,
                space: wenlan_types::WriteSpaceTarget::Named("Studio".into()),
                source_memory_ids: vec!["xs_old".into()],
                creation_kind: Some("authored".into()),
                workspace: Some("Studio".into()),
            },
            agent: "test",
            knowledge_path: None,
            page_min_cluster_size: 1,
            page_match_threshold: 2.0,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
            skip_protected_title_match: false,
        },
    )
    .await
    .unwrap();

    memory(&db, "xs_a", MIRA_NOTE).await;
    memory(&db, "xs_b", KILN_NOTE).await;
    let claim = claim_new(
        &db,
        &ReadScope::Global,
        &["xs_a".into(), "xs_b".into()],
        Some(&options(NO_AUTO_LINK)),
    )
    .await
    .unwrap();
    let work = claim.organize.clone().unwrap();
    organize_new(&db, &claim.token, &answers(&work))
        .await
        .unwrap();
    let links = crate::steep_new::links_new(&db, &claim.token)
        .await
        .unwrap();
    assert_eq!(
        links.link_targets,
        vec!["Firing schedule".to_string()],
        "the unique title in another Space resolves without qualification"
    );
    crate::steep_new::release_new(&claim.token).unwrap();
}
