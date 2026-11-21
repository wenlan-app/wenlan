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
//!
//! At night the same grouping model reads the conversations of each thing
//! that went on that day and writes what the person settled and left open as
//! memories, so the work joins long-term memory while the thing stays on
//! home until it goes quiet (UX-CHAT-5). That model sees what the person
//! typed and the end of the AI's last reply, nothing else.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::state::SharedState;
use chrono::Timelike;
use wenlan_core::conversation_intake::{self as intake, FoundConversation};
use wenlan_core::db::derived_space::{rewrite_space, Place, SpaceFacts};
use wenlan_core::db::intake::{found_session_id, needs_label};
use wenlan_core::db::night_tidy::TidyApplied;
use wenlan_core::db::thing_dream::DreamDue;
use wenlan_core::db::thing_map::{DreamMapWrite, TidyMapWrite};
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_core::night_tidy;
use wenlan_types::requests::StoreMemoryRequest;
use wenlan_types::NightTidyKind;

/// How often conversations are looked at.
const TICK: Duration = Duration::from_secs(10 * 60);
/// The first look, once the daemon has settled after starting.
const FIRST_LOOK: Duration = Duration::from_secs(90);
/// Most conversations labelled in one look; the rest wait for the next.
const LABEL_BATCH: usize = 30;
/// Most conversations grouped in one call.
const GROUP_BATCH: usize = 60;
/// Most things read for points in one look.
const DREAM_BATCH: usize = 10;
/// The night, in local hours: when the day's conversations are read for
/// points.
const NIGHT_HOURS: std::ops::Range<u32> = 2..6;
/// At night, a conversation is read once it has been still this long.
const DREAM_SETTLED_SECS: i64 = 30 * 60;
/// By day, a conversation is read once it has been still this long, so a
/// computer that is off every night still gets its points.
const DREAM_DAYTIME_SECS: i64 = 24 * 60 * 60;
/// How far back transcripts are looked up for a night's reading.
const DREAM_LOOKUP_SECS: i64 = 30 * 24 * 60 * 60;
/// Rule 2: a conversation still for this long is considered stopped.
pub const HANDOFF_IDLE_SECS: i64 = 50 * 60;

/// Waits after a model failed, so a conversation the everyday model cannot
/// read, or a grouping answer that will not parse, does not spend the
/// person's quota every look. Kept in memory: a restart tries again.
#[derive(Debug, Default)]
pub struct Backoff {
    /// Per conversation: failures so far and when to try again.
    label: HashMap<String, (u32, i64)>,
    /// Grouping: failures so far and when to try again.
    group: (u32, i64),
    /// Reading for points: failures so far and when to try again.
    dream: (u32, i64),
    /// When the Brief pass may try again after a failure.
    plain: i64,
    plain_fails: u32,
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
    fn plain_failed(&mut self, now: i64) {
        self.plain_fails += 1;
        self.plain = now + wait_after(self.plain_fails - 1);
    }
    fn dream_failed(&mut self, now: i64) {
        self.dream.0 += 1;
        self.dream.1 = now + wait_after(self.dream.0 - 1);
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
    /// Things read for points, and the points written.
    pub dreamed: usize,
    pub points: usize,
    /// Spaces whose Brief got plain lines, and Brief items closed.
    pub briefs: usize,
    pub briefs_closed: usize,
    /// Brief items filled from an idle local transcript.
    pub handoffs_filled: usize,
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
        // UX-QUIET-1: reading conversations is light work that keeps
        // "Working on" current while the person works, so it runs while they
        // type, but not when the computer is warm, short of memory, or low on
        // battery.
        if !still_quiet() {
            tracing::debug!("[things] look deferred: warm, short of memory or low battery");
            continue;
        }
        match look_once(&shared, &mut backoff).await {
            Ok(Some(report))
                if report.labelled > 0
                    || report.grouped > 0
                    || report.dreamed > 0
                    || report.briefs > 0
                    || report.handoffs_filled > 0 =>
            {
                tracing::info!("[things] read conversations: {report:?}");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!("[things] reading conversations failed: {error}"),
        }
    }
}

/// The computer can still spare light background work right now: read
/// before a look and again between its model calls.
fn still_quiet() -> bool {
    crate::scheduler::light_background_may_run_now()
}

/// The models one look uses: the everyday model to label, and the one that
/// groups.
pub struct IntakeModels {
    pub label: Arc<dyn LlmProvider>,
    pub group: Arc<dyn LlmProvider>,
}

/// The models to use now, or `None` when background AI is off or a model is
/// missing.
pub(crate) async fn models(shared: &SharedState) -> Option<IntakeModels> {
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
    let group = group_from(&state, synthesis)?;
    Some(IntakeModels { label, group })
}

/// The model that groups, alone: work that never labels (the skills night
/// pass and its "Update now") must not wait for the everyday model, which on
/// an on-device setup loads only once the computer is idle.
pub(crate) async fn group_model(shared: &SharedState) -> Option<Arc<dyn LlmProvider>> {
    let config = wenlan_core::config::load_config();
    if !config.background_ai_enabled() {
        return None;
    }
    let synthesis =
        wenlan_core::refinery::SynthesisSource::parse(config.synthesis_source.as_deref());
    let state = shared.read().await;
    group_from(&state, synthesis)
}

fn group_from(
    state: &crate::state::ServerState,
    synthesis: Option<wenlan_core::refinery::SynthesisSource>,
) -> Option<Arc<dyn LlmProvider>> {
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
    (route.source == wenlan_core::refinery::SynthesisSource::Anthropic)
        .then(|| route.llm.cloned())
        .flatten()
        .or_else(|| {
            wenlan_core::agent_cli_llm::AgentCliProvider::find()
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
        })
}

/// The cutoff of the newest look at which the model a night reads with was
/// not there (0: none). In memory and per computer: whether this computer can
/// read right now is not a fact to share through the library.
static DREAMS_WAITING_SINCE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// What a look knows of the model the night needs: with it, nothing waits for
/// one; without it, whatever was quiet by now waits. The cutoff only moves
/// forward, so the daytime cutoff never undoes a night's.
fn track_waiting_for_model(group_model_available: bool, now: i64, local_hour: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    if group_model_available {
        DREAMS_WAITING_SINCE.store(0, Relaxed);
    } else {
        DREAMS_WAITING_SINCE.fetch_max(dream_quiet_since(now, local_hour), Relaxed);
    }
}

/// The cutoff before which due conversations are waiting for a model, if the
/// latest look had none.
pub fn dreams_waiting_since() -> Option<i64> {
    match DREAMS_WAITING_SINCE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        since => Some(since),
    }
}

/// One look, now. `None` when it did not run.
pub async fn look_once(
    shared: &SharedState,
    backoff: &mut Backoff,
) -> Result<Option<LookReport>, String> {
    let db = shared
        .read()
        .await
        .db
        .clone()
        .ok_or_else(|| "the database is not ready".to_string())?;
    let roots = wenlan_types::session_context::TranscriptRoots::from_env(dirs::home_dir());
    let now = chrono::Utc::now().timestamp();
    let found =
        tokio::task::spawn_blocking(move || intake::scan(&roots, now, intake::INTAKE_WINDOW_SECS))
            .await
            .map_err(|e| e.to_string())?;
    let models = models(shared).await;
    // UX-HAND-8: without a readable shared-Space list no Space is filled, so
    // private conversation text never reaches a team or folder-shared Space.
    // A fill problem skips only the fill; intake and the night pass go on.
    let language = intake::language_name(wenlan_core::config::load_config().ui_language.as_deref());
    let settings_path = wenlan_core::shared_space::settings_path();
    let handoffs_filled = match shared_space_names(&settings_path) {
        Ok(_) => {
            let shared_space = |space: &str| {
                shared_space_names(&settings_path)
                    .map(|spaces| spaces.contains(space))
                    .map_err(|error| error.to_string())
            };
            match db
                .fill_idle_handoffs(
                    &found,
                    &shared_space,
                    models.as_ref().map(|models| models.label.as_ref()),
                    language,
                    now,
                    HANDOFF_IDLE_SECS,
                )
                .await
            {
                Ok(filled) => filled,
                Err(error) => {
                    tracing::warn!("[things] idle handoff fill failed: {error}");
                    0
                }
            }
        }
        Err(error) => {
            tracing::warn!("[things] idle handoff fill skipped, shared Spaces unreadable: {error}");
            0
        }
    };

    let Some(models) = models else {
        // The night reads with the grouping model alone. Without the
        // everyday (labelling) model it does not run, but that is not a
        // wait for AI to be set up: only a missing grouping model, or
        // background AI turned off, is (UX-CHAT-5).
        track_waiting_for_model(
            group_model(shared).await.is_some(),
            now,
            chrono::Local::now().hour(),
        );
        // No everyday model yet (an on-device one waits for an idle
        // computer): the skills night pass only needs the grouping model.
        if NIGHT_HOURS.contains(&chrono::Local::now().hour()) && still_quiet() {
            if let Some(group) = group_model(shared).await {
                crate::skill_grow::spawn_nightly_if_idle(shared.clone(), group);
            }
        }
        return Ok((handoffs_filled > 0).then_some(LookReport {
            found: found.len(),
            handoffs_filled,
            ..LookReport::default()
        }));
    };
    // The grouping model is here, whether or not this look then reads.
    track_waiting_for_model(true, 0, 0);
    let mut report = look_with_until(&db, found, &models, language, backoff, &still_quiet).await?;
    report.handoffs_filled = handoffs_filled;
    if !still_quiet() {
        return Ok(Some(report));
    }
    let local_hour = chrono::Local::now().hour();
    let (dreamed, points) = dream_once(
        &db,
        &models,
        language,
        now,
        local_hour,
        backoff,
        &DaemonWriter(shared.clone()),
        &wenlan_core::config::load_config().knowledge_path_or_default(),
    )
    .await?;
    report.dreamed = dreamed;
    report.points = points;
    // UX-SPACE-2: at night, the Spaces whose Brief changed get their plain
    // lines, written by the model that groups (the person's cloud model or
    // their AI tool), which reads a long handoff well.
    if NIGHT_HOURS.contains(&local_hour) && now >= backoff.plain && still_quiet() {
        match crate::brief_plain_night::plain_once(&db, &models.group, language, now).await {
            Ok((spaces, closed)) => {
                report.briefs = spaces;
                report.briefs_closed = closed;
            }
            Err(error) => {
                tracing::warn!("[brief] plain lines failed: {error}");
                backoff.plain_failed(now);
            }
        }
    }
    // Following AI tools' memory, when the person turned it on: same model,
    // same quiet moment, at most once an hour.
    if let Err(error) = crate::ai_memory_import::follow_in_background(
        &db,
        models.group.as_ref(),
        &DaemonWriter(shared.clone()),
    )
    .await
    {
        tracing::warn!("[ai-memory] background pass: {error}");
    }
    if NIGHT_HOURS.contains(&local_hour) && still_quiet() {
        crate::skill_grow::spawn_nightly_if_idle(shared.clone(), models.group.clone());
    }
    Ok(Some(report))
}

fn shared_space_names(
    settings_path: &std::path::Path,
) -> Result<std::collections::BTreeSet<String>, wenlan_core::error::WenlanError> {
    Ok(wenlan_core::shared_space::shares_at(settings_path)?
        .into_iter()
        .filter_map(|(scope, _)| match scope {
            wenlan_core::shared_space::Scope::Space(name) => Some(name),
            _ => None,
        })
        .collect())
}

/// The new memory's id, or why it was not written.
pub type Written<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;

/// Writes one point as a memory.
pub trait PointWriter: Send + Sync {
    fn write(&self, request: StoreMemoryRequest) -> Written<'_>;
}

/// Writes through the same path as any memory an AI tool stores, so a point
/// is classified, linked into the map and checked like the rest. It is
/// written by Wenlan, not by the person.
pub struct DaemonWriter(pub SharedState);

impl PointWriter for DaemonWriter {
    fn write(&self, request: StoreMemoryRequest) -> Written<'_> {
        Box::pin(async move {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                "x-agent-name",
                axum::http::HeaderValue::from_static("wenlan"),
            );
            crate::memory_routes::handle_store_memory(
                axum::extract::State(self.0.clone()),
                headers,
                crate::space_header::SpaceHeader(None),
                axum::Json(request),
            )
            .await
            .map(|response| response.0.source_id)
            .map_err(|e| e.to_string())
        })
    }
}

/// When a conversation must have been still to be read for points: half an
/// hour at night, a day otherwise.
pub fn dream_quiet_since(now: i64, local_hour: u32) -> i64 {
    if NIGHT_HOURS.contains(&local_hour) {
        now - DREAM_SETTLED_SECS
    } else {
        now - DREAM_DAYTIME_SECS
    }
}

/// Read the things that went on for points, if any are due. Returns the
/// things read and the points written.
#[allow(clippy::too_many_arguments)]
async fn dream_once(
    db: &MemoryDB,
    models: &IntakeModels,
    language: &str,
    now: i64,
    local_hour: u32,
    backoff: &mut Backoff,
    writer: &dyn PointWriter,
    pages_folder: &std::path::Path,
) -> Result<(usize, usize), String> {
    if now < backoff.dream.1 {
        return Ok((0, 0));
    }
    let due = db
        .things_to_dream(dream_quiet_since(now, local_hour), DREAM_BATCH)
        .await
        .map_err(|e| e.to_string())?;
    if due.is_empty() {
        return Ok((0, 0));
    }
    let roots = wenlan_types::session_context::TranscriptRoots::from_env(dirs::home_dir());
    let found = tokio::task::spawn_blocking(move || intake::scan(&roots, now, DREAM_LOOKUP_SECS))
        .await
        .map_err(|e| e.to_string())?;
    let found: HashMap<String, FoundConversation> = found
        .into_iter()
        .filter_map(|c| found_session_id(&c).map(|id| (id, c)))
        .collect();
    match dream_with(
        db,
        &due,
        &found,
        models.group.as_ref(),
        language,
        writer,
        Some(pages_folder),
        &SpaceFacts::here(),
    )
    .await
    {
        Ok(done) => {
            backoff.dream = (0, 0);
            Ok(done)
        }
        Err(error) => {
            backoff.dream_failed(now);
            Err(error)
        }
    }
}

/// Writes a memory a night derived, unless the store already has that very
/// memory: one that stands, of the same kind, in the same Space, with the
/// same words up to case and spacing. That one is then what the night kept,
/// and the conversation counts as read. A longer or shorter memory that only
/// starts the same way is a different memory, and a forgotten or replaced
/// one is not a copy at all.
enum Kept {
    /// Written now: the night owns it (Undo forgets it).
    New(String),
    /// Already there: not the night's to forget. Linked to the conversation
    /// when it had none and is nobody's own.
    Already,
    /// The store refused it for good (too short, for one): it will be refused
    /// every night, so the conversation counts as read and the point is
    /// dropped.
    Dropped(String),
    /// Could not be written, perhaps for a moment; the conversation is read
    /// again next night.
    Failed(String),
}

async fn keep_memory(
    db: &MemoryDB,
    writer: &dyn PointWriter,
    request: StoreMemoryRequest,
    session_id: &str,
) -> Kept {
    let space = match &request.space {
        wenlan_types::WriteSpaceTarget::Named(name) => Some(name.clone()),
        _ => None,
    };
    let kind = request.memory_type.clone().unwrap_or_default();
    if let Ok(copies) = db.live_copies_of(&request.content, &kind).await {
        for (existing, existing_space) in copies {
            if same_write_space(db, existing_space.as_deref(), space.as_deref()).await {
                if let Err(error) = db.link_existing_memory(&existing, session_id).await {
                    tracing::warn!("[things] a memory already kept was not linked: {error}");
                }
                return Kept::Already;
            }
        }
    }
    let mut request = request;
    loop {
        let personal_retry = matches!(request.space, wenlan_types::WriteSpaceTarget::Named(_));
        return match writer.write(request.clone()).await {
            Ok(id) => Kept::New(id),
            // The store's own duplicate check: another write got there
            // first, or a memory that stands starts the way this one does.
            // What it says is already in the store (a shorter restatement
            // of a longer memory is not stored: a known limit), so the
            // conversation counts as read.
            Err(error) if error.contains("Duplicate:") => {
                tracing::info!("[things] the store already has what a point says: {error}");
                Kept::Already
            }
            // Refused for good, whatever is tried again: too short, or
            // nothing to index.
            Err(error)
                if error.contains("must be at least 10 characters")
                    || error.contains("no indexable content") =>
            {
                Kept::Dropped(error)
            }
            // The Space it was going to is gone or not registered: what it
            // says is still worth keeping, personal.
            Err(error)
                if personal_retry
                    && (error.contains("is not registered")
                        || error.contains("Selected Space no longer exists")) =>
            {
                tracing::warn!("[things] a point's Space is not there, kept personal: {error}");
                request.space = wenlan_types::WriteSpaceTarget::Uncategorized;
                continue;
            }
            Err(error) => Kept::Failed(error),
        };
    }
}

/// Whether two memories' Spaces are the same, `None` being none.
async fn same_write_space(db: &MemoryDB, a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => wenlan_core::sentences::same_space(db, Some(a), Some(b)).await,
        _ => false,
    }
}

/// Read each due thing's conversations once and keep what they settled, and
/// the preferences, tool problems and lessons they showed.
/// `found` holds the transcripts by session id; a conversation whose
/// transcript is gone is marked read with nothing taken from it. `facts` says
/// which Spaces are shared with other people and what the person set up that
/// ties a folder to a Space.
#[allow(clippy::too_many_arguments)]
pub async fn dream_with(
    db: &MemoryDB,
    due: &[DreamDue],
    found: &HashMap<String, FoundConversation>,
    model: &dyn LlmProvider,
    language: &str,
    writer: &dyn PointWriter,
    pages_folder: Option<&std::path::Path>,
    facts: &SpaceFacts,
) -> Result<(usize, usize), String> {
    let mut points_written = 0;
    // Where what each conversation settles may be written, decided before
    // any conversation is read with another: a label is only a model's guess,
    // so a conversation no fact of the person's setup backs for a shared Space
    // is read with the personal ones, never with that Space's. Computed once
    // per conversation (it reads folders and runs `git`).
    let mut targets: HashMap<(String, String), (wenlan_types::WriteSpaceTarget, Option<String>)> =
        HashMap::new();
    for thing in due {
        for c in &thing.conversations {
            let Some(found_c) = found.get(&c.session_id) else {
                continue;
            };
            let place = Place {
                thing_id: &thing.thing_id,
                session_id: &c.session_id,
                label: &c.space,
                cwd: found_c.cwd.as_deref(),
            };
            targets.insert(
                (thing.thing_id.clone(), c.session_id.clone()),
                db.derived_conversation_space(facts, &place).await,
            );
        }
    }
    let group_of = |thing_id: &str, c: &wenlan_core::db::thing_dream::DueConversation| -> String {
        targets
            .get(&(thing_id.to_string(), c.session_id.clone()))
            .and_then(|(_, named)| named.clone())
            .unwrap_or_default()
    };
    // One reading per Space a thing's conversations will be written to: what
    // was said in one Space never reaches the prompt that writes another
    // Space's points, so a personal conversation cannot leak into a shared
    // Space (design v5 D14). Conversations that write no Space are read
    // together.
    let groups: Vec<(
        &DreamDue,
        Vec<&wenlan_core::db::thing_dream::DueConversation>,
    )> = due
        .iter()
        .flat_map(|thing| {
            let mut spaces: Vec<String> = thing
                .conversations
                .iter()
                .map(|c| group_of(&thing.thing_id, c))
                .collect();
            spaces.sort();
            spaces.dedup();
            let group_of = &group_of;
            spaces.into_iter().map(move |space| {
                let in_space = thing
                    .conversations
                    .iter()
                    .filter(|c| group_of(&thing.thing_id, c) == space)
                    .collect();
                (thing, in_space)
            })
        })
        .collect();
    // The person's standing preferences, read once for the night.
    let night_preferences = db
        .dream_preferences(intake::MAX_PRIOR_PREFERENCES)
        .await
        .map_err(|e| e.to_string())?;
    for (done, (thing, in_space)) in groups.iter().enumerate() {
        let group_space = in_space
            .first()
            .map(|c| group_of(&thing.thing_id, c))
            .unwrap_or_default();
        let group_space = group_space.as_str();
        let conversations: Vec<(
            &wenlan_core::db::thing_dream::DueConversation,
            FoundConversation,
        )> = in_space
            .iter()
            .filter_map(|c| found.get(&c.session_id).map(|f| (*c, f.clone())))
            .collect();
        // A conversation whose transcript is gone is not marked read: it
        // waits until it goes on again, and the thing's page says it could
        // not be read (trust audit F6).
        let gone: Vec<wenlan_core::db::thing_dream::DueConversation> = in_space
            .iter()
            .filter(|c| !found.contains_key(&c.session_id))
            .map(|c| (*c).clone())
            .collect();
        db.mark_dream_unread(&gone)
            .await
            .map_err(|e| e.to_string())?;
        if conversations.is_empty() {
            continue;
        }
        let prior = db
            .dream_points_in(&thing.thing_id, group_space)
            .await
            .map_err(|e| e.to_string())?;
        // The tool problems and lessons this thing's nights kept in this
        // Space, and the person's standing preferences from every thing:
        // those are shown only to a reading whose Space is not shared with
        // other people, so what the person said about how they work cannot
        // be copied into a shared Space's points.
        let mut working_prior: Vec<intake::PriorWorking> = db
            .dream_working_in(&thing.thing_id, group_space)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|w| w.kind != intake::WorkingKind::Preference)
            .collect();
        if group_space.is_empty() || facts.is_shared(group_space) == Some(false) {
            working_prior.extend(night_preferences.iter().cloned());
        }
        // The thing's map: its points no night kept yet are offered, so a
        // point the model writes can say it is one of them (UX-CHAT-5).
        let map = db
            .dream_map_view(&thing.thing_id)
            .await
            .map_err(|e| e.to_string())?;
        // Its unkept points come from any of the thing's conversations, so a
        // reading whose Space is shared with other people is offered none.
        let map_shown = group_space.is_empty() || facts.is_shared(group_space) == Some(false);
        let map_points: Vec<intake::MapPoint> = map
            .unkept
            .iter()
            .filter(|_| map_shown)
            .map(|p| intake::MapPoint {
                text: p.label.clone(),
                decided: p.state.and_then(|s| match s {
                    wenlan_types::ThingMapState::Decided => Some(true),
                    wenlan_types::ThingMapState::Open => Some(false),
                    wenlan_types::ThingMapState::Dropped => None,
                }),
            })
            .collect();
        let read: Vec<FoundConversation> = conversations.iter().map(|(_, f)| f.clone()).collect();
        let lines = intake::dream_lines(&read);
        // The topics a point can connect to. Without a list the night still
        // keeps its points; they stay under their thing.
        // The Space this reading writes to, not what the conversations were
        // labelled (a label is a guess).
        let spaces: Vec<String> = Some(group_space)
            .filter(|space| !space.is_empty())
            .map(str::to_string)
            .into_iter()
            .collect();
        let topics = db
            .night_topics(&thing.title, &read, &prior, &spaces)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("[things] topics for {}: {error}", thing.thing_id);
                Vec::new()
            });
        let topic_names: Vec<String> = topics.iter().map(|t| t.name.clone()).collect();
        let answer = model
            .generate(LlmRequest {
                system_prompt: Some(
                    "You keep what a person settled in their work, so it can be remembered. Answer with JSON only."
                        .into(),
                ),
                user_prompt: intake::dream_prompt(
                    &thing.title,
                    &read,
                    &prior,
                    &working_prior,
                    &map_points,
                    &topic_names,
                    language,
                ),
                max_tokens: 3_000,
                temperature: 0.0,
                label: Some("things_dream".into()),
                timeout_secs: Some(300),
            })
            .await
            .map_err(|e| format!("reading {} for points: {e}", thing.thing_id));
        // A failure stops the night here; what was read stays read.
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) if done > 0 => {
                tracing::warn!("[things] {error}");
                return Ok((done, points_written));
            }
            Err(error) => return Err(error),
        };
        let line_owners: Vec<usize> = lines.iter().map(|(owner, _)| *owner).collect();
        let on_behalf: Vec<bool> = read.iter().map(|c| c.on_behalf).collect();
        let working_kinds: Vec<intake::WorkingKind> =
            working_prior.iter().map(|w| w.kind).collect();
        let Some(dream) = intake::parse_dream(
            &answer,
            &on_behalf,
            prior.len(),
            &working_kinds,
            &line_owners,
            map_points.len(),
            topics.len(),
        ) else {
            return if done > 0 {
                Ok((done, points_written))
            } else {
                Err("reading for points: the answer was not JSON".into())
            };
        };
        let mut decided = Vec::new();
        let mut open = Vec::new();
        let mut on_map = Vec::new();
        let mut links = Vec::new();
        // A conversation one of whose points could not be kept is not marked
        // read, so the next night reads it again instead of losing the point.
        let mut not_kept = std::collections::HashSet::new();
        // The points this group kept, and the memories they wrote or replaced:
        // a note citing one of those already follows it, so only the notes
        // that cite none are checked for a contradiction below.
        let mut kept_points: Vec<crate::things_read_pages::NewPoint> = Vec::new();
        let mut following = std::collections::HashSet::new();
        for point in dream.points {
            let (from, _) = &conversations[point.from];
            // The Space its label named; else the conversation's or the
            // thing's own; only then the default. A Space shared with other
            // people only when the person's own setup ties the conversation
            // to it (UX-CHAT-5).
            let (space, point_space) = targets
                .get(&(thing.thing_id.clone(), from.session_id.clone()))
                .cloned()
                .unwrap_or((wenlan_types::WriteSpaceTarget::Uncategorized, None));
            // A point replaces an earlier one only in the same Space.
            let replaces = match point.replaces {
                Some(p)
                    if same_write_space(db, prior[p].space.as_deref(), point_space.as_deref())
                        .await =>
                {
                    Some(p)
                }
                _ => None,
            };
            // The line it came from, only when it is in its own conversation.
            let line = point
                .line
                .and_then(|n| lines.get(n))
                .filter(|(owner, _)| *owner == point.from)
                .map(|(_, text)| text.clone());
            let request = StoreMemoryRequest {
                content: point.text.clone(),
                memory_type: Some(if point.decided { "decision" } else { "fact" }.into()),
                space,
                source_agent: None,
                // The point is its own title: short, and what the page lists.
                title: Some(point.text.clone()),
                confidence: None,
                supersedes: replaces.map(|p| prior[p].id.clone()),
                entity: None,
                entity_type: None,
                entity_id: None,
                structured_fields: None,
                retrieval_cue: None,
                said: None,
            };
            let id = match keep_memory(db, writer, request, &from.session_id).await {
                Kept::New(id) => id,
                // Already kept: nothing to record, and the conversation is read.
                Kept::Already => continue,
                Kept::Dropped(error) => {
                    tracing::info!("[things] a point was not kept: {error}");
                    continue;
                }
                Kept::Failed(error) => {
                    tracing::warn!(
                        "[things] a point of {} was not kept; its conversation is read again next night: {error}",
                        thing.thing_id
                    );
                    not_kept.insert(point.from);
                    continue;
                }
            };
            db.attach_dream_point(&id, &from.session_id, line.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            // Its topic, within its own Space. A link that fails leaves the
            // point under its thing; it does not lose the night.
            if let Some(topic) = point
                .topic
                .map(|t| &topics[t])
                .filter(|t| t.fits(point_space.as_deref()))
            {
                match db.link_night_point(&id, &topic.entity_id).await {
                    Ok(link) => links.extend(link),
                    Err(error) => tracing::warn!(
                        "[things] a point of {} was not connected: {error}",
                        thing.thing_id
                    ),
                }
            }
            // The map point it is: one not kept yet that the model named, or
            // the one the memory it replaces is.
            let node = point
                .map
                .map(|n| map.unkept[n].id.clone())
                .or_else(|| replaces.and_then(|p| map.by_memory.get(&prior[p].id).cloned()));
            kept_points.push(crate::things_read_pages::NewPoint {
                memory_id: id.clone(),
                text: point.text.clone(),
                space: point_space.clone(),
                session_id: from.session_id.clone(),
            });
            following.insert(id.clone());
            if let Some(p) = replaces {
                following.insert(prior[p].id.clone());
            }
            on_map.push(DreamMapWrite {
                memory_id: id.clone(),
                session_id: from.session_id.clone(),
                text: point.text.clone(),
                decided: point.decided,
                node,
                replaces: replaces.is_some(),
            });
            if point.decided {
                decided.push(id);
            } else {
                open.push(id);
            }
        }
        // What the person wants from their AI tools, the problems they hit
        // and what failed under which condition. Not points: they are not on
        // the map and not settled or open, only memories with their source.
        let mut working = Vec::new();
        for item in dream.working {
            let (from, _) = &conversations[item.from];
            // A preference is about the person, in all their projects: it is
            // kept outside every Space. A tool problem or a lesson follows a
            // point's rule.
            let space = if item.kind == intake::WorkingKind::Preference {
                db.derived_preference_space()
            } else {
                targets
                    .get(&(thing.thing_id.clone(), from.session_id.clone()))
                    .map(|(target, _)| target.clone())
                    .unwrap_or(wenlan_types::WriteSpaceTarget::Uncategorized)
            };
            // A tool problem or a lesson replaces an earlier one only in the
            // same Space (the same kind is checked when the answer is read).
            // A preference replaces any standing preference: they are all
            // kept outside every Space, even one an earlier night put in one.
            let replaces = match item.replaces {
                Some(w)
                    if item.kind == intake::WorkingKind::Preference
                        || same_write_space(
                            db,
                            working_prior[w].space.as_deref(),
                            space.named(),
                        )
                        .await =>
                {
                    Some(w)
                }
                _ => None,
            };
            let line = item
                .line
                .and_then(|n| lines.get(n))
                .filter(|(owner, _)| *owner == item.from)
                .map(|(_, text)| text.clone());
            let request = StoreMemoryRequest {
                content: item.text.clone(),
                memory_type: Some(item.kind.memory_type().into()),
                space,
                source_agent: None,
                title: Some(item.text.clone()),
                confidence: None,
                supersedes: replaces.map(|w| working_prior[w].id.clone()),
                entity: None,
                entity_type: None,
                entity_id: None,
                structured_fields: None,
                retrieval_cue: None,
                said: None,
            };
            let id = match keep_memory(db, writer, request, &from.session_id).await {
                Kept::New(id) => id,
                Kept::Already => continue,
                Kept::Dropped(error) => {
                    tracing::info!(
                        "[things] a {} was not kept: {error}",
                        item.kind.memory_type()
                    );
                    continue;
                }
                Kept::Failed(error) => {
                    tracing::warn!(
                        "[things] a {} of {} was not kept; its conversation is read again next night: {error}",
                        item.kind.memory_type(),
                        thing.thing_id
                    );
                    not_kept.insert(item.from);
                    continue;
                }
            };
            // Not confirmed: the person did not see it on a page, so it
            // waits for their OK like anything else an AI extracted.
            db.attach_dream_working(&id, &from.session_id, line.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            working.push(id);
        }
        points_written += decided.len() + open.len();
        let read_conversations: Vec<wenlan_core::db::thing_dream::DueConversation> = conversations
            .iter()
            .enumerate()
            .filter(|(i, _)| !not_kept.contains(i))
            .map(|(_, (c, _))| (*c).clone())
            .collect();
        let changed = db
            .dream_into_map(&thing.thing_id, &thing.title, &on_map)
            .await
            .map_err(|e| e.to_string())?;
        // Then tidy what the AI wrote around it (UX-GRAPH-7). A failure
        // here tidies nothing and does not stop the night. Each tidy keeps
        // its own map changes, so it can be taken back alone.
        // With a synced library only the main computer tidies: two computers
        // hiding the same memories could each keep a different one, and the
        // hides sync to both.
        let tidy_here = page_writing_here().await;
        let mut tidied = if tidy_here {
            tidy_thing(db, &thing.thing_id, model, writer).await
        } else {
            Vec::new()
        };
        for tidy in &mut tidied {
            let write = TidyMapWrite {
                old: tidy.old.clone(),
                new: tidy.new.clone(),
                text: tidy.text.clone(),
            };
            tidy.map = db
                .tidy_into_map(&thing.thing_id, std::slice::from_ref(&write))
                .await
                .map_err(|e| e.to_string())?;
        }
        let night = db
            .record_dream_full(
                &thing.thing_id,
                &decided,
                &open,
                &working,
                &changed,
                &links,
                &read_conversations,
                &tidied,
            )
            .await
            .map_err(|e| e.to_string())?;
        // Then the notes these conversations only read: one sentence of such a
        // note changes only when a point just kept clearly contradicts it
        // (design v5 D15). Nothing here fails the night or un-reads a
        // conversation.
        let session_ids: Vec<&str> = conversations
            .iter()
            .map(|(c, _)| c.session_id.as_str())
            .collect();
        // Page text, proposals included, is written only by the main computer
        // of a shared library (the same gate as the tidy above).
        let read_pages = if tidy_here {
            crate::things_read_pages::check_read_pages(
                db,
                &session_ids,
                &kept_points,
                &following,
                model,
                pages_folder,
            )
            .await
        } else {
            Default::default()
        };
        if read_pages != Default::default() {
            tracing::info!(
                "[things] {}: {} sentence(s) changed, {} proposed, {} revision card(s) staged on notes that were read",
                thing.thing_id,
                read_pages.changed,
                read_pages.proposed,
                read_pages.staged
            );
        }
        // Then what the work has come to, from every point that stands now,
        // once the night is safely recorded: a failure here loses no point
        // and does not stop the night (UX-VIEW-6).
        if let Some(seq) = night {
            if let Err(error) =
                write_conclusion(db, &thing.thing_id, &thing.title, seq, model, language).await
            {
                tracing::warn!("[things] no conclusion for {}: {error}", thing.thing_id);
            }
        }
    }
    Ok((due.len(), points_written))
}

#[cfg(test)]
tokio::task_local! {
    /// Lets a test play a computer that is not the main one of a shared library.
    pub(crate) static NOT_THE_MAIN_COMPUTER: bool;
}

/// Whether AI may write page text on this computer: always, unless it shares
/// the library and another computer is the main one.
async fn page_writing_here() -> bool {
    // A test never reads the real data root's shared-library settings: it is
    // the main computer unless it says otherwise.
    #[cfg(test)]
    return !NOT_THE_MAIN_COMPUTER
        .try_with(|not_main| *not_main)
        .unwrap_or(false);
    #[cfg(not(test))]
    tokio::task::spawn_blocking(|| {
        wenlan_core::shared_space::library::page_writing_allowed_at(
            &wenlan_core::shared_space::settings_path(),
        )
    })
    .await
    .unwrap_or(false)
}

/// Tidy the memories an AI wrote around one thing's night (UX-GRAPH-7):
/// one call with what [`MemoryDB::tidy_offer`] found, then the merges and
/// updates are written through the same path as a point. What they take the
/// place of is hidden only when the night is recorded. Empty when there is
/// nothing to tidy or the call fails.
pub async fn tidy_thing(
    db: &MemoryDB,
    thing_id: &str,
    model: &dyn LlmProvider,
    writer: &dyn PointWriter,
) -> Vec<TidyApplied> {
    let now = chrono::Utc::now().timestamp();
    let offer = match db.tidy_offer(thing_id, now).await {
        Ok(offer) => offer,
        Err(error) => {
            tracing::warn!("[things] nothing to tidy for {thing_id}: {error}");
            return Vec::new();
        }
    };
    if offer.groups.is_empty() {
        return Vec::new();
    }
    let answer = model
        .generate(LlmRequest {
            system_prompt: Some(
                "You tidy the memories an AI keeps for a person, changing only what you are sure of. Answer with JSON only."
                    .into(),
            ),
            user_prompt: night_tidy::tidy_prompt(&offer.items, &offer.groups, now),
            max_tokens: 1_500,
            temperature: 0.0,
            label: Some("things_tidy".into()),
            timeout_secs: Some(300),
        })
        .await;
    let changes = match answer {
        Ok(answer) => night_tidy::parse_tidy(&answer, &offer.items, now).unwrap_or_default(),
        Err(error) => {
            tracing::warn!("[things] tidying {thing_id}: {error}");
            return Vec::new();
        }
    };
    let mut tidied = Vec::new();
    for change in changes {
        let (how, olds, text) = match change {
            night_tidy::TidyChange::Merge { items, text } => {
                (NightTidyKind::Merge, items.to_vec(), Some(text))
            }
            night_tidy::TidyChange::Update { item, text } => {
                (NightTidyKind::Update, vec![item], Some(text))
            }
            night_tidy::TidyChange::Replace { keep, drop } => {
                tidied.push(TidyApplied {
                    how: NightTidyKind::Replace,
                    old: vec![offer.items[drop].id.clone()],
                    new: offer.items[keep].id.clone(),
                    wrote: false,
                    text: None,
                    map: Default::default(),
                });
                continue;
            }
        };
        let Some(text) = text else { continue };
        let old: Vec<&night_tidy::TidyItem> = olds.iter().map(|&i| &offer.items[i]).collect();
        // The newest of them gives the source the new memory keeps.
        let source = old
            .iter()
            .filter(|item| item.session_id.is_some())
            .max_by_key(|item| item.created_at)
            .copied();
        let request = StoreMemoryRequest {
            content: text.clone(),
            memory_type: Some(
                match old[0].working_kind() {
                    // Merged only with its own kind, so it keeps what it is.
                    Some(kind) => kind.memory_type(),
                    None if old.iter().any(|item| item.decided) => "decision",
                    None => "fact",
                }
                .into(),
            ),
            // What it takes the place of is where it stays.
            space: rewrite_space(old[0].space.as_deref()),
            source_agent: None,
            title: Some(text.clone()),
            confidence: None,
            supersedes: None,
            entity: None,
            entity_type: None,
            entity_id: None,
            structured_fields: None,
            retrieval_cue: None,
            said: None,
        };
        let id = match writer.write(request).await {
            Ok(id) => id,
            Err(error) => {
                tracing::warn!("[things] a tidy of {thing_id} was not written: {error}");
                continue;
            }
        };
        if let Err(error) = db
            .adopt_tidy_memory(
                &id,
                source.and_then(|item| item.session_id.as_deref()),
                source.and_then(|item| item.line.as_deref()),
                old[0].working_kind().is_none(),
            )
            .await
        {
            tracing::warn!("[things] a tidy of {thing_id} kept no source: {error}");
        }
        tidied.push(TidyApplied {
            how,
            old: old.iter().map(|item| item.id.clone()).collect(),
            new: id,
            wrote: true,
            text: Some(text),
            map: Default::default(),
        });
    }
    tidied
}

/// Write a thing's conclusion onto the night `seq`: at most two sentences
/// from its standing points, each resting on points that exist (checked by
/// [`intake::parse_conclusion`]). No sentence survives, no conclusion.
async fn write_conclusion(
    db: &MemoryDB,
    thing_id: &str,
    title: &str,
    seq: i64,
    model: &dyn LlmProvider,
    language: &str,
) -> Result<(), String> {
    let standing = db.dream_points(thing_id).await.map_err(|e| e.to_string())?;
    let points = intake::conclusion_points(&standing);
    if points.is_empty() {
        return Ok(());
    }
    let answer = model
        .generate(LlmRequest {
            system_prompt: Some(
                "You sum up where a person's work has got to, from what they settled. Answer with JSON only."
                    .into(),
            ),
            user_prompt: intake::conclusion_prompt(title, points, language),
            max_tokens: 600,
            temperature: 0.0,
            label: Some("things_conclusion".into()),
            timeout_secs: Some(120),
        })
        .await
        .map_err(|e| format!("writing the conclusion: {e}"))?;
    let sentences = intake::parse_conclusion(&answer, points.len())
        .ok_or_else(|| "the conclusion answer was not JSON".to_string())?;
    let conclusion: Vec<wenlan_types::ThingConclusionSentence> = sentences
        .into_iter()
        .map(|s| wenlan_types::ThingConclusionSentence {
            text: s.text,
            points: s.points.into_iter().map(|i| points[i].id.clone()).collect(),
        })
        .collect();
    if conclusion.is_empty() {
        return Ok(());
    }
    db.set_dream_conclusion(thing_id, seq, &conclusion)
        .await
        .map_err(|e| e.to_string())
}

/// Connect the night points kept before nights named topics, once, through the
/// same list and links a night uses. Only the person starts it. A thing whose
/// model call fails is counted, and the run can be started again; connected
/// points are not asked about twice. Undo: `MemoryDB::undo_night_links_backfill`.
pub async fn connect_old_night_points(
    db: &MemoryDB,
    model: &dyn LlmProvider,
) -> Result<wenlan_types::NightLinksBackfill, String> {
    // One run at a time: a client that gives up and asks again must not
    // start a second run that calls the model for every thing again.
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("connecting the old points is already running".into());
    }
    let _running = Running;
    let mut report = wenlan_types::NightLinksBackfill::default();
    if db
        .night_links_backfilled()
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        report.already_done = true;
        return Ok(report);
    }
    for (thing_id, title) in db.things_with_nights().await.map_err(|e| e.to_string())? {
        let points = db
            .unlinked_night_points(&thing_id)
            .await
            .map_err(|e| e.to_string())?;
        if points.is_empty() {
            continue;
        }
        report.things += 1;
        report.points += points.len();
        let mut spaces: Vec<String> = points.iter().filter_map(|p| p.space.clone()).collect();
        spaces.sort();
        spaces.dedup();
        let as_prior: Vec<intake::PriorPoint> = points
            .iter()
            .map(|p| intake::PriorPoint {
                id: p.id.clone(),
                text: p.text.clone(),
                decided: false,
                space: p.space.clone(),
            })
            .collect();
        let topics = db
            .night_topics(&title, &[], &as_prior, &spaces)
            .await
            .map_err(|e| e.to_string())?;
        if topics.is_empty() {
            continue;
        }
        let texts: Vec<String> = points.iter().map(|p| p.text.clone()).collect();
        let names: Vec<String> = topics.iter().map(|t| t.name.clone()).collect();
        let answer = model
            .generate(LlmRequest {
                system_prompt: Some(
                    "You connect a person's kept points to the topics they are about. Answer with JSON only."
                        .into(),
                ),
                user_prompt: intake::night_links_prompt(&title, &texts, &names),
                max_tokens: 2_000,
                temperature: 0.0,
                label: Some("things_night_links".into()),
                timeout_secs: Some(300),
            })
            .await;
        let pairs = match answer {
            Ok(text) => {
                let pairs = intake::parse_night_links(&text, points.len(), topics.len());
                if pairs.is_none() {
                    tracing::warn!(
                        "[things] connecting old points of {thing_id}: unreadable answer"
                    );
                }
                pairs
            }
            Err(e) => {
                tracing::warn!("[things] connecting old points of {thing_id} failed: {e}");
                None
            }
        };
        let Some(pairs) = pairs else {
            report.failed += 1;
            continue;
        };
        let mut links = Vec::new();
        for (point, topic) in pairs {
            let (point, topic) = (&points[point], &topics[topic]);
            if !topic.fits(point.space.as_deref()) {
                continue;
            }
            match db.link_night_point(&point.id, &topic.entity_id).await {
                Ok(link) => links.extend(link),
                Err(e) => {
                    // Keep the links already made undoable: record them
                    // below, and leave this thing to the next run.
                    tracing::warn!("[things] connecting a night point of {thing_id} failed: {e}");
                    report.failed += 1;
                    break;
                }
            }
        }
        report.linked += links.len();
        db.record_night_links_backfill(&thing_id, &links)
            .await
            .map_err(|e| e.to_string())?;
    }
    if report.failed == 0 {
        db.mark_night_links_backfilled(report.linked)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(report)
}

/// Most paraphrase pairs one call about the old memories looks at.
const OLD_PASS_PAIRS: usize = 10;
/// Most memories about a passed date one call says again.
const OLD_PASS_PASSED: usize = 12;

/// The one-time pass over old memories (UX-GRAPH-7), once the owner OK'd its
/// report: exact copies are hidden behind the earliest; a model is asked
/// whether each paraphrase pair says the same, and a pair it merges is
/// hidden the same way (nothing is rewritten); an AI's memory about a passed
/// date is said again in the past tense, through the same path as a night's
/// update. Then [`MemoryDB::apply_backlog`] records it all at once, with one
/// Undo. A call that fails leaves its pairs or memories as they are.
pub async fn run_old_memory_pass(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    writer: &dyn PointWriter,
) -> Result<wenlan_core::db::night_backlog::BacklogPass, String> {
    use wenlan_core::db::night_backlog::{BacklogMemory, BacklogPlan};
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("the old-memory pass is already running".into());
    }
    let _running = Running;
    if db
        .backlog_pass()
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("the old-memory pass already ran; take it back first".into());
    }
    let now = chrono::Utc::now().timestamp();
    let report = db.backlog_report(now).await.map_err(|e| e.to_string())?;
    let item = |m: &BacklogMemory| night_tidy::TidyItem {
        id: m.id.clone(),
        text: m.text.clone(),
        space: m.space.clone(),
        created_at: m.created_at,
        event_date: m.event_date,
        decided: false,
        memory_type: "fact".into(),
        session_id: None,
        line: None,
    };
    let ask = |items: Vec<night_tidy::TidyItem>, groups: Vec<Vec<usize>>| async move {
        let answer = model
            .generate(LlmRequest {
                system_prompt: Some(
                    "You tidy the memories an AI keeps for a person, changing only what you are sure of. Answer with JSON only."
                        .into(),
                ),
                user_prompt: night_tidy::tidy_prompt(&items, &groups, now),
                max_tokens: 3_000,
                temperature: 0.0,
                label: Some("things_old_pass".into()),
                timeout_secs: Some(300),
            })
            .await;
        match answer {
            Ok(text) => night_tidy::parse_tidy(&text, &items, now).map(|changes| (items, changes)),
            Err(error) => {
                tracing::warn!("[things] old-memory pass call failed: {error}");
                None
            }
        }
    };
    let mut plan = BacklogPlan::default();
    let mut paraphrases = Vec::new();
    for pair in report.hideable() {
        if pair.exact {
            plan.hide.push((pair.keep.id.clone(), pair.hide.id.clone()));
        } else {
            paraphrases.push(pair);
        }
    }
    for batch in paraphrases.chunks(OLD_PASS_PAIRS) {
        let items: Vec<_> = batch
            .iter()
            .flat_map(|p| [item(&p.keep), item(&p.hide)])
            .collect();
        let groups = (0..batch.len()).map(|i| vec![2 * i, 2 * i + 1]).collect();
        let Some((_, changes)) = ask(items, groups).await else {
            continue;
        };
        for change in changes {
            // "These two say the same" or "the newer says the older no
            // longer holds" is taken, and only as hiding the older behind
            // the newer (`keep` comes first in each group): an old memory is
            // never rewritten here.
            let same_pair = match change {
                night_tidy::TidyChange::Merge { items: [a, b], .. } if a / 2 == b / 2 => {
                    Some(a / 2)
                }
                night_tidy::TidyChange::Replace { keep, drop }
                    if keep % 2 == 0 && drop == keep + 1 =>
                {
                    Some(keep / 2)
                }
                _ => None,
            };
            if let Some(i) = same_pair {
                let pair = batch[i];
                plan.hide.push((pair.keep.id.clone(), pair.hide.id.clone()));
            }
        }
    }
    let passed: Vec<&BacklogMemory> = report
        .passed
        .iter()
        .filter(|m| m.author == wenlan_types::authorship::SentenceAuthor::Ai)
        .collect();
    for batch in passed.chunks(OLD_PASS_PASSED) {
        let items: Vec<_> = batch.iter().map(|m| item(m)).collect();
        let groups = (0..batch.len()).map(|i| vec![i]).collect();
        let Some((items, changes)) = ask(items, groups).await else {
            continue;
        };
        for change in changes {
            let night_tidy::TidyChange::Update { item: i, text } = change else {
                continue;
            };
            let old = &items[i];
            let detail = db.get_memory_detail(&old.id).await.ok().flatten();
            let request = StoreMemoryRequest {
                content: text.clone(),
                // The kind it was: a preference, tool problem or lesson an AI
                // tool saved stays one, not a fact.
                memory_type: Some(
                    detail
                        .as_ref()
                        .and_then(|d| d.memory_type.clone())
                        .filter(|t| {
                            t == "decision" || intake::WorkingKind::from_memory_type(t).is_some()
                        })
                        .unwrap_or_else(|| "fact".into()),
                ),
                space: rewrite_space(old.space.as_deref()),
                source_agent: None,
                title: Some(text.clone()),
                confidence: None,
                supersedes: None,
                entity: None,
                entity_type: None,
                entity_id: None,
                structured_fields: None,
                retrieval_cue: None,
                said: None,
            };
            let id = match writer.write(request).await {
                Ok(id) => id,
                Err(error) => {
                    tracing::warn!("[things] an old memory was not said again: {error}");
                    continue;
                }
            };
            let session = detail.as_ref().and_then(|d| d.session_id.as_deref());
            // It stands for what was confirmed only when that was: a working
            // memory the person has not said yes to still waits for it.
            let confirm = detail.as_ref().is_none_or(|d| {
                d.confirmed
                    || d.memory_type
                        .as_deref()
                        .is_none_or(|t| intake::WorkingKind::from_memory_type(t).is_none())
            });
            if let Err(error) = db.adopt_tidy_memory(&id, session, None, confirm).await {
                tracing::warn!("[things] an old memory's update kept no source: {error}");
            }
            plan.updated.push((old.id.clone(), id));
        }
    }
    db.apply_backlog(&plan, &SpaceFacts::here())
        .await
        .map_err(|e| e.to_string())
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
    look_with_until(db, found, models, language, backoff, &|| true).await
}

/// [`look_with`], stopping before the next model call once `keep_going`
/// says the computer is warm, short of memory or low on battery
/// (UX-QUIET-1).
pub async fn look_with_until(
    db: &MemoryDB,
    found: Vec<FoundConversation>,
    models: &IntakeModels,
    language: &str,
    backoff: &mut Backoff,
    keep_going: &(dyn Fn() -> bool + Sync),
) -> Result<LookReport, String> {
    let now = chrono::Utc::now().timestamp();
    let mut report = LookReport {
        found: found.len(),
        ..Default::default()
    };
    let (kept, _dropped) = intake::keep_work(found);
    report.kept = kept.len();
    let ids: Vec<String> = kept.iter().filter_map(found_session_id).collect();
    // What each worked on is read from the transcript, with no model: kept
    // for every conversation already read, not only the ones read again.
    let touched: Vec<(String, Vec<String>)> = kept
        .iter()
        .filter_map(|c| found_session_id(c).map(|id| (id, c.touched.clone())))
        .collect();
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
        if !keep_going() {
            return Ok(report);
        }
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
        let Some(mut label) = answer
            .ok()
            .and_then(|text| intake::parse_label(&text, &spaces))
        else {
            backoff.label_failed(id, now);
            continue;
        };
        backoff.label.remove(&id);
        // A conversation whose AI kept a diagram of the work is work, so the
        // diagram lands under a thing even when the model judged it a test
        // or a one-off (UX-ART-4). A title is still needed to name it.
        if !label.work
            && !label.title.is_empty()
            && db
                .originals_for_conversation(&conv.tool, &conv.external_id)
                .await
                .is_ok_and(|kept| !kept.is_empty())
        {
            label.work = true;
        }
        let repo = conv.cwd.as_deref().and_then(intake::repo_root_of);
        db.record_conversation_label(&conv, &label, repo.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        report.labelled += 1;
        report.work += usize::from(label.work);
    }

    db.record_conversation_touched(&touched)
        .await
        .map_err(|e| e.to_string())?;
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
