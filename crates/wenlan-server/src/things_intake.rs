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
use wenlan_core::db::intake::{found_session_id, needs_label};
use wenlan_core::db::thing_dream::DreamDue;
use wenlan_core::db::thing_map::DreamMapWrite;
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_types::requests::StoreMemoryRequest;
use wenlan_types::WriteSpaceTarget;

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
            Ok(Some(report)) if report.labelled > 0 || report.grouped > 0 || report.dreamed > 0 => {
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
    let mut report = look_with_until(&db, found, &models, language, backoff, &still_quiet).await?;
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
    )
    .await?;
    report.dreamed = dreamed;
    report.points = points;
    Ok(Some(report))
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
async fn dream_once(
    db: &MemoryDB,
    models: &IntakeModels,
    language: &str,
    now: i64,
    local_hour: u32,
    backoff: &mut Backoff,
    writer: &dyn PointWriter,
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
    match dream_with(db, &due, &found, models.group.as_ref(), language, writer).await {
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

/// Read each due thing's conversations once and keep what they settled.
/// `found` holds the transcripts by session id; a conversation whose
/// transcript is gone is marked read with nothing taken from it.
pub async fn dream_with(
    db: &MemoryDB,
    due: &[DreamDue],
    found: &HashMap<String, FoundConversation>,
    model: &dyn LlmProvider,
    language: &str,
    writer: &dyn PointWriter,
) -> Result<(usize, usize), String> {
    let mut points_written = 0;
    for (done, thing) in due.iter().enumerate() {
        let conversations: Vec<(
            &wenlan_core::db::thing_dream::DueConversation,
            FoundConversation,
        )> = thing
            .conversations
            .iter()
            .filter_map(|c| found.get(&c.session_id).map(|f| (c, f.clone())))
            .collect();
        if conversations.is_empty() {
            db.record_dream(
                &thing.thing_id,
                &[],
                &[],
                &Default::default(),
                &[],
                &thing.conversations,
            )
            .await
            .map_err(|e| e.to_string())?;
            continue;
        }
        let prior = db
            .dream_points(&thing.thing_id)
            .await
            .map_err(|e| e.to_string())?;
        // The thing's map: its points no night kept yet are offered, so a
        // point the model writes can say it is one of them (UX-CHAT-5).
        let map = db
            .dream_map_view(&thing.thing_id)
            .await
            .map_err(|e| e.to_string())?;
        let map_points: Vec<intake::MapPoint> = map
            .unkept
            .iter()
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
        // The topics a point can connect to. Without a list the night still
        // keeps its points; they stay under their thing.
        let mut spaces: Vec<String> = conversations
            .iter()
            .filter(|(c, _)| !c.space.is_empty())
            .map(|(c, _)| c.space.clone())
            .collect();
        spaces.sort();
        spaces.dedup();
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
                    &map_points,
                    &topic_names,
                    language,
                ),
                max_tokens: 2_000,
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
        let Some(points) = intake::parse_dream(
            &answer,
            read.len(),
            prior.len(),
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
        for point in points {
            let (from, _) = &conversations[point.from];
            let space = if from.space.is_empty() {
                WriteSpaceTarget::Inherit
            } else {
                WriteSpaceTarget::Named(from.space.clone())
            };
            let request = StoreMemoryRequest {
                content: point.text.clone(),
                memory_type: Some(if point.decided { "decision" } else { "fact" }.into()),
                space,
                source_agent: None,
                // The point is its own title: short, and what the page lists.
                title: Some(point.text.clone()),
                confidence: None,
                supersedes: point.replaces.map(|p| prior[p].id.clone()),
                entity: None,
                entity_type: None,
                entity_id: None,
                structured_fields: None,
                retrieval_cue: None,
            };
            let id = match writer.write(request).await {
                Ok(id) => id,
                Err(error) => {
                    tracing::warn!(
                        "[things] a point of {} was not kept: {error}",
                        thing.thing_id
                    );
                    continue;
                }
            };
            db.attach_dream_point(&id, &from.session_id)
                .await
                .map_err(|e| e.to_string())?;
            // Its topic, within its own Space. A link that fails leaves the
            // point under its thing; it does not lose the night.
            let point_space = (!from.space.is_empty()).then_some(from.space.as_str());
            if let Some(topic) = point
                .topic
                .map(|t| &topics[t])
                .filter(|t| t.fits(point_space))
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
            let node = point.map.map(|n| map.unkept[n].id.clone()).or_else(|| {
                point
                    .replaces
                    .and_then(|p| map.by_memory.get(&prior[p].id).cloned())
            });
            on_map.push(DreamMapWrite {
                memory_id: id.clone(),
                session_id: from.session_id.clone(),
                text: point.text.clone(),
                decided: point.decided,
                node,
                replaces: point.replaces.is_some(),
            });
            if point.decided {
                decided.push(id);
            } else {
                open.push(id);
            }
        }
        points_written += decided.len() + open.len();
        let changed = db
            .dream_into_map(&thing.thing_id, &thing.title, &on_map)
            .await
            .map_err(|e| e.to_string())?;
        let night = db
            .record_dream(
                &thing.thing_id,
                &decided,
                &open,
                &changed,
                &links,
                &thing.conversations,
            )
            .await
            .map_err(|e| e.to_string())?;
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
