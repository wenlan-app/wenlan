// SPDX-License-Identifier: Apache-2.0
//! **Organizing on demand**: the memories a new page will cite get the same
//! classification and entity detection Wenlan's own model gives them in
//! Automatic mode, done by the user's AI tool.
//!
//! In Automatic mode the ambient lanes classify a memory (type, quality,
//! importance, tags) and detect the people, projects and tools it mentions
//! (entity pages, `memory_entities`, relations), one model call per memory.
//! On demand there is no background model, so a page written from imported
//! notes would land with no entities and no links. Here a new-page claim
//! ([`crate::steep_new::claim_new`]) also hands out, for each claimed source
//! still waiting on those steps, the exact request the lane would send
//! ([`crate::ingest::classification_request`],
//! [`crate::kg::entity_extraction::kg_extraction_request`]). The caller's small
//! model answers each one, and [`apply`] lands the answer through the lane's
//! own parser and version-checked commit
//! ([`crate::ingest::apply_classification_output`],
//! [`MemoryDB::apply_entity_extraction`]). Only the model differs.
//!
//! The model-free part of the entity stage (a link repair, the vector
//! auto-link to a known entity) runs in the daemon while the tasks are
//! planned, exactly as the ambient lane runs it before spending a call.

use wenlan_types::steep_new::{
    SteepOrganizeStage, SteepOrganizeTask, SteepOrganizeWork, STEEP_ORGANIZE_STAGE_CLASSIFY,
    STEEP_ORGANIZE_STAGE_ENTITIES,
};

use crate::db::{EntityEnrichmentPrep, EntityExtractionPending, MemoryDB, MemoryEnrichmentInput};
use crate::error::WenlanError;
use crate::prompts::PromptRegistry;

/// What a claim needs to plan organize work: the live prompts (so a user's
/// prompt override applies here as in Automatic mode) and the auto-link
/// distance the entity lane uses.
#[derive(Debug, Clone)]
pub struct OrganizeOptions {
    pub prompts: PromptRegistry,
    pub entity_link_distance: f32,
}

/// The lane input a task was planned from. The answer commits against it,
/// so a memory that changed since the claim commits nothing.
#[derive(Debug, Clone)]
pub enum OrganizeInput {
    Classify(MemoryEnrichmentInput),
    Entities(EntityExtractionPending),
}

/// One planned small-model call.
#[derive(Debug, Clone)]
pub struct OrganizeTask {
    pub n: u32,
    pub memory_id: String,
    pub input: OrganizeInput,
    pub system_prompt: String,
    pub prompt: String,
}

impl OrganizeTask {
    pub fn stage(&self) -> &'static str {
        match self.input {
            OrganizeInput::Classify(_) => STEEP_ORGANIZE_STAGE_CLASSIFY,
            OrganizeInput::Entities(_) => STEEP_ORGANIZE_STAGE_ENTITIES,
        }
    }
}

/// How to answer, for the caller.
pub const ORGANIZE_NOTE: &str = "Organize the claim's sources the way Wenlan's own model \
would, on your tool's cheapest model, not the main model. A helper fetches these tasks itself \
with organize_tasks and the token; nobody copies source text to it. For each task, use the \
instructions of its stage as the system prompt and the task's prompt as the user message, \
and answer with only what those instructions ask for. Then call organize with the token and \
the answers as [{n, answer}], and report back one line: the counts that call returned. Only \
if no helper on a cheaper model can start at all, the main model does this itself for this \
run and says so in its report.";

/// Plan the organize work for `source_ids`, in that order: for each memory
/// still waiting on classification or on entity detection, the request the
/// ambient lane would send. A memory the vector auto-link settles needs no
/// task. Task numbers start at 1.
pub async fn plan(
    db: &MemoryDB,
    options: &OrganizeOptions,
    source_ids: &[String],
) -> Result<Vec<OrganizeTask>, WenlanError> {
    let mut tasks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let entities_enabled = crate::db::entity_sweep_enabled();
    for source_id in source_ids {
        if !seen.insert(source_id.as_str()) {
            continue;
        }
        if let Some(input) = db
            .get_classification_candidate_for(
                source_id,
                crate::post_ingest::ENRICHMENT_MAX_ATTEMPTS,
            )
            .await?
        {
            let request = crate::ingest::classification_request(&input, &options.prompts);
            tasks.push(OrganizeTask {
                n: 0,
                memory_id: source_id.clone(),
                input: OrganizeInput::Classify(input),
                system_prompt: request.system_prompt.unwrap_or_default(),
                prompt: request.user_prompt,
            });
        }
        if !entities_enabled {
            continue;
        }
        if let Some(EntityEnrichmentPrep::NeedsExtraction(pending)) = db
            .entity_enrichment_for_claim(source_id, options.entity_link_distance)
            .await?
        {
            let request = crate::kg::entity_extraction::kg_extraction_request(
                &options.prompts,
                &pending.content,
            );
            tasks.push(OrganizeTask {
                n: 0,
                memory_id: source_id.clone(),
                input: OrganizeInput::Entities(pending),
                system_prompt: request.system_prompt.unwrap_or_default(),
                prompt: request.user_prompt,
            });
        }
    }
    for (i, task) in tasks.iter_mut().enumerate() {
        task.n = u32::try_from(i + 1).unwrap_or(u32::MAX);
    }
    Ok(tasks)
}

/// The wire form of `tasks`, with each stage's system prompt once.
pub fn work(tasks: &[OrganizeTask]) -> Option<SteepOrganizeWork> {
    if tasks.is_empty() {
        return None;
    }
    let mut stages: Vec<SteepOrganizeStage> = Vec::new();
    for task in tasks {
        if !stages.iter().any(|s| s.stage == task.stage()) {
            stages.push(SteepOrganizeStage {
                stage: task.stage().to_string(),
                instructions: task.system_prompt.clone(),
            });
        }
    }
    Some(SteepOrganizeWork {
        stages,
        tasks: tasks
            .iter()
            .map(|task| SteepOrganizeTask {
                n: task.n,
                stage: task.stage().to_string(),
                memory_id: task.memory_id.clone(),
                prompt: task.prompt.clone(),
            })
            .collect(),
        note: ORGANIZE_NOTE.to_string(),
    })
}

/// What landing one answer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The lane's commit saved it.
    Saved,
    /// The answer could not be read; the lane recorded a retry receipt.
    Unreadable,
    /// The memory changed since the claim; nothing was saved.
    Stale,
}

/// Land one answer through the lane's own parser and commit.
pub async fn apply(
    db: &MemoryDB,
    task: &OrganizeTask,
    answer: &str,
) -> Result<Applied, WenlanError> {
    match &task.input {
        OrganizeInput::Classify(input) => {
            let readable = crate::llm_provider::parse_classify_response(answer).is_some();
            let committed =
                crate::ingest::apply_classification_output(db, input, Ok(answer.to_string()))
                    .await?;
            Ok(match (readable, committed) {
                (true, true) => Applied::Saved,
                (false, true) => Applied::Unreadable,
                (_, false) => Applied::Stale,
            })
        }
        OrganizeInput::Entities(pending) => {
            // An unreadable reply is an empty extraction, exactly as when
            // Wenlan's own model gives one.
            let results = crate::kg::entity_extraction::parse_kg_output(&pending.content, answer);
            let committed = db.apply_entity_extraction(pending, Ok(results)).await?;
            Ok(if committed {
                Applied::Saved
            } else {
                Applied::Stale
            })
        }
    }
}

#[cfg(test)]
#[path = "steep_organize_test.rs"]
mod steep_organize_test;
