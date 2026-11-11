// SPDX-License-Identifier: Apache-2.0
//! Following what Claude Code and Codex remember on this computer, one way
//! (see `wenlan_core::ai_memory_import` for the gates). Reads the tools'
//! memory files, never writes them.
//!
//! Each memory's owner is the tool's file. A pass compares the files with
//! the ledger of memories already decided:
//! - a new memory goes through the gates and may become a point;
//! - an edited memory brings its point up to date (the new text supersedes
//!   it, so pages built on it are rewritten and the old text stays in
//!   history);
//! - a memory gone from the tool takes its point with it (pages built on it
//!   are rewritten);
//! - a point the person edited is theirs and is never changed or removed; a
//!   point the person removed never comes back;
//! - a tool whose memory folder is missing is left as it was.
//!
//! The person turns following on; after that a pass runs with the
//! background reading of conversations, on the same model
//! (`things_intake::models`, its grouping model), at most once an hour.
//! Removals need no model; new and edited memories wait for one.

use std::collections::{HashMap, HashSet};

use wenlan_core::ai_memory_import::{
    self as gates, Candidate, CandidateKind, FoundMemory, ImportCounts, MatchItem, MemoryTool,
    Verdict,
};
use wenlan_core::db::ai_memory_import::{AiMemoryImport, Decided, LedgerRow, PointState};
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_core::read_scope::ReadScope;
use wenlan_types::requests::StoreMemoryRequest;
use wenlan_types::session_context::TranscriptRoots;
use wenlan_types::WriteSpaceTarget;

use crate::things_intake::PointWriter;

/// Pages and points shown with each memory.
const CANDIDATE_PAGES: usize = 3;
const CANDIDATE_POINTS: usize = 2;
/// Title kept for a new point.
const TITLE_CHARS: usize = 80;
/// How the import marks what it wrote, next to the tool's name.
pub const SOURCE_SUFFIX: &str = "-memory";
/// A background pass runs at most this often.
pub const BACKGROUND_EVERY_SECS: i64 = 60 * 60;

/// The tools' memory files as they are now.
pub struct Seen {
    pub found: Vec<FoundMemory>,
    /// Tools whose memory folder is on this computer.
    pub present: Vec<MemoryTool>,
}

/// The memory files on this computer, read off the async runtime.
pub async fn scan_now() -> Result<Seen, String> {
    let roots = TranscriptRoots::from_env(dirs::home_dir());
    tokio::task::spawn_blocking(move || Seen {
        found: gates::scan(&roots),
        present: gates::tools_present(&roots),
    })
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

/// A memory to decide, with the ledger row of the earlier text it replaces
/// when it is an edit.
struct Fresh {
    key: String,
    memory: FoundMemory,
    replaces: Option<(LedgerRow, String)>,
}

struct Kept {
    fresh: Fresh,
    /// About the person's own life: filed unsorted, never in a project's or
    /// a shared Space.
    personal: bool,
    space: Option<String>,
    candidates: Vec<Candidate>,
}

/// Turn following on. Does nothing when it is already on.
pub async fn start(db: &MemoryDB) -> Result<(), String> {
    if db
        .ai_memory_import()
        .await
        .map_err(|e| e.to_string())?
        .is_none()
    {
        db.record_ai_memory_import(&AiMemoryImport {
            at: chrono::Utc::now().timestamp(),
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One pass over `seen`, while following is on. With no `model`, only
/// removals are made; new and edited memories wait. Every decision is noted
/// as it is made, so a pass that fails partway leaves nothing half done and
/// the next one carries on.
pub async fn follow(
    db: &MemoryDB,
    model: Option<&dyn LlmProvider>,
    writer: &dyn PointWriter,
    seen: Seen,
) -> Result<AiMemoryImport, String> {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("the AI memory pass is already running".into());
    }
    let _running = Running;
    let Some(mut record) = db.ai_memory_import().await.map_err(|e| e.to_string())? else {
        return Err("following AI tools' memory is off".into());
    };
    let err = |e: wenlan_core::WenlanError| e.to_string();

    let mut counts = ImportCounts {
        read: seen.found.len(),
        ..Default::default()
    };
    let by_tool = count_by_tool(&seen.found);
    // A scan cut short at the cap is not a list of what the tools hold: what
    // it missed must not read as deleted.
    let complete = seen.found.len() < gates::MAX_MEMORIES;

    let ledger = db.ai_memory_ledger().await.map_err(err)?;
    let known: HashSet<&str> = ledger.iter().map(|r| r.key.as_str()).collect();
    let mut found_keys = HashSet::new();
    let mut fresh = Vec::new();
    for memory in seen.found {
        let key = gates::memory_key(&memory);
        found_keys.insert(key.clone());
        if !known.contains(key.as_str()) {
            fresh.push(Fresh {
                key,
                memory,
                replaces: None,
            });
        }
    }
    let gone: Vec<LedgerRow> = ledger
        .iter()
        .filter(|r| complete && !found_keys.contains(&r.key))
        .filter(|r| gates::key_tool(&r.key).is_some_and(|t| seen.present.contains(&t)))
        .cloned()
        .collect();

    // An edit: the one memory gone from a file and the one new memory in that
    // same file, while its point is as Wenlan wrote it.
    let mut gone_by_file: HashMap<&str, Vec<&LedgerRow>> = HashMap::new();
    for row in &gone {
        gone_by_file.entry(row.file.as_str()).or_default().push(row);
    }
    let mut fresh_by_file: HashMap<String, usize> = HashMap::new();
    for f in &fresh {
        *fresh_by_file
            .entry(f.memory.path.to_string_lossy().into_owned())
            .or_default() += 1;
    }
    let mut paired = HashSet::new();
    for f in &mut fresh {
        let file = f.memory.path.to_string_lossy().into_owned();
        let (Some(rows), Some(1)) = (gone_by_file.get(file.as_str()), fresh_by_file.get(&file))
        else {
            continue;
        };
        let [row] = rows.as_slice() else { continue };
        let Decided::Point { id, hash } = &row.decided else {
            continue;
        };
        if db.ai_memory_point(id, hash).await.map_err(err)? == PointState::Untouched {
            paired.insert(row.key.clone());
            f.replaces = Some(((*row).clone(), id.clone()));
        }
    }

    // Gone from the tool: its own point goes too, unless the person edited
    // or removed it.
    for row in gone.iter().filter(|r| !paired.contains(&r.key)) {
        if let Decided::Point { id, hash } = &row.decided {
            if db.ai_memory_point(id, hash).await.map_err(err)? == PointState::Untouched {
                db.delete_by_source_id("memory", id).await.map_err(err)?;
                counts.removed += 1;
            }
        }
        db.drop_ai_memory_decided(&row.key).await.map_err(err)?;
    }

    // 1. Secrets and personal data: never sent to a model, never kept.
    let mut clean = Vec::new();
    for f in fresh {
        if gates::looks_sensitive(&f.memory.text) {
            counts.sensitive += 1;
            settle(db, &f, Decided::Sensitive, &mut counts).await?;
        } else {
            clean.push(f);
        }
    }

    let Some(model) = model else {
        counts.waiting = clean.len();
        return finish(db, &mut record, counts, by_tool).await;
    };

    // 2. The questions: knowledge only, the person's own life filed apart.
    let mut knowledge = Vec::new();
    for batch in chunked(clean, gates::KEEP_BATCH) {
        let texts: Vec<&str> = batch.iter().map(|f| f.memory.text.as_str()).collect();
        let Some(answer) = ask(model, gates::keep_prompt(&texts), "ai_memory_keep").await else {
            counts.undecided += batch.len();
            continue;
        };
        let keeps = gates::parse_keep(&answer, batch.len());
        for (f, keep) in batch.into_iter().zip(keeps) {
            match keep {
                gates::Keep::Work => knowledge.push((f, false)),
                gates::Keep::Personal => knowledge.push((f, true)),
                gates::Keep::Drop => {
                    counts.not_knowledge += 1;
                    settle(db, &f, Decided::NotKnowledge, &mut counts).await?;
                }
            }
        }
    }

    // 3. An edit of a memory Wenlan already follows brings its point up to
    // date, where the point is now. A new one is compared with what Wenlan
    // already holds in its Space.
    let mut spaces: HashMap<std::path::PathBuf, Option<String>> = HashMap::new();
    let mut kept = Vec::new();
    for (f, personal) in knowledge {
        if let Some((_, old)) = &f.replaces {
            let space = db.ai_memory_point_space(old).await.map_err(err)?;
            let target = match space {
                Some(space) => WriteSpaceTarget::Named(space),
                None => WriteSpaceTarget::Uncategorized,
            };
            let mut request = point_request(&f.memory, target);
            request.supersedes = Some(old.clone());
            let id = writer.write(request).await?;
            counts.updated += 1;
            let hash = gates::text_hash(&f.memory.text);
            note(db, &f, Decided::Point { id, hash }).await?;
            if let Some((row, _)) = &f.replaces {
                db.drop_ai_memory_decided(&row.key).await.map_err(err)?;
            }
            continue;
        }
        let space = match &f.memory.project_dir {
            _ if personal => None,
            Some(dir) => match spaces.get(dir) {
                Some(space) => space.clone(),
                None => {
                    let space = db
                        .space_for_transcript_folder(&dir.to_string_lossy())
                        .await
                        .map_err(err)?;
                    spaces.insert(dir.clone(), space.clone());
                    space
                }
            },
            None => None,
        };
        let scope = match (&space, personal) {
            (_, true) => ReadScope::Uncategorized,
            (Some(space), false) => ReadScope::Space(space.clone()),
            (None, false) => ReadScope::Global,
        };
        let candidates = candidates(db, &f.memory.text, &scope).await?;
        kept.push(Kept {
            fresh: f,
            personal,
            space,
            candidates,
        });
    }
    decide_and_write(db, model, writer, kept, &mut counts).await?;
    finish(db, &mut record, counts, by_tool).await
}

/// A background pass, when following is on and the last pass is an hour
/// old. `None` when it did not run.
pub async fn follow_in_background(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    writer: &dyn PointWriter,
) -> Result<Option<AiMemoryImport>, String> {
    let Some(record) = db.ai_memory_import().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if chrono::Utc::now().timestamp() - record.last_at < BACKGROUND_EVERY_SECS {
        return Ok(None);
    }
    let seen = scan_now().await?;
    follow(db, Some(model), writer, seen).await.map(Some)
}

fn chunked(items: Vec<Fresh>, size: usize) -> Vec<Vec<Fresh>> {
    let mut out: Vec<Vec<Fresh>> = Vec::new();
    for item in items {
        match out.last_mut() {
            Some(last) if last.len() < size => last.push(item),
            _ => out.push(vec![item]),
        }
    }
    out
}

async fn finish(
    db: &MemoryDB,
    record: &mut AiMemoryImport,
    counts: ImportCounts,
    by_tool: Vec<(String, usize)>,
) -> Result<AiMemoryImport, String> {
    record.last_at = chrono::Utc::now().timestamp();
    record.counts = counts;
    record.by_tool = by_tool;
    // Following may have been turned off while this pass ran: then the pass
    // does not turn it back on.
    if db
        .ai_memory_import()
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        db.record_ai_memory_import(record)
            .await
            .map_err(|e| e.to_string())?;
    }
    tracing::info!("[ai-memory] pass done: {:?}", record.counts);
    Ok(record.clone())
}

/// Note what became of a memory.
async fn note(db: &MemoryDB, f: &Fresh, decided: Decided) -> Result<(), String> {
    db.set_ai_memory_decided(&f.key, &f.memory.path.to_string_lossy(), &decided)
        .await
        .map_err(|e| e.to_string())
}

/// A memory left out: when it was an edit, the point of its earlier text
/// goes, since the tool no longer says that.
async fn settle(
    db: &MemoryDB,
    f: &Fresh,
    decided: Decided,
    counts: &mut ImportCounts,
) -> Result<(), String> {
    note(db, f, decided).await?;
    if let Some((row, old)) = &f.replaces {
        db.delete_by_source_id("memory", old)
            .await
            .map_err(|e| e.to_string())?;
        db.drop_ai_memory_decided(&row.key)
            .await
            .map_err(|e| e.to_string())?;
        counts.removed += 1;
    }
    Ok(())
}

async fn decide_and_write(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    writer: &dyn PointWriter,
    kept: Vec<Kept>,
    counts: &mut ImportCounts,
) -> Result<(), String> {
    for batch in kept.chunks(gates::MATCH_BATCH) {
        let items: Vec<MatchItem<'_>> = batch
            .iter()
            .map(|k| MatchItem {
                memory: &k.fresh.memory.text,
                candidates: &k.candidates,
            })
            .collect();
        let Some(answer) = ask(model, gates::match_prompt(&items), "ai_memory_match").await else {
            counts.undecided += batch.len();
            continue;
        };
        for (i, verdict) in gates::parse_match(&answer, &items).into_iter().enumerate() {
            let k = &batch[i];
            let decided = match verdict {
                Verdict::New => {
                    let id = writer.write(new_point(k)).await?;
                    counts.added += 1;
                    Decided::Point {
                        id,
                        hash: gates::text_hash(&k.fresh.memory.text),
                    }
                }
                // Wenlan already holds it: nothing is added or changed.
                Verdict::Same { .. } => {
                    counts.already += 1;
                    Decided::Already
                }
                Verdict::Conflict => {
                    counts.conflicts += 1;
                    Decided::Conflict
                }
            };
            note(db, &k.fresh, decided).await?;
        }
    }
    Ok(())
}

/// The existing pages and points nearest to `text` in `scope`.
async fn candidates(
    db: &MemoryDB,
    text: &str,
    scope: &ReadScope,
) -> Result<Vec<Candidate>, String> {
    // Words only: page search reads its query as full-text syntax, where a
    // stray period or quote is an error, not a character.
    let text: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let text = text.as_str();
    let mut out = Vec::new();
    let pages = db
        .search_pages_scoped(text, CANDIDATE_PAGES, None, scope)
        .await
        .map_err(|e| e.to_string())?;
    for page in pages {
        out.push(Candidate {
            kind: CandidateKind::Page,
            id: page.id,
            title: page.title,
            text: page.content,
        });
    }
    let points = db
        .search_memory(text, CANDIDATE_POINTS, None, scope, None, None, None, None)
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

/// A new memory written as a point, in its project's Space.
fn new_point(kept: &Kept) -> StoreMemoryRequest {
    point_request(
        &kept.fresh.memory,
        match &kept.space {
            _ if kept.personal => WriteSpaceTarget::Uncategorized,
            Some(space) => WriteSpaceTarget::Named(space.clone()),
            None => WriteSpaceTarget::Inherit,
        },
    )
}

/// A memory as a point, marked as coming from its tool's memory.
fn point_request(memory: &FoundMemory, space: WriteSpaceTarget) -> StoreMemoryRequest {
    let title: String = memory
        .text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(TITLE_CHARS)
        .collect();
    StoreMemoryRequest {
        content: memory.text.clone(),
        memory_type: Some("fact".into()),
        space,
        source_agent: Some(format!("{}{SOURCE_SUFFIX}", memory.tool.as_str())),
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
        last_at: pass.last_at,
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
            "ai_memory_import_needs_the_person: only the person turns following AI tools' memory on or off"
                .into(),
        ));
    }
    Ok(())
}

/// GET /api/import/ai-memory
///
/// How many memories Claude Code and Codex keep on this computer, whether a
/// model is set up to read them, and following, if it is on. Counts only.
pub async fn handle_status(
    axum::extract::State(state): axum::extract::State<crate::state::SharedState>,
) -> Result<
    axum::Json<wenlan_types::ai_memory_import::AiMemoryImportStatus>,
    crate::error::ServerError,
> {
    let db = open_db(&state).await?;
    let seen = scan_now()
        .await
        .map_err(crate::error::ServerError::ValidationError)?;
    let ready = crate::things_intake::models(&state).await.is_some();
    Ok(axum::Json(
        wenlan_types::ai_memory_import::AiMemoryImportStatus {
            found: tool_counts(&count_by_tool(&seen.found)),
            ready,
            import: db.ai_memory_import().await?.as_ref().map(summary),
        },
    ))
}

/// POST /api/import/ai-memory
///
/// The person turns following on (or, when it is on, asks for a pass now),
/// and a pass runs at once. Without a model set up, only removals are made
/// and new memories wait for one.
pub async fn handle_run(
    axum::extract::State(state): axum::extract::State<crate::state::SharedState>,
    headers: axum::http::HeaderMap,
) -> Result<
    axum::Json<wenlan_types::ai_memory_import::AiMemoryImportSummary>,
    crate::error::ServerError,
> {
    require_person(&headers)?;
    let db = open_db(&state).await?;
    let models = crate::things_intake::models(&state).await;
    let seen = scan_now()
        .await
        .map_err(crate::error::ServerError::ValidationError)?;
    start(&db)
        .await
        .map_err(crate::error::ServerError::ValidationError)?;
    let writer = crate::things_intake::DaemonWriter(state.clone());
    let pass = follow(
        &db,
        models.as_ref().map(|m| m.group.as_ref()),
        &writer,
        seen,
    )
    .await
    .map_err(crate::error::ServerError::ValidationError)?;
    Ok(axum::Json(summary(&pass)))
}

/// POST /api/import/ai-memory/undo
///
/// The person turns following off: every point it brought in that they did
/// not edit is taken back. It can be turned on again.
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
