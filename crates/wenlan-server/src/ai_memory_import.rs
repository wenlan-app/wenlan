// SPDX-License-Identifier: Apache-2.0
//! The one-time import of what Claude Code and Codex remember on this
//! computer, started only by the person (see
//! `wenlan_core::ai_memory_import` for the gates). Reads the tools' memory
//! files, never writes them.
//!
//! The model is the one the nightly reading of conversations uses
//! (`things_intake::models`, its grouping model): a cloud model when one is
//! set, else the person's own AI tool. It runs only when the person presses
//! import, never in the background.

use std::collections::HashMap;

use wenlan_core::ai_memory_import::{
    self as gates, Candidate, CandidateKind, FoundMemory, ImportCounts, MatchItem, Verdict,
};
use wenlan_core::db::ai_memory_import::AiMemoryImport;
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_core::read_scope::ReadScope;
use wenlan_types::requests::StoreMemoryRequest;
use wenlan_types::session_context::TranscriptRoots;
use wenlan_types::WriteSpaceTarget;

use crate::things_intake::PointWriter;

/// Pages and points shown with each memory: pages first, since only a page
/// can take one more source.
const CANDIDATE_PAGES: usize = 3;
const CANDIDATE_POINTS: usize = 2;
/// Title kept for a new point.
const TITLE_CHARS: usize = 80;
/// How the import marks what it wrote, next to the tool's name.
pub const SOURCE_SUFFIX: &str = "-memory";

/// The memory files on this computer, read off the async runtime.
pub async fn scan_now() -> Result<Vec<FoundMemory>, String> {
    let roots = TranscriptRoots::from_env(dirs::home_dir());
    tokio::task::spawn_blocking(move || gates::scan(&roots))
        .await
        .map_err(|e| e.to_string())
}

/// How many memories each tool keeps here, before anything is read by a
/// model: what the person sees before saying yes.
pub fn count_by_tool(found: &[FoundMemory]) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for memory in found {
        let tool = memory.tool.as_str();
        match counts.iter_mut().find(|(t, _)| t == tool) {
            Some((_, n)) => *n += 1,
            None => counts.push((tool.to_string(), 1)),
        }
    }
    counts
}

struct Kept {
    memory: FoundMemory,
    space: Option<String>,
    candidates: Vec<Candidate>,
}

/// Run the import over `found`, once. Refused while an earlier import
/// stands. What it wrote is recorded for the one Undo; when anything fails
/// after a write, what it wrote is forgotten again.
pub async fn run(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    writer: &dyn PointWriter,
    found: Vec<FoundMemory>,
) -> Result<AiMemoryImport, String> {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("the AI memory import is already running".into());
    }
    let _running = Running;
    if db
        .ai_memory_import()
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("the AI memory import already ran; take it back first".into());
    }

    let mut counts = ImportCounts {
        read: found.len(),
        ..Default::default()
    };
    let by_tool = count_by_tool(&found);

    // 1. Secrets and personal data: dropped whole, never sent to a model.
    let (sensitive, clean): (Vec<_>, Vec<_>) = found
        .into_iter()
        .partition(|m| gates::looks_sensitive(&m.text));
    counts.sensitive = sensitive.len();
    drop(sensitive);

    // 2. The four questions.
    let mut knowledge = Vec::new();
    for batch in clean.chunks(gates::KEEP_BATCH) {
        let texts: Vec<&str> = batch.iter().map(|m| m.text.as_str()).collect();
        let answer = ask(model, gates::keep_prompt(&texts), "ai_memory_keep").await;
        let Some(answer) = answer else {
            counts.undecided += batch.len();
            continue;
        };
        for (memory, keep) in batch.iter().zip(gates::parse_keep(&answer, batch.len())) {
            if keep {
                knowledge.push(memory.clone());
            } else {
                counts.not_knowledge += 1;
            }
        }
    }

    // 3. What Wenlan already holds near each one, in its Space.
    let mut spaces: HashMap<std::path::PathBuf, Option<String>> = HashMap::new();
    let mut kept = Vec::with_capacity(knowledge.len());
    for memory in knowledge {
        let space = match &memory.project_dir {
            Some(dir) => match spaces.get(dir) {
                Some(space) => space.clone(),
                None => {
                    let space = db
                        .space_for_transcript_folder(&dir.to_string_lossy())
                        .await
                        .map_err(|e| e.to_string())?;
                    spaces.insert(dir.clone(), space.clone());
                    space
                }
            },
            None => None,
        };
        let candidates = candidates(db, &memory.text, space.as_deref()).await?;
        kept.push(Kept {
            memory,
            space,
            candidates,
        });
    }

    let mut written = Vec::new();
    let mut attached = Vec::new();
    let outcome = decide_and_write(
        db,
        model,
        writer,
        &kept,
        &mut counts,
        &mut written,
        &mut attached,
    )
    .await;
    let pass = AiMemoryImport {
        at: chrono::Utc::now().timestamp(),
        counts,
        written: written.clone(),
        attached,
        by_tool,
    };
    let recorded = match outcome {
        Ok(()) => db
            .record_ai_memory_import(&pass)
            .await
            .map_err(|e| e.to_string()),
        Err(error) => Err(error),
    };
    if let Err(error) = recorded {
        if let Err(cleanup) = db.forget_ai_memory_rows(&written).await {
            tracing::warn!("[ai-memory] cleanup after a failed import: {cleanup}");
        }
        return Err(error);
    }
    tracing::info!("[ai-memory] import done: {:?}", pass.counts);
    Ok(pass)
}

async fn decide_and_write(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    writer: &dyn PointWriter,
    kept: &[Kept],
    counts: &mut ImportCounts,
    written: &mut Vec<String>,
    attached: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for batch in kept.chunks(gates::MATCH_BATCH) {
        let items: Vec<MatchItem<'_>> = batch
            .iter()
            .map(|k| MatchItem {
                memory: &k.memory.text,
                candidates: &k.candidates,
            })
            .collect();
        let Some(answer) = ask(model, gates::match_prompt(&items), "ai_memory_match").await else {
            counts.undecided += batch.len();
            continue;
        };
        let verdicts = gates::parse_match(&answer, &items);

        // The source check, run again before a memory becomes one more
        // source of a page: does the memory support the page's sentence?
        let mut to_check = Vec::new();
        for (i, verdict) in verdicts.iter().enumerate() {
            match verdict {
                Verdict::New => {
                    let id = writer.write(new_point(&batch[i])).await?;
                    written.push(id);
                    counts.added += 1;
                }
                Verdict::Conflict => counts.conflicts += 1,
                Verdict::Same { with, sentence } => {
                    let candidate = &batch[i].candidates[*with];
                    if candidate.kind == CandidateKind::Point {
                        counts.already += 1;
                    } else {
                        to_check.push((i, candidate.id.clone(), sentence.clone()));
                    }
                }
            }
        }
        if to_check.is_empty() {
            continue;
        }
        let supported = source_check(model, batch, &to_check).await;
        for ((i, page_id, _), ok) in to_check.into_iter().zip(supported) {
            if !ok {
                counts.conflicts += 1;
                continue;
            }
            let id = writer.write(new_point(&batch[i])).await?;
            written.push(id.clone());
            let result = wenlan_core::post_write::page_write(
                db,
                wenlan_core::post_write::PageWrite::Attach {
                    page_id: &page_id,
                    source_memory_ids: std::slice::from_ref(&id),
                    link_reason: "ai_memory_import",
                    agent: "wenlan",
                },
            )
            .await;
            match result {
                Ok(_) => {
                    attached.push((page_id, id));
                    counts.attached += 1;
                }
                Err(error) => {
                    // The memory stays as a new point; the page keeps its
                    // sources as they were.
                    tracing::warn!("[ai-memory] a source was not attached: {error}");
                    counts.added += 1;
                }
            }
        }
    }
    Ok(())
}

/// The existing pages (not the person's own or locked ones, which only they
/// change) and points nearest to `text`, in its Space or everywhere when it
/// has none.
async fn candidates(
    db: &MemoryDB,
    text: &str,
    space: Option<&str>,
) -> Result<Vec<Candidate>, String> {
    let scope = match space {
        Some(space) => ReadScope::Space(space.to_string()),
        None => ReadScope::Global,
    };
    let mut out = Vec::new();
    let pages = db
        .search_pages_scoped(text, CANDIDATE_PAGES, None, &scope)
        .await
        .map_err(|e| e.to_string())?;
    for page in pages {
        out.push(Candidate {
            kind: if page.user_edited || page.review_required {
                // The person's page already saying it is enough; it takes
                // no source from an AI tool's memory.
                CandidateKind::Point
            } else {
                CandidateKind::Page
            },
            id: page.id,
            title: page.title,
            text: page.content,
        });
    }
    let points = db
        .search_memory(text, CANDIDATE_POINTS, None, &scope, None, None, None, None)
        .await
        .map_err(|e| e.to_string())?;
    for point in points {
        out.push(Candidate {
            kind: CandidateKind::Point,
            id: point.source_id,
            title: point.title,
            text: point.content,
        });
    }
    out.truncate(gates::MATCH_CANDIDATES);
    Ok(out)
}

/// Whether each memory supports the page sentence the model matched it to,
/// by the same entailment check pages use. A failed call supports nothing.
async fn source_check(
    model: &dyn LlmProvider,
    batch: &[Kept],
    checks: &[(usize, String, String)],
) -> Vec<bool> {
    use wenlan_core::claim_judge as judge;
    let ids: Vec<String> = (0..checks.len()).map(|i| format!("item-{i}")).collect();
    let items: Vec<_> = checks
        .iter()
        .zip(&ids)
        .map(|((i, _, sentence), id)| judge::M5ClaimEntailmentItem {
            item_id: id,
            claim: sentence,
            evidence: &batch[*i].memory.text,
        })
        .collect();
    let Some(prompt) = judge::build_m5_claim_entailment_batch_user_prompt(&items) else {
        return vec![false; checks.len()];
    };
    let answer = model
        .generate(LlmRequest {
            system_prompt: Some(judge::M5_CLAIM_ENTAILMENT_SYSTEM_PROMPT.to_string()),
            user_prompt: prompt,
            max_tokens: 1_000,
            temperature: 0.0,
            label: Some("ai_memory_source_check".into()),
            timeout_secs: Some(judge::M5_CLAIM_JUDGE_TIMEOUT_SECS),
        })
        .await;
    let Some(scores) = answer
        .ok()
        .and_then(|raw| judge::parse_m5_claim_entailment_batch_scores(&raw, &ids))
    else {
        return vec![false; checks.len()];
    };
    ids.iter()
        .map(|id| {
            scores
                .iter()
                .any(|s| &s.item_id == id && s.score >= judge::M5_JUDGE_QUALIFICATION_THRESHOLD)
        })
        .collect()
}

/// A memory written as a point, marked as coming from its tool's memory,
/// in its project's Space.
fn new_point(kept: &Kept) -> StoreMemoryRequest {
    let title: String = kept
        .memory
        .text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(TITLE_CHARS)
        .collect();
    StoreMemoryRequest {
        content: kept.memory.text.clone(),
        memory_type: Some("fact".into()),
        space: match &kept.space {
            Some(space) => WriteSpaceTarget::Named(space.clone()),
            None => WriteSpaceTarget::Inherit,
        },
        source_agent: Some(format!("{}{SOURCE_SUFFIX}", kept.memory.tool.as_str())),
        title: Some(title),
        confidence: None,
        supersedes: None,
        entity: None,
        entity_type: None,
        entity_id: None,
        structured_fields: None,
        retrieval_cue: None,
        said: None,
    }
}

async fn ask(model: &dyn LlmProvider, prompt: String, label: &str) -> Option<String> {
    match model
        .generate(LlmRequest {
            system_prompt: Some(
                "You sort an AI tool's memories for a person's knowledge base, keeping only what you are sure of. Answer with JSON only."
                    .into(),
            ),
            user_prompt: prompt,
            max_tokens: 3_000,
            temperature: 0.0,
            label: Some(label.into()),
            timeout_secs: Some(300),
        })
        .await
    {
        Ok(text) => Some(text),
        Err(error) => {
            tracing::warn!("[ai-memory] {label} call failed: {error}");
            None
        }
    }
}

// ---- Routes -------------------------------------------------------------

fn summary(pass: &AiMemoryImport) -> wenlan_types::ai_memory_import::AiMemoryImportSummary {
    wenlan_types::ai_memory_import::AiMemoryImportSummary {
        at: pass.at,
        counts: pass.counts.clone(),
        by_tool: tool_counts(&pass.by_tool),
    }
}

fn tool_counts(
    counts: &[(String, usize)],
) -> Vec<wenlan_types::ai_memory_import::AiMemoryToolCount> {
    counts
        .iter()
        .map(
            |(tool, memories)| wenlan_types::ai_memory_import::AiMemoryToolCount {
                tool: tool.clone(),
                memories: *memories,
            },
        )
        .collect()
}

async fn open_db(
    state: &crate::state::SharedState,
) -> Result<std::sync::Arc<MemoryDB>, crate::error::ServerError> {
    let state = state.read().await;
    state
        .db
        .clone()
        .ok_or(crate::error::ServerError::DbNotInitialized)
}

fn require_person(headers: &axum::http::HeaderMap) -> Result<(), crate::error::ServerError> {
    if !crate::memory_routes::caller_is_person(headers) {
        return Err(crate::error::ServerError::ValidationError(
            "ai_memory_import_needs_the_person: only the person starts or takes back this import"
                .into(),
        ));
    }
    Ok(())
}

/// GET /api/import/ai-memory
///
/// How many memories Claude Code and Codex keep on this computer, whether a
/// model is set up to read them, and the import that stands. Counts only.
pub async fn handle_status(
    axum::extract::State(state): axum::extract::State<crate::state::SharedState>,
) -> Result<
    axum::Json<wenlan_types::ai_memory_import::AiMemoryImportStatus>,
    crate::error::ServerError,
> {
    let db = open_db(&state).await?;
    let found = scan_now()
        .await
        .map_err(crate::error::ServerError::ValidationError)?;
    let ready = crate::things_intake::models(&state).await.is_some();
    Ok(axum::Json(
        wenlan_types::ai_memory_import::AiMemoryImportStatus {
            found: tool_counts(&count_by_tool(&found)),
            ready,
            import: db.ai_memory_import().await?.as_ref().map(summary),
        },
    ))
}

/// POST /api/import/ai-memory
///
/// The person brings the memories in, once. Refused while an import stands.
pub async fn handle_run(
    axum::extract::State(state): axum::extract::State<crate::state::SharedState>,
    headers: axum::http::HeaderMap,
) -> Result<
    axum::Json<wenlan_types::ai_memory_import::AiMemoryImportSummary>,
    crate::error::ServerError,
> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let models = crate::things_intake::models(&state).await.ok_or_else(|| {
        crate::error::ServerError::ValidationError("no model to read the memories with".into())
    })?;
    let found = scan_now()
        .await
        .map_err(crate::error::ServerError::ValidationError)?;
    let writer = crate::things_intake::DaemonWriter(state.clone());
    let pass = run(&db, models.group.as_ref(), &writer, found)
        .await
        .map_err(crate::error::ServerError::ValidationError)?;
    Ok(axum::Json(summary(&pass)))
}

/// POST /api/import/ai-memory/undo
///
/// The person takes the whole import back; it can run again.
pub async fn handle_undo(
    axum::extract::State(state): axum::extract::State<crate::state::SharedState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<wenlan_types::ai_memory_import::AiMemoryImportUndo>, crate::error::ServerError>
{
    require_person(&headers)?;
    let db = open_db(&state).await?;
    Ok(axum::Json(
        wenlan_types::ai_memory_import::AiMemoryImportUndo {
            undone: db.undo_ai_memory_import().await?.as_ref().map(summary),
        },
    ))
}

#[cfg(test)]
#[path = "ai_memory_import_test.rs"]
mod tests;
