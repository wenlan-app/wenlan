// SPDX-License-Identifier: Apache-2.0
//! "What you're working on", from the conversations people have with their
//! AI tools (UX-THING-1): every few minutes, read the conversations that went
//! on since the last look, have the everyday model label them, and have a
//! stronger model put the work into things. See
//! `wenlan_core::conversation_intake` for the steps.
//!
//! Runs only while background AI is on. The everyday model reads each
//! conversation on this computer; the grouping model sees only the one-line
//! labels. Grouping uses the synthesis model when one is set, else the
//! person's own AI tool (Claude Code or Codex), which spends that tool's
//! quota. With neither, nothing is read.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::state::SharedState;
use wenlan_core::conversation_intake::{self as intake, FoundConversation};
use wenlan_core::db::intake::{found_session_id, needs_label};
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};

/// How often conversations are looked at.
const TICK: Duration = Duration::from_secs(10 * 60);
/// The first look, once the daemon has settled after starting.
const FIRST_LOOK: Duration = Duration::from_secs(90);
/// Most conversations labelled in one look; the rest wait for the next.
const LABEL_BATCH: usize = 30;
/// Most conversations grouped in one call.
const GROUP_BATCH: usize = 60;

/// Waits after a model failed, so a conversation the everyday model cannot
/// read, or a grouping answer that will not parse, does not spend the
/// person's quota every look. Kept in memory: a restart tries again.
#[derive(Debug, Default)]
pub struct Backoff {
    /// Per conversation: failures so far and when to try again.
    label: HashMap<String, (u32, i64)>,
    /// Grouping: failures so far and when to try again.
    group: (u32, i64),
}

/// The wait after `fails` failures in a row: one look, doubling, at most a
/// day.
fn wait_after(fails: u32) -> i64 {
    (TICK.as_secs() as i64)
        .saturating_mul(1 << fails.min(8))
        .min(86_400)
}

impl Backoff {
    fn label_ready(&self, id: &str, now: i64) -> bool {
        self.label.get(id).is_none_or(|&(_, at)| now >= at)
    }
    fn label_failed(&mut self, id: String, now: i64) {
        let fails = self.label.get(&id).map_or(0, |&(n, _)| n) + 1;
        self.label.insert(id, (fails, now + wait_after(fails - 1)));
    }
    fn group_failed(&mut self, now: i64) {
        self.group.0 += 1;
        self.group.1 = now + wait_after(self.group.0 - 1);
    }
}

/// What one look did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LookReport {
    pub found: usize,
    pub kept: usize,
    pub labelled: usize,
    pub work: usize,
    pub grouped: usize,
    pub things_made: usize,
}

/// Start the loop.
pub fn start(shared: SharedState, shutdown: tokio::sync::watch::Receiver<bool>) {
    tokio::spawn(run(shared, shutdown));
}

async fn run(shared: SharedState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut wait = FIRST_LOOK;
    let mut backoff = Backoff::default();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            return;
        }
        wait = TICK;
        match look_once(&shared, &mut backoff).await {
            Ok(Some(report)) if report.labelled > 0 || report.grouped > 0 => {
                tracing::info!("[things] read conversations: {report:?}");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!("[things] reading conversations failed: {error}"),
        }
    }
}

/// The models one look uses: the everyday model to label, and the one that
/// groups.
pub struct IntakeModels {
    pub label: Arc<dyn LlmProvider>,
    pub group: Arc<dyn LlmProvider>,
}

/// The models to use now, or `None` when background AI is off or a model is
/// missing.
async fn models(shared: &SharedState) -> Option<IntakeModels> {
    let config = wenlan_core::config::load_config();
    if !config.background_ai_enabled() {
        return None;
    }
    let everyday = wenlan_core::refinery::EverydaySource::parse(config.everyday_source.as_deref());
    let synthesis =
        wenlan_core::refinery::SynthesisSource::parse(config.synthesis_source.as_deref());
    let state = shared.read().await;
    let label = wenlan_core::refinery::resolve_everyday(
        everyday,
        state.api_llm.as_ref(),
        state.external_llm.as_ref(),
        state.llm.as_ref(),
    )
    .llm
    .cloned()?;
    let route = wenlan_core::refinery::resolve_synthesis(
        synthesis,
        state.synthesis_llm.as_ref(),
        state.api_llm.as_ref(),
        state.external_llm.as_ref(),
        state.llm.as_ref(),
    );
    // Only a cloud model groups in its place: a small local model puts
    // everything in one pile or every chat on its own, and the person's AI
    // tool groups well and is already set up.
    let group = (route.source == wenlan_core::refinery::SynthesisSource::Anthropic)
        .then(|| route.llm.cloned())
        .flatten()
        .or_else(|| {
            wenlan_core::agent_cli_llm::AgentCliProvider::find()
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
        })?;
    Some(IntakeModels { label, group })
}

/// One look, now. `None` when it did not run.
pub async fn look_once(
    shared: &SharedState,
    backoff: &mut Backoff,
) -> Result<Option<LookReport>, String> {
    let Some(models) = models(shared).await else {
        return Ok(None);
    };
    let db = shared
        .read()
        .await
        .db
        .clone()
        .ok_or_else(|| "the database is not ready".to_string())?;
    let language = intake::language_name(wenlan_core::config::load_config().ui_language.as_deref());
    let roots = wenlan_types::session_context::TranscriptRoots::from_env(dirs::home_dir());
    let now = chrono::Utc::now().timestamp();
    let found =
        tokio::task::spawn_blocking(move || intake::scan(&roots, now, intake::INTAKE_WINDOW_SECS))
            .await
            .map_err(|e| e.to_string())?;
    look_with(&db, found, &models, language, backoff)
        .await
        .map(Some)
}

/// One look over conversations already found: label what is new or went on,
/// then group what is labelled.
pub async fn look_with(
    db: &MemoryDB,
    found: Vec<FoundConversation>,
    models: &IntakeModels,
    language: &str,
    backoff: &mut Backoff,
) -> Result<LookReport, String> {
    let now = chrono::Utc::now().timestamp();
    let mut report = LookReport {
        found: found.len(),
        ..Default::default()
    };
    let (kept, _dropped) = intake::keep_work(found);
    report.kept = kept.len();
    let ids: Vec<String> = kept.iter().filter_map(found_session_id).collect();
    let reads = db
        .conversation_reads(&ids)
        .await
        .map_err(|e| e.to_string())?;
    let spaces = db.intake_spaces().await.map_err(|e| e.to_string())?;
    // Newest first, so a backlog never holds up today's work.
    let to_label: Vec<FoundConversation> = kept
        .into_iter()
        .filter(|c| {
            found_session_id(c)
                .is_some_and(|id| needs_label(c, reads.get(&id)) && backoff.label_ready(&id, now))
        })
        .take(LABEL_BATCH)
        .collect();
    for conv in to_label {
        let answer = models
            .label
            .generate(LlmRequest {
                system_prompt: Some(
                    "You describe a person's conversations with AI tools. Answer with one line of JSON only."
                        .into(),
                ),
                user_prompt: intake::label_prompt(&conv, &spaces, language),
                max_tokens: 400,
                temperature: 0.0,
                label: Some("things_label".into()),
                timeout_secs: Some(120),
            })
            .await;
        let id = found_session_id(&conv).unwrap_or_default();
        let Some(label) = answer
            .ok()
            .and_then(|text| intake::parse_label(&text, &spaces))
        else {
            backoff.label_failed(id, now);
            continue;
        };
        backoff.label.remove(&id);
        let repo = conv.cwd.as_deref().and_then(intake::repo_root_of);
        db.record_conversation_label(&conv, &label, repo.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        report.labelled += 1;
        report.work += usize::from(label.work);
    }

    let pending = db
        .intake_pending(GROUP_BATCH)
        .await
        .map_err(|e| e.to_string())?;
    let open = db.intake_open_things().await.map_err(|e| e.to_string())?;
    // Nothing new to sort and no thing whose line is out of date.
    if pending.is_empty() && open.iter().all(|t| t.news.is_empty()) || now < backoff.group.1 {
        return Ok(report);
    }
    let corrections = db.intake_corrections().await.map_err(|e| e.to_string())?;
    let made = group(db, &pending, &open, &corrections, models, language).await;
    match made {
        Ok(made) => {
            backoff.group = (0, 0);
            report.things_made = made;
            report.grouped = pending.len();
            Ok(report)
        }
        Err(error) => {
            backoff.group_failed(now);
            Err(error)
        }
    }
}

/// Ask the grouping model and put the conversations where it says.
async fn group(
    db: &MemoryDB,
    pending: &[intake::Pending],
    open: &[intake::OpenThing],
    corrections: &[intake::Correction],
    models: &IntakeModels,
    language: &str,
) -> Result<usize, String> {
    let answer = models
        .group
        .generate(LlmRequest {
            system_prompt: Some(
                "You sort a person's conversations into the pieces of work they belong to. Answer with JSON only."
                    .into(),
            ),
            user_prompt: intake::group_prompt(pending, open, corrections, language),
            max_tokens: 4_000,
            temperature: 0.0,
            label: Some("things_group".into()),
            timeout_secs: Some(300),
        })
        .await
        .map_err(|e| format!("grouping: {e}"))?;
    let grouping = intake::parse_grouping(&answer, pending, open.len())
        .ok_or_else(|| "grouping: the answer was not JSON".to_string())?;
    db.apply_intake_grouping(pending, open, &grouping)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "things_intake_test.rs"]
mod tests;
