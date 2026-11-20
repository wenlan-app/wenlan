// SPDX-License-Identifier: Apache-2.0
//! Person-confirmed local settings writes through the generic file applier.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use axum::{http::HeaderMap, Json};
use serde::Serialize;
use wenlan_core::{
    rulesync_personal,
    rulesync_renderer::PINNED_VERSION,
    rulesync_settings::{self, RenderedSettings},
};
use wenlan_types::{
    rulesync::RulesSyncReport,
    rulesync_settings::{SettingsApplyRequest, SettingsPlan, SettingsPreviewRequest},
};

use crate::{
    error::ServerError,
    route_registry::{post, TrackedRouter},
    rulesync_apply::{ApplyOutcome, FileRoot, Snapshot},
    rulesync_routes::{require_local_person, Setup},
    state::SharedState,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/rules/setup/settings/preview", post(preview))
        .route("/api/rules/setup/settings/apply", post(apply))
}

async fn preview(
    headers: HeaderMap,
    Json(request): Json<SettingsPreviewRequest>,
) -> Result<Json<SettingsPlan>, ServerError> {
    execute(headers, request.source, |_, plan| Ok(plan.response)).await
}

async fn apply(
    headers: HeaderMap,
    Json(request): Json<SettingsApplyRequest>,
) -> Result<Json<RulesSyncReport>, ServerError> {
    execute(headers, request.source, move |root, plan| {
        apply_plan(root, plan, &request.token)
    })
    .await
}

async fn execute<T: Send + 'static>(
    headers: HeaderMap,
    source: PathBuf,
    finish: impl FnOnce(&FileRoot, LocalPlan) -> Result<T> + Send + 'static,
) -> Result<Json<T>, ServerError> {
    require_local_person(&headers)?;
    if !rulesync_personal::enabled() {
        return Err(ServerError::ValidationError(
            "WENLAN_RULESYNC is off".into(),
        ));
    }

    let result = tokio::task::spawn_blocking(move || {
        let setup = Setup::system()?;
        let _lock = setup.lock()?;
        let root = FileRoot::open(&setup.home).context("open local AI-tool home")?;
        let rendered = rulesync_settings::render_source(&setup.binary, &setup.data, &source)?;
        let plan = plan_rendered(&root, &setup.home, &source, rendered)?;
        finish(&root, plan)
    })
    .await
    .map_err(|_| ServerError::Internal("settings task failed".into()))?;
    result
        .map(Json)
        .map_err(|error| ServerError::Conflict(format!("{error:#}")))
}

#[derive(Serialize)]
struct PlannedWrite {
    path: PathBuf,
    seen: Snapshot,
    desired: Snapshot,
}

struct LocalPlan {
    response: SettingsPlan,
    writes: Vec<PlannedWrite>,
}

fn plan_rendered(
    root: &FileRoot,
    home: &Path,
    source: &Path,
    rendered: RenderedSettings,
) -> Result<LocalPlan> {
    let mut files = Vec::new();
    let mut writes = Vec::new();
    for (path, staged) in rendered.files {
        let seen = root
            .read(&path, false)
            .with_context(|| format!("read existing {}", path.display()))?;
        let current = match &seen {
            Snapshot::Absent => None,
            Snapshot::File { bytes, .. } => Some(bytes.as_slice()),
            _ => bail!("{} must be a regular settings file", path.display()),
        };
        let change = rulesync_settings::merge_file(&path, current, &staged)
            .with_context(|| format!("merge {}", path.display()))?;
        if let Some(change) = change {
            let desired = match &seen {
                Snapshot::File { mode, .. } => Snapshot::File {
                    bytes: change.bytes,
                    mode: *mode,
                },
                _ => Snapshot::file(change.bytes),
            };
            files.push(change.preview);
            writes.push(PlannedWrite {
                path,
                seen,
                desired,
            });
        }
    }
    let token = rulesync_settings::confirmation_token(&(
        PINNED_VERSION,
        home,
        source,
        rendered.source_digest,
        &writes,
    ))?;
    Ok(LocalPlan {
        response: SettingsPlan { token, files },
        writes,
    })
}

fn apply_plan(root: &FileRoot, plan: LocalPlan, token: &str) -> Result<RulesSyncReport> {
    ensure!(
        plan.response.token == token,
        "settings changed since preview. Run the preview again"
    );
    // Check the whole plan before writing its first file. FileRoot rechecks
    // each snapshot again at the write boundary.
    for write in &plan.writes {
        ensure!(
            root.read(&write.path, false)? == write.seen,
            "{} changed since preview. Run the preview again",
            write.path.display()
        );
    }
    let mut report = RulesSyncReport {
        written: Vec::new(),
        blocked: Vec::new(),
        proposals: Vec::new(),
        message: String::new(),
    };
    for write in plan.writes {
        let path = write.path.display();
        if !report.blocked.is_empty() {
            report.blocked.push(format!(
                "{path}: preview again after resolving the write failure"
            ));
            continue;
        }
        match root.apply(&write.path, &write.seen, &write.desired, false) {
            Ok(ApplyOutcome::Applied) => report.written.push(path.to_string()),
            Ok(ApplyOutcome::ChangedSincePlan) => {
                report.blocked.push(format!("{path}: changed during apply"))
            }
            Err(error) => report.blocked.push(format!("{path}: {error}")),
        }
    }
    report.message = if !report.blocked.is_empty() {
        "Some settings still need review"
    } else if report.written.is_empty() {
        "Settings are already up to date"
    } else {
        "Applied the approved local settings"
    }
    .into();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use std::{fs, sync::Arc};
    use tokio::sync::RwLock;
    use tower::ServiceExt as _;
    use wenlan_types::requests::{CALLER_HEADER, PERSON_CALLER_VALUE, PERSON_KEY_HEADER};

    fn rendered(source_digest: &str) -> RenderedSettings {
        RenderedSettings {
            source_digest: source_digest.into(),
            files: [
                (
                    ".claude.json".into(),
                    br#"{"mcpServers":{"new":{"command":"new"}}}"#.to_vec(),
                ),
                (
                    ".codex/config.toml".into(),
                    b"[mcp_servers.new]\ncommand = 'new'\n".to_vec(),
                ),
            ]
            .into(),
        }
    }

    #[tokio::test]
    async fn rulesync_settings_routes_require_the_local_person_key_and_no_agent() {
        let state = Arc::new(RwLock::new(crate::state::ServerState::new()));
        for (path, body) in [
            (
                "/api/rules/setup/settings/preview",
                r#"{"source":"/does-not-exist"}"#,
            ),
            (
                "/api/rules/setup/settings/apply",
                r#"{"source":"/does-not-exist","token":"fixture"}"#,
            ),
        ] {
            for agent in [None, Some("codex")] {
                let mut request = Request::post(path)
                    .header("content-type", "application/json")
                    .header(CALLER_HEADER, PERSON_CALLER_VALUE);
                if let Some(agent) = agent {
                    request = request
                        .header("x-agent-name", agent)
                        .header(PERSON_KEY_HEADER, crate::person_key::test_key());
                }
                let response = crate::router::build_router(state.clone())
                    .oneshot(request.body(Body::from(body)).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let data = to_bytes(response.into_body(), 1024).await.unwrap();
                let error: serde_json::Value = serde_json::from_slice(&data).unwrap();
                assert!(error["error"].as_str().unwrap().contains("only the person"));
            }
        }
    }

    #[test]
    fn rulesync_settings_rejects_changed_source_and_destinations_before_any_writes() {
        let temp = tempfile::tempdir().unwrap();
        let root = FileRoot::open(temp.path()).unwrap();
        let home = temp.path();
        let source = home.join(".rulesync");
        let first = plan_rendered(&root, home, &source, rendered("source")).unwrap();
        assert_eq!(first.response.files.len(), 2);
        assert!(!home.join(".claude.json").exists());
        assert!(!home.join(".codex").exists());

        let token = first.response.token.clone();
        let different = plan_rendered(&root, home, &source, rendered("changed")).unwrap();
        assert!(apply_plan(&root, different, &token).is_err());
        assert!(!home.join(".claude.json").exists());

        fs::create_dir(home.join(".codex")).unwrap();
        let concurrent = "# written while the preview was open\nmodel = 'other'\n";
        fs::write(home.join(".codex/config.toml"), concurrent).unwrap();
        // Exercise both a freshly recomputed plan and a change after planning.
        let updated = plan_rendered(&root, home, &source, rendered("source")).unwrap();
        assert!(apply_plan(&root, updated, &token).is_err());
        assert!(apply_plan(&root, first, &token).is_err());
        assert!(!home.join(".claude.json").exists());
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            concurrent
        );
    }

    #[test]
    fn rulesync_settings_applies_a_fresh_redacted_plan_and_retries_as_noop() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let source = home.join(".rulesync");
        let existing =
            br#"{"sessionToken":"fixture-live-value","mcpServers":{"local":{"command":"local"}}}"#;
        fs::write(home.join(".claude.json"), existing).unwrap();
        let root = FileRoot::open(home).unwrap();
        let plan = plan_rendered(&root, home, &source, rendered("source")).unwrap();
        let token = plan.response.token.clone();
        assert!(!serde_json::to_string(&plan.response)
            .unwrap()
            .contains("fixture-live-value"));

        let same = plan_rendered(&root, home, &source, rendered("source")).unwrap();
        let report = apply_plan(&root, same, &token).unwrap();
        assert_eq!(report.written.len(), 2);
        assert!(report.blocked.is_empty());
        let bytes = fs::read(home.join(".claude.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["sessionToken"], "fixture-live-value");
        assert_eq!(json["mcpServers"]["local"]["command"], "local");
        assert_eq!(json["mcpServers"]["new"]["command"], "new");

        let retry = plan_rendered(&root, home, &source, rendered("source")).unwrap();
        assert!(retry.response.files.is_empty());
        let fresh = retry.response.token.clone();
        assert!(apply_plan(&root, retry, &fresh).unwrap().written.is_empty());
        assert_eq!(fs::read(home.join(".claude.json")).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_settings_never_follows_a_destination_link() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("config.json");
        fs::write(&target, "{}").unwrap();
        symlink(&target, home.join(".claude.json")).unwrap();
        let root = FileRoot::open(home).unwrap();
        assert!(plan_rendered(&root, home, home, rendered("source")).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"{}");
    }
}
