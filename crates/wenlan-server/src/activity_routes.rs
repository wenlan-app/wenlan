// SPDX-License-Identifier: Apache-2.0
//! `GET /api/activity` — one typed call describing all background organizing
//! work, grouped by asset (Memories, Entities, Pages).
//!
//! Data-layer only: counts from existing readers plus the resolved job routes.
//! Counts, enum labels and model ids only — never page or memory prose.

use crate::config_routes::{resolve_job_routes, JobRoute};
use crate::error::ServerError;
use crate::import_routes::DEFAULT_ACTIVE_IMPORT_BATCH_LIMIT;
use crate::refinery_routes::parse_action;
use crate::route_registry::{get, TrackedRouter};
use crate::scheduler::AmbientGateSnapshot;
use crate::state::SharedState;
use axum::{extract::State, response::Json};
use wenlan_core::config;
use wenlan_core::db::{ActivityCounts, EmbeddingBacklog};
use wenlan_types::activity::{
    ActivityAssetKind, ActivityAssetStatus, ActivityJob, ActivityLane, ActivityRefinement,
    ActivityRefinementGroup, ActivityResponse, ActivityRoute, ActivityState, ActivityStep,
    ActivityStepName, ActivityStepState,
};
use wenlan_types::import::ImportBatchStatus;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route("/api/activity", get(handle_activity))
}

/// GET /api/activity — background-work summary for the Activity surface.
pub async fn handle_activity(
    State(state): State<SharedState>,
) -> Result<Json<ActivityResponse>, ServerError> {
    // Snapshot everything the handler needs out of the state lock in one
    // short block, then drop the guard before any await.
    let cfg = config::load_config();
    let (db, everyday, synthesis, work_can_run) = {
        let s = state.read().await;
        let db = s.db.clone().ok_or(ServerError::DbNotInitialized)?;
        let (everyday, synthesis) = resolve_job_routes(&cfg, &s);
        // A `std::sync::Mutex`, locked and released within this statement.
        let work_can_run = s
            .ambient_gate
            .lock()
            .unwrap()
            .as_ref()
            .map(AmbientGateSnapshot::work_can_run);
        (db, everyday, synthesis, work_can_run)
    };
    let counts = db.activity_counts().await?;
    let batches = db
        .active_import_batches(DEFAULT_ACTIVE_IMPORT_BATCH_LIMIT)
        .await?;
    let mut response = compose_activity(counts, &batches, &everyday, &synthesis);
    response.state = final_state(
        &response,
        &batches,
        cfg.background_ai_enabled(),
        work_can_run,
    );
    Ok(Json(response))
}

/// The state the route reports: Off when background AI is off (whatever
/// runs, including the Embed catch-up, which needs no AI model), otherwise
/// the composed state refined by the scheduler's gate. Steps keep their own
/// states either way, so Embed reads Running in every mode.
fn final_state(
    response: &ActivityResponse,
    batches: &[ImportBatchStatus],
    background_ai_enabled: bool,
    work_can_run: Option<bool>,
) -> ActivityState {
    if background_ai_enabled {
        held_until_idle(
            response.state,
            work_can_run,
            gated_work_in_flight(response, batches),
        )
    } else {
        ActivityState::Off
    }
}

/// Organizing work that cannot run yet becomes WaitingForIdle. The scheduler
/// holds background work until the computer is quiet (idle input, spare CPU
/// and memory, nominal thermals) and publishes what its latest check saw,
/// including whether a recent import's short priority window could still run.
/// An unfinished import is not enough on its own: once that window ends or is
/// held, its work waits like everything else. Before the first check (`None`)
/// nothing is claimed. Every other state passes through: Blocked needs the
/// user, and UpToDate has nothing to hold.
///
/// `gated_work` is false when the only work in flight is the Embed catch-up,
/// which the gate does not hold: it is really running, so it stays
/// Organizing rather than claiming to wait for an idle computer.
fn held_until_idle(
    state: ActivityState,
    work_can_run: Option<bool>,
    gated_work: bool,
) -> ActivityState {
    match state {
        ActivityState::Organizing if gated_work && work_can_run == Some(false) => {
            ActivityState::WaitingForIdle
        }
        other => other,
    }
}

/// Whether any work the scheduler's gate holds is in flight: an unfinished
/// import batch or a running step other than Embed. Embed runs on the
/// built-in embedder under a maintenance lease, outside the gate.
fn gated_work_in_flight(response: &ActivityResponse, batches: &[ImportBatchStatus]) -> bool {
    batches.iter().any(|batch| !batch.complete)
        || response.assets.iter().any(|asset| {
            asset.steps.iter().any(|step| {
                step.name != ActivityStepName::Embed && step.state == ActivityStepState::Running
            })
        })
}

/// The Embed step: rows a big import stored without a vector, which the
/// built-in embedder catches up on in the background. Running with done of
/// total while rows wait, Idle at 0 of 0 otherwise. Never Blocked and never
/// failed: it needs no model lane, and a row it cannot embed is counted
/// again by the next run. Like the other steps, done is finished work and
/// total is done plus what is still waiting.
fn embed_step(backlog: EmbeddingBacklog) -> ActivityStep {
    let waiting = backlog.remaining > 0;
    ActivityStep {
        name: ActivityStepName::Embed,
        state: if waiting {
            ActivityStepState::Running
        } else {
            ActivityStepState::Idle
        },
        done: if waiting { backlog.done() } else { 0 },
        total: if waiting { backlog.total } else { 0 },
        failed: 0,
        job: ActivityStepName::Embed.job(),
    }
}

/// Done/failed/pending split for one group of `enrichment_steps` rows.
///
/// Step status vocabulary (from `import_batch_status`): `"ok"` and
/// `"skipped"` are done; `"failed"` and `"abandoned"` are failed; anything
/// else is still in flight.
#[derive(Debug, Default)]
struct StepTally {
    done: u64,
    failed: u64,
    pending: u64,
}

fn tally(counts: &ActivityCounts, step_names: &[&str]) -> StepTally {
    let mut tally = StepTally::default();
    for cell in &counts.step_counts {
        if !step_names.contains(&cell.step_name.as_str()) {
            continue;
        }
        match cell.status.as_str() {
            "ok" | "skipped" => tally.done += cell.count,
            "failed" | "abandoned" => tally.failed += cell.count,
            _ => tally.pending += cell.count,
        }
    }
    tally
}

/// Memories with no `enrichment_steps` row for this step name: work the step
/// has not started. Zero when the reader has no entry (defensive; the reader
/// always emits all four).
fn backlog(counts: &ActivityCounts, step_name: &str) -> u64 {
    counts
        .memories_without_step
        .iter()
        .find(|entry| entry.step_name == step_name)
        .map(|entry| entry.count)
        .unwrap_or(0)
}

/// Per-step liveness from its backlog and its lane: failed work blocks even
/// when the lane is healthy; waiting work blocks only when no lane serves it.
fn step_state(pending: u64, failed: u64, lane_available: bool) -> ActivityStepState {
    if failed > 0 || (pending > 0 && !lane_available) {
        ActivityStepState::Blocked
    } else if pending > 0 {
        ActivityStepState::Running
    } else {
        ActivityStepState::Idle
    }
}

fn rollup(states: &[ActivityStepState]) -> ActivityStepState {
    if states.contains(&ActivityStepState::Blocked) {
        ActivityStepState::Blocked
    } else if states.contains(&ActivityStepState::Running) {
        ActivityStepState::Running
    } else {
        ActivityStepState::Idle
    }
}

/// The asset's headline numbers follow the first step that is doing or
/// waiting on work, so the bar and the blocked count describe the same
/// thing. When every step is idle the asset rests on its settled step:
/// Summarize for Memories, Confirm for Entities, Write for Pages.
fn headline(steps: &[ActivityStep], resting: ActivityStepName) -> (u64, u64) {
    steps
        .iter()
        .find(|step| {
            matches!(
                step.state,
                ActivityStepState::Running | ActivityStepState::Blocked
            )
        })
        .or_else(|| steps.iter().find(|step| step.name == resting))
        .map(|step| (step.done, step.total))
        .unwrap_or((0, 0))
}

/// An asset's blocked count is an exact distinct-item count from the reader:
/// failed items when the lane can serve them (only failures are stuck),
/// unfinished items when it cannot (waiting work is stuck too). A memory
/// failed in one step and waiting in another counts once.
fn asset_blocked(backlog: &wenlan_core::db::AssetBacklog, lane_available: bool) -> u64 {
    if lane_available {
        backlog.failed
    } else {
        backlog.unfinished
    }
}

/// Map a `JobRoute.source` string onto its lane. Unknown sources fall to
/// `None`: an unrecognized lane must read as "no model", never as healthy.
fn lane_of(source: &str) -> ActivityLane {
    match source {
        "anthropic" => ActivityLane::Anthropic,
        "external" => ActivityLane::External,
        "on_device" => ActivityLane::OnDevice,
        "basic" => ActivityLane::Basic,
        "none" => ActivityLane::None,
        _ => ActivityLane::None,
    }
}

fn route_of(job: ActivityJob, route: &JobRoute) -> ActivityRoute {
    ActivityRoute {
        job,
        lane: lane_of(&route.source),
        model: route.model.clone(),
        mode: route.mode.clone(),
        available: route.mode == "pinned" && route.model.is_some(),
    }
}

/// Split the open refinement rows by whether the review queue lists them.
///
/// Ready for review is an `awaiting_review` row whose action
/// `refinery_routes::parse_action` knows, the same filter
/// `GET /api/refinery/queue` applies. Every other open row is not ready: a
/// `pending` row the refinery has not processed yet, or an `awaiting_review`
/// row in an action the queue cannot show (`community_split`, say), which no
/// list reaches. Known gap: the queue also skips a `lint_repair_review` row
/// whose payload does not parse, and this count cannot see payloads, so such
/// a row counts as ready although the list leaves it out.
///
/// Reported only. It never feeds `ActivityResponse.state`: some open rows
/// never move on their own and would pin the state to Blocked.
fn refinement_of(counts: &ActivityCounts) -> ActivityRefinement {
    let mut refinement = ActivityRefinement::default();
    for row in &counts.refinement_open {
        if row.status == "awaiting_review" && parse_action(&row.action).is_some() {
            refinement.ready_for_review += row.count;
        } else {
            refinement.not_ready += row.count;
        }
        refinement.groups.push(ActivityRefinementGroup {
            action: row.action.clone(),
            status: row.status.clone(),
            count: row.count,
        });
    }
    refinement
}

/// Assemble an `ActivityResponse` from raw counts. Pure: unit-testable without
/// a DB.
pub(crate) fn compose_activity(
    counts: ActivityCounts,
    batches: &[ImportBatchStatus],
    everyday: &JobRoute,
    synthesis: &JobRoute,
) -> ActivityResponse {
    let everyday_route = route_of(ActivityJob::Everyday, everyday);
    let synthesis_route = route_of(ActivityJob::Synthesis, synthesis);

    // ── Memories ────────────────────────────────────────────────────
    let store = ActivityStep {
        name: ActivityStepName::Store,
        state: ActivityStepState::Idle,
        done: counts.memories_total,
        total: counts.memories_total,
        failed: 0,
        job: ActivityStepName::Store.job(),
    };
    // Summarize (Enrich) is `title_enrich`; memories with no row for it are
    // still waiting on it, so the per-step backlog counts as pending here.
    let summarize_tally = tally(&counts, &["title_enrich"]);
    let summarize_pending = summarize_tally.pending + backlog(&counts, "title_enrich");
    let summarize = ActivityStep {
        name: ActivityStepName::Summarize,
        state: step_state(
            summarize_pending,
            summarize_tally.failed,
            everyday_route.available,
        ),
        done: summarize_tally.done,
        total: summarize_tally.done + summarize_tally.failed + summarize_pending,
        failed: summarize_tally.failed,
        job: ActivityStepName::Summarize.job(),
    };
    // Link is `page_growth`, with the same per-step backlog treatment as
    // Summarize: a memory with no `page_growth` row is waiting on Link even
    // when it already has rows for other steps.
    let link_tally = tally(&counts, &["page_growth"]);
    let link_pending = link_tally.pending + backlog(&counts, "page_growth");
    let link = ActivityStep {
        name: ActivityStepName::Link,
        state: step_state(link_pending, link_tally.failed, everyday_route.available),
        done: link_tally.done,
        total: link_tally.done + link_tally.failed + link_pending,
        failed: link_tally.failed,
        job: ActivityStepName::Link.job(),
    };
    let embed = embed_step(counts.embedding_backlog);
    // The headline leaves Embed out: it counts passages (rows), not
    // memories, so the asset's numbers stay in memories. Its Running state
    // still rolls up, because the work is real and runs in every mode.
    let (memories_done, memories_total) = headline(
        &[store.clone(), summarize.clone(), link.clone()],
        ActivityStepName::Summarize,
    );
    let memory_steps = vec![store, embed, summarize, link];
    let memories = ActivityAssetStatus {
        kind: ActivityAssetKind::Memories,
        state: rollup(
            &memory_steps
                .iter()
                .map(|step| step.state)
                .collect::<Vec<_>>(),
        ),
        done: memories_done,
        total: memories_total,
        blocked: asset_blocked(&counts.memories_backlog, everyday_route.available),
        steps: memory_steps,
    };

    // ── Entities ────────────────────────────────────────────────────
    // Detect's done/failed are exact distinct-memory counts from the reader:
    // done has BOTH `entity_extract` and `entity_link` done, failed has
    // EITHER failed without being done. Total is the memory population;
    // pending is the remainder.
    let detect_pending = counts
        .memories_total
        .saturating_sub(counts.detect.done + counts.detect.failed);
    let detect = ActivityStep {
        name: ActivityStepName::Detect,
        state: step_state(
            detect_pending,
            counts.detect.failed,
            everyday_route.available,
        ),
        done: counts.detect.done,
        total: counts.memories_total,
        failed: counts.detect.failed,
        job: ActivityStepName::Detect.job(),
    };
    // Confirm is user-driven in the Wiki: no lane, always Idle.
    let confirm = ActivityStep {
        name: ActivityStepName::Confirm,
        state: ActivityStepState::Idle,
        done: counts.entities_confirmed,
        total: counts.entities_detected + counts.entities_confirmed,
        failed: 0,
        job: ActivityStepName::Confirm.job(),
    };
    let entity_steps = vec![detect, confirm];
    let (entities_done, entities_total) = headline(&entity_steps, ActivityStepName::Confirm);
    let entities = ActivityAssetStatus {
        kind: ActivityAssetKind::Entities,
        state: rollup(&[entity_steps[0].state, entity_steps[1].state]),
        done: entities_done,
        total: entities_total,
        blocked: asset_blocked(&counts.entities_backlog, everyday_route.available),
        steps: entity_steps,
    };

    // ── Pages ───────────────────────────────────────────────────────
    // Write (Distill) is served by the synthesis lane. The three page counts
    // are NESTED — stale and refresh-blocked are subsets of the total — so
    // current pages are derived by subtraction, never by addition.
    // (Debug-asserted: the reader guarantees the nesting.)
    debug_assert!(counts.pages_stale + counts.pages_refresh_blocked <= counts.pages_total);
    let write_done = counts
        .pages_total
        .saturating_sub(counts.pages_stale + counts.pages_refresh_blocked);
    let write = ActivityStep {
        name: ActivityStepName::Write,
        state: step_state(
            counts.pages_stale,
            counts.pages_refresh_blocked,
            synthesis_route.available,
        ),
        done: write_done,
        total: counts.pages_total,
        failed: counts.pages_refresh_blocked,
        job: ActivityStepName::Write.job(),
    };
    let page_steps = vec![write];
    let (pages_done, pages_asset_total) = headline(&page_steps, ActivityStepName::Write);
    let pages = ActivityAssetStatus {
        kind: ActivityAssetKind::Pages,
        state: page_steps[0].state,
        done: pages_done,
        total: pages_asset_total,
        // One row per page: exact in both lane states.
        blocked: if synthesis_route.available {
            counts.pages_refresh_blocked
        } else {
            counts.pages_refresh_blocked + counts.pages_stale
        },
        steps: page_steps,
    };

    let refinement = refinement_of(&counts);

    // ── Overall ─────────────────────────────────────────────────────
    let assets = vec![memories, entities, pages];
    // Never WaitingForIdle: that needs the scheduler's gate, which the counts
    // do not carry, so the handler refines Organizing (see `held_until_idle`).
    let state = if assets
        .iter()
        .any(|asset| asset.state == ActivityStepState::Blocked)
    {
        ActivityState::Blocked
    } else if assets
        .iter()
        .any(|asset| asset.state == ActivityStepState::Running)
        || batches.iter().any(|batch| !batch.complete)
    {
        ActivityState::Organizing
    } else {
        ActivityState::UpToDate
    };

    ActivityResponse {
        state,
        last_activity_at: counts
            .last_work_at
            .max(batches.iter().map(|batch| batch.updated_at).max()),
        assets,
        everyday: everyday_route,
        synthesis: synthesis_route,
        refinement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;
    use wenlan_core::db::{AssetBacklog, RefinementOpenCount, StepBacklog, StepCount};

    fn job_route(source: &str, model: Option<&str>, mode: &str) -> JobRoute {
        JobRoute {
            source: source.to_string(),
            model: model.map(str::to_string),
            mode: mode.to_string(),
            pin: None,
        }
    }

    fn pinned_local() -> (JobRoute, JobRoute) {
        (
            job_route("on_device", Some("qwen3-4b"), "pinned"),
            job_route("on_device", Some("qwen3-4b"), "pinned"),
        )
    }

    fn empty_counts() -> ActivityCounts {
        ActivityCounts {
            memories_total: 0,
            step_counts: vec![],
            memories_without_step: vec![],
            detect: wenlan_core::db::DetectTally { done: 0, failed: 0 },
            memories_backlog: AssetBacklog {
                failed: 0,
                unfinished: 0,
            },
            entities_backlog: AssetBacklog {
                failed: 0,
                unfinished: 0,
            },
            entities_detected: 0,
            entities_confirmed: 0,
            pages_total: 0,
            pages_stale: 0,
            pages_refresh_blocked: 0,
            last_work_at: None,
            refinement_open: vec![],
            embedding_backlog: EmbeddingBacklog::default(),
        }
    }

    fn incomplete_batch() -> ImportBatchStatus {
        ImportBatchStatus {
            batch_id: "batch-1".to_string(),
            source: "other".to_string(),
            started_at: 1_700_000_000,
            updated_at: 1_700_000_100,
            chunks_received: 1,
            memories_imported: 10,
            memories_skipped: 0,
            entities_detected: 0,
            entities_established: 0,
            pages_distilled: 0,
            phases: vec![],
            complete: false,
            space: None,
        }
    }

    fn memories_asset(response: &ActivityResponse) -> &ActivityAssetStatus {
        &response.assets[0]
    }

    fn entities_asset(response: &ActivityResponse) -> &ActivityAssetStatus {
        &response.assets[1]
    }

    /// Eight memories with no step row for any step: every lane-served step
    /// has the whole population as backlog.
    fn unstarted_backlog() -> Vec<StepBacklog> {
        [
            "title_enrich",
            "page_growth",
            "entity_extract",
            "entity_link",
        ]
        .into_iter()
        .map(|step_name| StepBacklog {
            step_name: step_name.to_string(),
            count: 8,
        })
        .collect()
    }

    fn eight_memories_no_rows() -> ActivityCounts {
        ActivityCounts {
            memories_total: 8,
            step_counts: vec![],
            memories_without_step: unstarted_backlog(),
            detect: wenlan_core::db::DetectTally { done: 0, failed: 0 },
            memories_backlog: AssetBacklog {
                failed: 0,
                unfinished: 8,
            },
            entities_backlog: AssetBacklog {
                failed: 0,
                unfinished: 8,
            },
            ..empty_counts()
        }
    }

    fn pages_asset(response: &ActivityResponse) -> &ActivityAssetStatus {
        &response.assets[2]
    }

    /// A fresh install: no memories, no pages, and no model configured. The
    /// spec's headline edge case — nothing is waiting, so nothing is blocked.
    #[test]
    fn empty_db_with_unconfigured_lanes_is_up_to_date() {
        let everyday = job_route("basic", None, "unconfigured");
        let synthesis = job_route("none", None, "unconfigured");
        let response = compose_activity(empty_counts(), &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::UpToDate);
        for asset in &response.assets {
            assert_eq!(asset.state, ActivityStepState::Idle, "{:?}", asset.kind);
            assert_eq!(asset.blocked, 0, "{:?}", asset.kind);
            assert_eq!((asset.done, asset.total), (0, 0), "{:?}", asset.kind);
        }
    }

    /// `available` is a conjunction: a pinned route with no model cannot run,
    /// and a model on an unavailable pin cannot either. The other fixtures
    /// always vary both together, so only this test pins the conjunction.
    #[test]
    fn available_requires_both_pinned_and_a_model() {
        let pinned_without_model = job_route("on_device", None, "pinned");
        let model_without_pin = job_route("on_device", Some("qwen3-4b"), "pinned_unavailable");
        for broken in [pinned_without_model, model_without_pin] {
            let counts = ActivityCounts {
                memories_total: 8,
                memories_without_step: unstarted_backlog(),
                memories_backlog: AssetBacklog {
                    failed: 0,
                    unfinished: 8,
                },
                entities_backlog: AssetBacklog {
                    failed: 0,
                    unfinished: 8,
                },
                ..empty_counts()
            };
            let synthesis = job_route("on_device", Some("qwen3-4b"), "pinned");
            let response = compose_activity(counts, &[], &broken, &synthesis);
            assert!(!response.everyday.available, "{:?}", broken.mode);
            // Work is waiting on a lane that cannot run it.
            assert_eq!(response.state, ActivityState::Blocked, "{:?}", broken.mode);
            assert_eq!(memories_asset(&response).blocked, 8, "{:?}", broken.mode);
        }
    }

    /// The roll-up counts items, not step instances, and a memory stuck in
    /// two different steps still counts once. Five memories failed in
    /// Summarize and five DIFFERENT ones failed in Link is ten items, which
    /// only the reader's distinct-memory tally can see: a max across steps
    /// would say five and a sum would say ten only by accident.
    #[test]
    fn blocked_counts_distinct_memories_across_steps() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            memories_total: 10,
            step_counts: vec![
                StepCount {
                    step_name: "title_enrich".to_string(),
                    status: "failed".to_string(),
                    count: 5,
                },
                StepCount {
                    step_name: "page_growth".to_string(),
                    status: "failed".to_string(),
                    count: 5,
                },
            ],
            memories_backlog: AssetBacklog {
                failed: 10,
                unfinished: 10,
            },
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        let memories = memories_asset(&response);
        assert_eq!(memories.state, ActivityStepState::Blocked);
        assert_eq!(memories.blocked, 10);
    }

    #[test]
    fn empty_db_with_pinned_lanes_is_up_to_date() {
        let (everyday, synthesis) = pinned_local();
        let response = compose_activity(empty_counts(), &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::UpToDate);
        assert_eq!(response.last_activity_at, None);
        assert_eq!(
            response
                .assets
                .iter()
                .map(|asset| asset.kind)
                .collect::<Vec<_>>(),
            vec![
                ActivityAssetKind::Memories,
                ActivityAssetKind::Entities,
                ActivityAssetKind::Pages,
            ]
        );
        for asset in &response.assets {
            assert_eq!(asset.state, ActivityStepState::Idle);
            assert_eq!(asset.done, 0);
            assert_eq!(asset.total, 0);
            assert_eq!(asset.blocked, 0);
        }
        assert!(response.everyday.available);
        assert!(response.synthesis.available);
    }

    #[test]
    fn pending_enrichment_with_lane_is_organizing() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            memories_total: 10,
            step_counts: vec![StepCount {
                step_name: "title_enrich".to_string(),
                status: "pending".to_string(),
                count: 10,
            }],
            memories_backlog: AssetBacklog {
                failed: 0,
                unfinished: 10,
            },
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Organizing);
        let memories = memories_asset(&response);
        assert_eq!(memories.state, ActivityStepState::Running);
        assert_eq!(memories.total, 10);
        assert_eq!(memories.blocked, 0);
    }

    #[test]
    fn pending_enrichment_without_lane_is_blocked() {
        let everyday = job_route("on_device", None, "pinned_unavailable");
        let (_, synthesis) = pinned_local();
        let counts = ActivityCounts {
            memories_total: 10,
            step_counts: vec![StepCount {
                step_name: "title_enrich".to_string(),
                status: "pending".to_string(),
                count: 10,
            }],
            memories_backlog: AssetBacklog {
                failed: 0,
                unfinished: 10,
            },
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Blocked);
        let memories = memories_asset(&response);
        assert_eq!(memories.state, ActivityStepState::Blocked);
        assert_eq!(memories.blocked, 10);
    }

    #[test]
    fn failed_enrichment_with_lane_is_blocked() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            memories_total: 10,
            step_counts: vec![StepCount {
                step_name: "title_enrich".to_string(),
                status: "failed".to_string(),
                count: 2,
            }],
            memories_backlog: AssetBacklog {
                failed: 2,
                unfinished: 10,
            },
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Blocked);
        let memories = memories_asset(&response);
        assert_eq!(memories.state, ActivityStepState::Blocked);
        assert_eq!(memories.blocked, 2);
    }

    #[test]
    fn stale_pages_with_synthesis_lane_are_organizing() {
        let (everyday, _) = pinned_local();
        let synthesis = job_route("anthropic", Some("claude-sonnet-4-6"), "pinned");
        // 12 live pages, of which 3 stale: stale is a subset of the total,
        // not a second population.
        let counts = ActivityCounts {
            pages_total: 12,
            pages_stale: 3,
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Organizing);
        let pages = pages_asset(&response);
        assert_eq!(pages.state, ActivityStepState::Running);
        assert_eq!(pages.done, 9);
        assert_eq!(pages.total, 12);
        assert_eq!(pages.blocked, 0);
    }

    #[test]
    fn stale_pages_without_synthesis_lane_are_blocked() {
        let (everyday, _) = pinned_local();
        let synthesis = job_route("none", None, "unconfigured");
        let counts = ActivityCounts {
            pages_total: 12,
            pages_stale: 3,
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Blocked);
        let pages = pages_asset(&response);
        assert_eq!(pages.state, ActivityStepState::Blocked);
        assert_eq!((pages.done, pages.total), (9, 12));
        assert_eq!(pages.blocked, 3);
    }

    #[test]
    fn pages_total_does_not_double_count_stale() {
        let (everyday, _) = pinned_local();
        let synthesis = job_route("anthropic", Some("claude-sonnet-4-6"), "pinned");
        // 10 live pages: 6 current, 3 waiting, 1 refresh-blocked.
        let counts = ActivityCounts {
            pages_total: 10,
            pages_stale: 3,
            pages_refresh_blocked: 1,
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        let pages = pages_asset(&response);
        // One refresh-blocked page is a failed step, so Write is Blocked even
        // though the synthesis lane is available. The point of this test is
        // the arithmetic: 10 pages, not 13, and 6 current, not 10.
        assert_eq!(pages.state, ActivityStepState::Blocked);
        assert_eq!((pages.done, pages.total), (6, 10));
        assert_eq!(pages.blocked, 1);
    }

    #[test]
    fn incomplete_batch_with_zero_counts_is_organizing() {
        let (everyday, synthesis) = pinned_local();
        let batch = incomplete_batch();
        let response = compose_activity(empty_counts(), &[batch], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Organizing);
        // The batch's write is newer than any step row: it wins the timestamp.
        assert_eq!(response.last_activity_at, Some(1_700_000_100));
    }

    #[test]
    fn link_counts_memories_with_no_step_row() {
        let (everyday, synthesis) = pinned_local();
        let response = compose_activity(eight_memories_no_rows(), &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Organizing);
        let memories = memories_asset(&response);
        let link = memories
            .steps
            .iter()
            .find(|step| step.name == ActivityStepName::Link)
            .expect("Link step");
        assert_eq!(link.state, ActivityStepState::Running);
        assert_eq!((link.done, link.total, link.failed), (0, 8, 0));
        assert_eq!((memories.done, memories.total), (0, 8));
    }

    #[test]
    fn asset_headline_follows_the_live_step() {
        let everyday = job_route("basic", None, "unconfigured");
        let (_, synthesis) = pinned_local();
        let response = compose_activity(eight_memories_no_rows(), &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Blocked);
        // Detect is the live Entities step, so the asset reports Detect's
        // 0/8 — not Confirm's 0/0.
        let entities = entities_asset(&response);
        assert_eq!(entities.state, ActivityStepState::Blocked);
        assert_eq!((entities.done, entities.total), (0, 8));
    }

    #[test]
    fn blocked_counts_items_not_step_instances() {
        let everyday = job_route("basic", None, "unconfigured");
        let (_, synthesis) = pinned_local();
        let response = compose_activity(eight_memories_no_rows(), &[], &everyday, &synthesis);
        // Eight memories pending in both Summarize and Link is eight blocked
        // items, not sixteen.
        assert_eq!(memories_asset(&response).blocked, 8);
    }

    /// Steeping with the gate closed is waiting, not running: this is the
    /// sweeping clock beside "Last activity 2h ago".
    #[test]
    fn organizing_work_held_by_the_gate_is_waiting_for_idle() {
        use ActivityState::{Blocked, Organizing, UpToDate, WaitingForIdle};
        let gate = |admitted, import_priority_admitted| AmbientGateSnapshot {
            admitted,
            import_priority_admitted,
            blocked_reason: None,
            sampled_at_epoch: 0,
        };
        let cases = [
            (Organizing, Some(gate(false, false)), WaitingForIdle),
            // The gate admits work: it runs.
            (Organizing, Some(gate(true, false)), Organizing),
            // A recent import's priority window runs work past a closed gate.
            (Organizing, Some(gate(false, true)), Organizing),
            // No check yet: claim nothing.
            (Organizing, None, Organizing),
            // Only Steeping can wait; the other states say their own thing.
            (Blocked, Some(gate(false, false)), Blocked),
            (UpToDate, Some(gate(false, false)), UpToDate),
        ];
        for (state, gate, expected) in cases {
            let work_can_run = gate.as_ref().map(AmbientGateSnapshot::work_can_run);
            assert_eq!(
                held_until_idle(state, work_can_run, true),
                expected,
                "{state:?} gate={gate:?}"
            );
        }
    }

    #[test]
    fn idle_asset_rests_on_its_settled_step() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            memories_total: 8,
            step_counts: [
                "title_enrich",
                "page_growth",
                "entity_extract",
                "entity_link",
            ]
            .into_iter()
            .map(|step_name| StepCount {
                step_name: step_name.to_string(),
                status: "ok".to_string(),
                count: 8,
            })
            .collect(),
            // Every step row is ok, so the exact tallies agree: all 8
            // memories are finished in both lanes, nothing unfinished.
            detect: wenlan_core::db::DetectTally { done: 8, failed: 0 },
            entities_confirmed: 5,
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::UpToDate);
        // Every step idle: Entities rests on Confirm's 5/5, not Detect's 8/8.
        let entities = entities_asset(&response);
        assert_eq!(entities.state, ActivityStepState::Idle);
        assert_eq!((entities.done, entities.total), (5, 5));
    }

    fn open_row(action: &str, status: &str, count: u64) -> RefinementOpenCount {
        RefinementOpenCount {
            action: action.to_string(),
            status: status.to_string(),
            count,
        }
    }

    /// Ready for review is exactly what the review queue lists: awaiting
    /// review in an action it can show. A pending row and an awaiting row in
    /// an action the queue has no arm for are open but not ready.
    #[test]
    fn refinement_splits_ready_and_not_ready_by_review_queue_rule() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            refinement_open: vec![
                open_row("community_split", "awaiting_review", 2),
                open_row("entity_merge", "awaiting_review", 3),
                open_row("entity_merge", "pending", 4),
                open_row("page_merge", "awaiting_review", 5),
            ],
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.refinement.ready_for_review, 3 + 5);
        assert_eq!(response.refinement.not_ready, 2 + 4);
        assert_eq!(
            response
                .refinement
                .groups
                .iter()
                .map(|group| (group.action.as_str(), group.status.as_str(), group.count))
                .collect::<Vec<_>>(),
            vec![
                ("community_split", "awaiting_review", 2),
                ("entity_merge", "awaiting_review", 3),
                ("entity_merge", "pending", 4),
                ("page_merge", "awaiting_review", 5),
            ]
        );
    }

    /// Open suggestions that never move on their own must not pin the state:
    /// with every asset idle the library is still up to date.
    #[test]
    fn open_refinements_never_change_the_state() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            refinement_open: vec![
                open_row("community_split", "awaiting_review", 1),
                open_row("entity_merge", "pending", 7),
            ],
            ..empty_counts()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        for asset in &response.assets {
            assert_eq!(asset.state, ActivityStepState::Idle, "{:?}", asset.kind);
        }
        assert_eq!(response.refinement.not_ready, 8);
        assert_eq!(response.refinement.ready_for_review, 0);
        assert_eq!(response.state, ActivityState::UpToDate);
    }

    fn embedding_waiting(remaining: u64, total: u64) -> ActivityCounts {
        ActivityCounts {
            embedding_backlog: EmbeddingBacklog { remaining, total },
            ..empty_counts()
        }
    }

    fn embed_of(response: &ActivityResponse) -> &ActivityStep {
        memories_asset(response)
            .steps
            .iter()
            .find(|step| step.name == ActivityStepName::Embed)
            .expect("Embed step")
    }

    /// Rows waiting for a vector: Embed runs with done = total - remaining
    /// of the catch-up's total, has no lane and nothing failed, and the
    /// Memories headline stays in memories rather than passages.
    #[test]
    fn embed_step_runs_with_done_of_total_while_rows_wait() {
        let (everyday, synthesis) = pinned_local();
        let counts = ActivityCounts {
            memories_total: 40,
            step_counts: ["title_enrich", "page_growth"]
                .into_iter()
                .map(|step_name| StepCount {
                    step_name: step_name.to_string(),
                    status: "ok".to_string(),
                    count: 40,
                })
                .collect(),
            ..embedding_waiting(300, 1_000)
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        let embed = embed_of(&response);
        assert_eq!(embed.state, ActivityStepState::Running);
        assert_eq!((embed.done, embed.total, embed.failed), (700, 1_000, 0));
        assert_eq!(embed.job, None);
        let memories = memories_asset(&response);
        assert_eq!(
            memories
                .steps
                .iter()
                .map(|step| step.name)
                .collect::<Vec<_>>(),
            vec![
                ActivityStepName::Store,
                ActivityStepName::Embed,
                ActivityStepName::Summarize,
                ActivityStepName::Link,
            ]
        );
        assert_eq!(memories.state, ActivityStepState::Running);
        assert_eq!((memories.done, memories.total), (40, 40));
        assert_eq!(memories.blocked, 0);
        assert_eq!(response.state, ActivityState::Organizing);
    }

    #[test]
    fn embed_step_is_idle_at_zero_when_nothing_waits() {
        let (everyday, synthesis) = pinned_local();
        let response = compose_activity(empty_counts(), &[], &everyday, &synthesis);
        let embed = embed_of(&response);
        assert_eq!(embed.state, ActivityStepState::Idle);
        assert_eq!((embed.done, embed.total, embed.failed), (0, 0, 0));
        assert_eq!(response.state, ActivityState::UpToDate);
        // A stale total with nothing remaining reads as idle, not 1000 of 1000.
        let response = compose_activity(embedding_waiting(0, 1_000), &[], &everyday, &synthesis);
        let embed = embed_of(&response);
        assert_eq!(embed.state, ActivityStepState::Idle);
        assert_eq!((embed.done, embed.total), (0, 0));
    }

    /// Embed needs no model: with no lane configured it is still running,
    /// never blocked, and on its own never makes the library Blocked.
    #[test]
    fn embed_without_a_model_is_running_not_blocked() {
        let everyday = job_route("basic", None, "unconfigured");
        let synthesis = job_route("none", None, "unconfigured");
        let response = compose_activity(embedding_waiting(5, 5), &[], &everyday, &synthesis);
        assert_eq!(embed_of(&response).state, ActivityStepState::Running);
        assert_eq!(memories_asset(&response).state, ActivityStepState::Running);
        assert_eq!(memories_asset(&response).blocked, 0);
        assert_eq!(response.state, ActivityState::Organizing);
    }

    /// Embed running beside work a missing model blocks hides nothing: the
    /// library is still Blocked and the blocked count is the same.
    #[test]
    fn embed_does_not_hide_a_blocked_library() {
        let everyday = job_route("basic", None, "unconfigured");
        let (_, synthesis) = pinned_local();
        let counts = ActivityCounts {
            embedding_backlog: EmbeddingBacklog {
                remaining: 3,
                total: 9,
            },
            ..eight_memories_no_rows()
        };
        let response = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(response.state, ActivityState::Blocked);
        assert_eq!(memories_asset(&response).state, ActivityStepState::Blocked);
        assert_eq!(memories_asset(&response).blocked, 8);
        assert_eq!(embed_of(&response).state, ActivityStepState::Running);
    }

    /// The reported state in each mode while only the catch-up runs. Off
    /// stays Off (the step still says Running); with background AI on, a
    /// closed gate does not turn it into WaitingForIdle, because the gate
    /// does not hold the catch-up. Gated work beside it still waits.
    #[test]
    fn embed_keeps_each_mode_honest() {
        let (everyday, synthesis) = pinned_local();
        let closed = Some(false);
        let embed_only = compose_activity(embedding_waiting(10, 20), &[], &everyday, &synthesis);

        assert_eq!(
            final_state(&embed_only, &[], false, closed),
            ActivityState::Off
        );
        assert_eq!(embed_of(&embed_only).state, ActivityStepState::Running);
        assert_eq!(
            final_state(&embed_only, &[], true, closed),
            ActivityState::Organizing
        );
        assert_eq!(
            final_state(&embed_only, &[], true, Some(true)),
            ActivityState::Organizing
        );

        // An unfinished import batch is gated work: it waits.
        let batch = incomplete_batch();
        assert_eq!(
            final_state(&embed_only, std::slice::from_ref(&batch), true, closed),
            ActivityState::WaitingForIdle
        );

        // So is a running Summarize beside the catch-up.
        let counts = ActivityCounts {
            embedding_backlog: EmbeddingBacklog {
                remaining: 10,
                total: 20,
            },
            ..eight_memories_no_rows()
        };
        let both = compose_activity(counts, &[], &everyday, &synthesis);
        assert_eq!(both.state, ActivityState::Organizing);
        assert_eq!(
            final_state(&both, &[], true, closed),
            ActivityState::WaitingForIdle
        );
        assert_eq!(final_state(&both, &[], false, closed), ActivityState::Off);
    }

    /// The route itself: a big import's deferred rows show up as a running
    /// Embed step, read from the live counters, and the step goes idle when
    /// the catch-up ends.
    #[tokio::test]
    async fn activity_route_reports_rows_waiting_for_a_vector() {
        let tmp = tempfile::tempdir().unwrap();
        let emitter: Arc<dyn wenlan_core::events::EventEmitter> =
            Arc::new(wenlan_core::events::NoopEmitter);
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(tmp.path(), emitter)
                .await
                .unwrap(),
        );
        let written = db
            .upsert_documents_without_embeddings(vec![wenlan_core::sources::RawDocument {
                source: "memory".to_string(),
                source_id: "vault::big/note.md::0".to_string(),
                title: "note".to_string(),
                content: "a vault note stored first and embedded in the background".to_string(),
                last_modified: 1,
                ..Default::default()
            }])
            .await
            .unwrap();
        assert!(written > 0);
        let state = Arc::new(RwLock::new(crate::state::ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let get = || async {
            let app = crate::router::build_router(state.clone());
            let req = Request::builder()
                .method("GET")
                .uri("/api/activity")
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(req).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice::<ActivityResponse>(&body).unwrap()
        };

        let response = get().await;
        let embed = embed_of(&response);
        assert_eq!(embed.state, ActivityStepState::Running);
        assert_eq!((embed.done, embed.total), (0, written as u64));

        db.end_embedding_catch_up();
        let response = get().await;
        let embed = embed_of(&response);
        assert_eq!(embed.state, ActivityStepState::Idle);
        assert_eq!((embed.done, embed.total), (0, 0));
    }

    #[tokio::test]
    async fn activity_without_db_returns_503() {
        let state = Arc::new(RwLock::new(crate::state::ServerState::default()));
        let app = crate::router::build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/activity")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        // 503 = DbNotInitialized, reported when no database is attached.
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
