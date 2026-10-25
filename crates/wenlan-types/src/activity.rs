// SPDX-License-Identifier: Apache-2.0
//! Wire types for `GET /api/activity` — one call describing all background
//! organizing work, grouped by asset (Memories, Entities, Pages), plus the
//! refinement suggestions still open.
//!
//! Counts, enum labels and model ids only: never page or memory prose, so the
//! route is truth-manifest `NotApplicable` like `/api/config/routing`.

use serde::{Deserialize, Serialize};

/// Overall background-work state. Precedence: Blocked over Organizing over
/// WaitingForIdle over UpToDate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    UpToDate,
    /// Background AI was explicitly turned off; stored notes remain usable.
    Off,
    /// Work is waiting and can run now.
    Organizing,
    /// Work is waiting but cannot run yet: the scheduler's latest resource
    /// check held background work (the computer is in use, busy, low on
    /// memory or hot) and no import is bypassing that check. Only ever
    /// replaces Organizing.
    WaitingForIdle,
    Blocked,
    /// A state this build does not know, sent by a newer daemon. The daemon
    /// never sends it; it lets an older reader keep the rest of the response
    /// instead of failing to parse it. Every enum in this response has one.
    #[serde(other)]
    Unknown,
}

/// The three asset groups the Activity surface reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityAssetKind {
    Memories,
    Entities,
    Pages,
    /// A value from a newer daemon; see `ActivityState::Unknown`.
    #[serde(other)]
    Unknown,
}

/// One background step inside an asset. `Store`, `Embed` and `Confirm` finish
/// without a model lane (the import request stores; the built-in embedder
/// prepares; the user confirms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStepName {
    Store,
    /// Memory rows a big import stored without a vector, which the built-in
    /// embedder prepares for search by meaning in the background. Keyword
    /// search already finds them. Counts rows (passages), not memories, and
    /// runs in every mode: it needs neither the user's AI model nor
    /// background AI, and is never blocked.
    Embed,
    Summarize,
    Link,
    Detect,
    Confirm,
    Write,
    /// A value from a newer daemon; see `ActivityState::Unknown`.
    #[serde(other)]
    Unknown,
}

impl ActivityStepName {
    /// The job whose resolved lane serves this step; `None` for the laneless
    /// steps `Store` (done inside the import request), `Embed` (the built-in
    /// embedder) and `Confirm` (user-driven in the Wiki).
    pub fn job(self) -> Option<ActivityJob> {
        match self {
            ActivityStepName::Summarize | ActivityStepName::Link | ActivityStepName::Detect => {
                Some(ActivityJob::Everyday)
            }
            ActivityStepName::Write => Some(ActivityJob::Synthesis),
            ActivityStepName::Store
            | ActivityStepName::Embed
            | ActivityStepName::Confirm
            | ActivityStepName::Unknown => None,
        }
    }
}

/// Per-step liveness. There is no queued/pending variant: anything not done
/// and not failed is in flight while its lane is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStepState {
    Idle,
    Running,
    Blocked,
    /// A value from a newer daemon; see `ActivityState::Unknown`.
    #[serde(other)]
    Unknown,
}

/// The two background job classes, served by the resolved routing lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityJob {
    Everyday,
    Synthesis,
    /// A value from a newer daemon; see `ActivityState::Unknown`.
    #[serde(other)]
    Unknown,
}

/// Resolved lane serving a job. Mirrors `JobRoute.source` strings verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLane {
    OnDevice,
    External,
    Anthropic,
    Basic,
    None,
    /// A value from a newer daemon; see `ActivityState::Unknown`.
    #[serde(other)]
    Unknown,
}

/// One resolved job route: which lane serves the job and whether it can run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityRoute {
    pub job: ActivityJob,
    pub lane: ActivityLane,
    pub model: Option<String>,
    /// "pinned" | "pinned_unavailable" | "unconfigured", verbatim from routing.
    pub mode: String,
    /// True only when `mode == "pinned"` and `model.is_some()`.
    pub available: bool,
}

/// Progress of one background step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityStep {
    pub name: ActivityStepName,
    pub state: ActivityStepState,
    pub done: u64,
    pub total: u64,
    pub failed: u64,
    /// The job whose lane serves this step; `None` for Store, Embed and
    /// Confirm.
    pub job: Option<ActivityJob>,
}

/// Progress of one asset group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityAssetStatus {
    pub kind: ActivityAssetKind,
    pub state: ActivityStepState,
    pub done: u64,
    pub total: u64,
    /// Exact count of items that cannot proceed: items with a failed step
    /// when the asset's lane is available, plus items still waiting when it
    /// is not. Each item counts once no matter how many of its steps are
    /// stuck.
    pub blocked: u64,
    pub steps: Vec<ActivityStep>,
}

/// Refinement suggestions still open (`pending` or `awaiting_review`).
/// Reported only: never changes `ActivityResponse.state`, because some open
/// rows never move on their own and would pin the state to Blocked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActivityRefinement {
    /// Open rows the review queue lists now: `awaiting_review` in an action
    /// it can show.
    pub ready_for_review: u64,
    /// Every other open row: not yet processed, or waiting in an action the
    /// review queue cannot show.
    pub not_ready: u64,
    /// Open rows by action and status, verbatim labels, for Diagnostics.
    pub groups: Vec<ActivityRefinementGroup>,
}

/// One `(action, status)` cell of the open refinement rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityRefinementGroup {
    pub action: String,
    pub status: String,
    pub count: u64,
}

/// The `GET /api/activity` response body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityResponse {
    pub state: ActivityState,
    /// Unix seconds of the newest enrichment step write; None on an empty DB.
    pub last_activity_at: Option<i64>,
    /// Always three entries in order Memories, Entities, Pages.
    pub assets: Vec<ActivityAssetStatus>,
    pub everyday: ActivityRoute,
    pub synthesis: ActivityRoute,
    /// Open refinement suggestions. Defaults to all zero when absent, so this
    /// build still reads a daemon from before the field existed.
    #[serde(default)]
    pub refinement: ActivityRefinement,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_wire_labels_are_snake_case() {
        assert_eq!(
            serde_json::to_value(ActivityState::UpToDate).unwrap(),
            serde_json::Value::String("up_to_date".to_string())
        );
        assert_eq!(
            serde_json::to_value(ActivityLane::OnDevice).unwrap(),
            serde_json::Value::String("on_device".to_string())
        );
        for (state, label) in [
            (ActivityState::UpToDate, "up_to_date"),
            (ActivityState::Off, "off"),
            (ActivityState::Organizing, "organizing"),
            (ActivityState::WaitingForIdle, "waiting_for_idle"),
            (ActivityState::Blocked, "blocked"),
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, format!("\"{label}\""));
        }
    }

    /// An app from before `Embed` reads it as `Unknown`, not a parse failure.
    #[test]
    fn embed_step_wire_label() {
        assert_eq!(
            serde_json::to_value(ActivityStepName::Embed).unwrap(),
            serde_json::Value::String("embed".to_string())
        );
        let back: ActivityStepName = serde_json::from_str("\"embed\"").unwrap();
        assert_eq!(back, ActivityStepName::Embed);
    }

    #[test]
    fn activity_step_job_mapping() {
        assert_eq!(
            ActivityStepName::Summarize.job(),
            Some(ActivityJob::Everyday)
        );
        assert_eq!(ActivityStepName::Link.job(), Some(ActivityJob::Everyday));
        assert_eq!(ActivityStepName::Detect.job(), Some(ActivityJob::Everyday));
        assert_eq!(ActivityStepName::Write.job(), Some(ActivityJob::Synthesis));
        assert_eq!(ActivityStepName::Store.job(), None);
        assert_eq!(ActivityStepName::Embed.job(), None);
        assert_eq!(ActivityStepName::Confirm.job(), None);
        assert_eq!(ActivityStepName::Unknown.job(), None);
    }

    #[test]
    fn activity_response_round_trips_with_mode_passthrough() {
        let response = ActivityResponse {
            state: ActivityState::Blocked,
            last_activity_at: Some(1_700_000_000),
            assets: vec![ActivityAssetStatus {
                kind: ActivityAssetKind::Pages,
                state: ActivityStepState::Blocked,
                done: 12,
                total: 15,
                blocked: 3,
                steps: vec![ActivityStep {
                    name: ActivityStepName::Write,
                    state: ActivityStepState::Blocked,
                    done: 12,
                    total: 15,
                    failed: 0,
                    job: Some(ActivityJob::Synthesis),
                }],
            }],
            everyday: ActivityRoute {
                job: ActivityJob::Everyday,
                lane: ActivityLane::OnDevice,
                model: Some("qwen3-4b".to_string()),
                mode: "pinned".to_string(),
                available: true,
            },
            synthesis: ActivityRoute {
                job: ActivityJob::Synthesis,
                lane: ActivityLane::Anthropic,
                model: None,
                // Opaque routing label: passed through verbatim, never parsed.
                mode: "pinned_unavailable".to_string(),
                available: false,
            },
            refinement: ActivityRefinement {
                ready_for_review: 2,
                not_ready: 5,
                groups: vec![
                    ActivityRefinementGroup {
                        action: "community_split".to_string(),
                        status: "awaiting_review".to_string(),
                        count: 1,
                    },
                    ActivityRefinementGroup {
                        action: "entity_merge".to_string(),
                        status: "pending".to_string(),
                        count: 4,
                    },
                ],
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"pinned_unavailable\""));
        assert!(json.contains("\"on_device\""));
        assert!(json.contains("\"ready_for_review\":2"));
        let back: ActivityResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back, response);
    }

    /// A daemon from before `refinement` existed still parses, with every
    /// suggestion count at zero.
    #[test]
    fn activity_response_without_refinement_defaults_to_zero() {
        let json = r#"{"state":"up_to_date","last_activity_at":null,"assets":[],
            "everyday":{"job":"everyday","lane":"on_device","model":"m","mode":"pinned","available":true},
            "synthesis":{"job":"synthesis","lane":"on_device","model":"m","mode":"pinned","available":true}}"#;
        let response: ActivityResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.refinement, ActivityRefinement::default());
        assert_eq!(response.refinement.ready_for_review, 0);
        assert_eq!(response.refinement.not_ready, 0);
        assert!(response.refinement.groups.is_empty());
    }

    /// An app older than its daemon must not lose the whole response to one
    /// word it has never seen, in any of the response's enums.
    #[test]
    fn activity_response_with_newer_words_still_parses() {
        let json = r#"{"state":"new_state","last_activity_at":null,
            "assets":[{"kind":"new_kind","state":"new_step_state","done":2,"total":2,"blocked":0,
                "steps":[{"name":"new_step","state":"idle","done":2,"total":2,"failed":0,"job":"new_job"}]}],
            "everyday":{"job":"everyday","lane":"new_lane","model":"m","mode":"pinned","available":true},
            "synthesis":{"job":"synthesis","lane":"on_device","model":"m","mode":"pinned","available":true}}"#;
        let response: ActivityResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.state, ActivityState::Unknown);
        let asset = &response.assets[0];
        assert_eq!(asset.kind, ActivityAssetKind::Unknown);
        assert_eq!(asset.state, ActivityStepState::Unknown);
        assert_eq!(asset.total, 2);
        assert_eq!(asset.steps[0].name, ActivityStepName::Unknown);
        assert_eq!(asset.steps[0].job, Some(ActivityJob::Unknown));
        assert_eq!(response.everyday.lane, ActivityLane::Unknown);
        assert_eq!(response.synthesis.lane, ActivityLane::OnDevice);

        // The app hands this struct on to the webview, which reads "unknown".
        let passed_on = serde_json::to_value(&response).unwrap();
        assert_eq!(passed_on["state"], "unknown");
        assert_eq!(passed_on["assets"][0]["kind"], "unknown");
        assert_eq!(passed_on["assets"][0]["state"], "unknown");
        assert_eq!(passed_on["assets"][0]["steps"][0]["name"], "unknown");
        assert_eq!(passed_on["assets"][0]["steps"][0]["job"], "unknown");
        assert_eq!(passed_on["everyday"]["lane"], "unknown");
    }
}
