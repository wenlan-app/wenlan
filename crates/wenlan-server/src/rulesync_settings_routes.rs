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
    rulesync_apply::{FileRoot, Snapshot},
    rulesync_routes::{require_local_person, Setup, SETTINGS_RESUME_NOTICE},
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
    execute(
        headers,
        Some(request.source),
        |_, _, plan| Ok(plan.response),
    )
    .await
}

async fn apply(
    headers: HeaderMap,
    Json(request): Json<SettingsApplyRequest>,
) -> Result<Json<RulesSyncReport>, ServerError> {
    execute(headers, request.source, move |setup, root, plan| {
        apply_plan(setup, root, plan, &request.token)
    })
    .await
}

async fn execute<T: Send + 'static>(
    headers: HeaderMap,
    source: Option<PathBuf>,
    finish: impl FnOnce(&Setup, &FileRoot, LocalPlan) -> Result<T> + Send + 'static,
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
        let root = setup.home_files().context("open local AI-tool home")?;
        let plan = if let Some(source) = source {
            let rendered = rulesync_settings::render_source(&setup.binary, &setup.data, &source)?;
            plan_rendered(&setup, &root, &source, rendered)?
        } else {
            plan_local(&setup, &root)?.context("No settings changes to review")?
        };
        finish(&setup, &root, plan)
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

pub(crate) struct LocalPlan {
    pub(crate) response: SettingsPlan,
    source_digest: String,
    writes: Vec<PlannedWrite>,
}

pub(crate) fn plan_local(setup: &Setup, root: &FileRoot) -> Result<Option<LocalPlan>> {
    if !setup.data.try_exists()? {
        return Ok(None);
    }
    setup.data_files()?;
    let Some(rendered) = rulesync_settings::render_local_source(&setup.binary, &setup.data)? else {
        return Ok(None);
    };
    let plan = plan_rendered(setup, root, &setup.data, rendered)?;
    Ok((!plan.response.files.is_empty() || plan.response.notice.is_some()).then_some(plan))
}

pub(crate) fn has_settings_review(setup: &Setup, root: &FileRoot) -> Result<bool> {
    Ok(plan_local(setup, root)?.is_some())
}

fn plan_rendered(
    setup: &Setup,
    root: &FileRoot,
    source: &Path,
    mut rendered: RenderedSettings,
) -> Result<LocalPlan> {
    let remaining = setup.settings_remaining(&rendered.source_digest)?;
    if let Some(remaining) = &remaining {
        ensure!(
            remaining.iter().all(|path| rendered.files.contains_key(path)),
            "The renderer no longer produces an unfinished settings file. Run `wenlan rules restore` to undo applied changes."
        );
        rendered.files.retain(|path, _| remaining.contains(path));
    }
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
        if let Some(mut change) = change {
            change.preview.tool = crate::rulesync_routes::tool_for_path(&path)
                .map(crate::rulesync_routes::tool_label);
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
        &setup.home,
        source,
        &rendered.source_digest,
        &remaining,
        &writes,
    ))?;
    Ok(LocalPlan {
        response: SettingsPlan {
            token,
            files,
            notice: remaining.map(|_| SETTINGS_RESUME_NOTICE.into()),
        },
        source_digest: rendered.source_digest,
        writes,
    })
}

pub(crate) fn apply_plan(
    setup: &Setup,
    root: &FileRoot,
    plan: LocalPlan,
    token: &str,
) -> Result<RulesSyncReport> {
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
    setup.apply_settings(
        root,
        plan.source_digest,
        plan.writes
            .into_iter()
            .map(|write| (write.path, write.seen, write.desired))
            .collect(),
    )
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

    fn test_setup(base: &Path) -> Setup {
        let setup = Setup {
            home: base.join("home"),
            data: base.join("data"),
            binary: "unused".into(),
        };
        fs::create_dir_all(&setup.home).unwrap();
        setup
    }

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
        let setup = test_setup(temp.path());
        let _lock = setup.lock().unwrap();
        let home = &setup.home;
        let root = FileRoot::open_home(home).unwrap();
        let source = home.join(".rulesync");
        let first = plan_rendered(&setup, &root, &source, rendered("source")).unwrap();
        assert_eq!(first.response.files.len(), 2);
        assert!(!home.join(".claude.json").exists());
        assert!(!home.join(".codex").exists());

        let token = first.response.token.clone();
        let different = plan_rendered(&setup, &root, &source, rendered("changed")).unwrap();
        assert!(apply_plan(&setup, &root, different, &token).is_err());
        assert!(!home.join(".claude.json").exists());

        fs::create_dir(home.join(".codex")).unwrap();
        let concurrent = "# written while the preview was open\nmodel = 'other'\n";
        fs::write(home.join(".codex/config.toml"), concurrent).unwrap();
        // Exercise both a freshly recomputed plan and a change after planning.
        let updated = plan_rendered(&setup, &root, &source, rendered("source")).unwrap();
        assert!(apply_plan(&setup, &root, updated, &token).is_err());
        assert!(apply_plan(&setup, &root, first, &token).is_err());
        assert!(!home.join(".claude.json").exists());
        assert_eq!(
            fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
            concurrent
        );
    }

    #[test]
    fn rulesync_settings_applies_a_fresh_redacted_plan_and_retries_as_noop() {
        let temp = tempfile::tempdir().unwrap();
        let setup = test_setup(temp.path());
        let _lock = setup.lock().unwrap();
        let home = &setup.home;
        let source = home.join(".rulesync");
        let existing =
            br#"{"sessionToken":"fixture-live-value","mcpServers":{"local":{"command":"local"}}}"#;
        fs::write(home.join(".claude.json"), existing).unwrap();
        let root = FileRoot::open_home(home).unwrap();
        let plan = plan_rendered(&setup, &root, &source, rendered("source")).unwrap();
        let token = plan.response.token.clone();
        assert!(!serde_json::to_string(&plan.response)
            .unwrap()
            .contains("fixture-live-value"));

        let same = plan_rendered(&setup, &root, &source, rendered("source")).unwrap();
        let report = apply_plan(&setup, &root, same, &token).unwrap();
        assert_eq!(report.written.len(), 2);
        assert!(report.blocked.is_empty());
        let bytes = fs::read(home.join(".claude.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["sessionToken"], "fixture-live-value");
        assert_eq!(json["mcpServers"]["local"]["command"], "local");
        assert_eq!(json["mcpServers"]["new"]["command"], "new");

        let retry = plan_rendered(&setup, &root, &source, rendered("source")).unwrap();
        assert!(retry.response.files.is_empty());
        let fresh = retry.response.token.clone();
        assert!(apply_plan(&setup, &root, retry, &fresh)
            .unwrap()
            .written
            .is_empty());
        assert_eq!(fs::read(home.join(".claude.json")).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_settings_refuse_a_physical_harness_alias() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let setup = test_setup(temp.path());
        let _lock = setup.lock().unwrap();
        let home = &setup.home;
        // Exercise the mixed-case lookup on case-insensitive volumes too.
        fs::create_dir(home.join(".Codex")).unwrap();
        if !home.join(".codex").exists() {
            fs::rename(home.join(".Codex"), home.join(".codex")).unwrap();
        }
        let physical = fs::canonicalize(home.join(".codex")).unwrap();
        fs::write(physical.join("config.toml"), b"# harness settings\n").unwrap();
        symlink(&physical, home.join(".agents")).unwrap();
        let root = FileRoot::open_home(home).unwrap();
        let result = plan_rendered(&setup, &root, home, rendered("source")).and_then(|plan| {
            let token = plan.response.token.clone();
            apply_plan(&setup, &root, plan, &token)
        });
        assert!(result.is_err() || !result.unwrap().blocked.is_empty());
        assert_eq!(
            fs::read(physical.join("config.toml")).unwrap(),
            b"# harness settings\n"
        );
        assert!(!home.join(".claude.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn run9_crash_settings_subprocess() {
        let Some(base) = std::env::var_os("WENLAN_TEST_SETTINGS_ROOT").map(PathBuf::from) else {
            return;
        };
        let setup = test_setup(&base);
        let _lock = setup.lock().unwrap();
        let root = FileRoot::open_home(&setup.home).unwrap();
        let plan = plan_rendered(&setup, &root, &base, rendered("confirmed")).unwrap();
        let token = plan.response.token.clone();
        apply_plan(&setup, &root, plan, &token).unwrap();
        panic!("the injected exit was not reached");
    }

    #[cfg(unix)]
    fn crash_after_claude_write(base: &Path) {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("rulesync_settings_routes::tests::run9_crash_settings_subprocess")
            .env("WENLAN_TEST_SETTINGS_ROOT", base)
            .env("WENLAN_TEST_FILE_APPLY_CRASH", "displaced:.claude.json")
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(76), "{child:?}");
    }

    #[cfg(unix)]
    #[test]
    fn run9_settings_restore_reverts_the_first_tool_after_a_half_applied_batch() {
        let temp = tempfile::tempdir().unwrap();
        let setup = test_setup(temp.path());
        fs::create_dir_all(setup.home.join(".codex")).unwrap();
        let claude = setup.home.join(".claude.json");
        let codex = setup.home.join(".codex/config.toml");
        let original = br#"{"sessionToken":"example-not-a-token"}"#;
        fs::write(&claude, original).unwrap();
        fs::write(&codex, b"# local configuration\n").unwrap();
        crash_after_claude_write(temp.path());
        assert_ne!(fs::read(&claude).unwrap(), original);
        assert_eq!(fs::read(&codex).unwrap(), b"# local configuration\n");

        let restored = setup.restore().unwrap();
        assert!(restored.blocked.is_empty(), "{restored:?}");
        assert_eq!(fs::read(&claude).unwrap(), original);
        assert_eq!(fs::read(&codex).unwrap(), b"# local configuration\n");
    }

    #[cfg(unix)]
    #[test]
    fn run9_settings_reports_partial_progress_and_resumes_the_same_source() {
        let temp = tempfile::tempdir().unwrap();
        let setup = test_setup(temp.path());
        let claude = setup.home.join(".claude.json");
        let codex = setup.home.join(".codex/config.toml");
        fs::write(&claude, br#"{"sessionToken":"example-not-a-token"}"#).unwrap();
        crash_after_claude_write(temp.path());
        assert!(!codex.exists());
        let applied_claude = fs::read(&claude).unwrap();

        let _lock = setup.lock().unwrap();
        let root = FileRoot::open_home(&setup.home).unwrap();
        let source = temp.path();
        let different = plan_rendered(&setup, &root, source, rendered("changed"));
        assert!(different.is_err());
        let resume = plan_rendered(&setup, &root, source, rendered("confirmed")).unwrap();
        assert_eq!(
            resume.response.notice.as_deref(),
            Some(SETTINGS_RESUME_NOTICE)
        );
        assert_eq!(resume.response.files.len(), 1);
        assert_eq!(resume.response.files[0].path, ".codex/config.toml");
        assert!(!serde_json::to_string(&resume.response)
            .unwrap()
            .contains("example-not-a-token"));
        assert!(
            !codex.exists(),
            "preview must not complete a settings write"
        );

        let token = resume.response.token.clone();
        let report = apply_plan(&setup, &root, resume, &token).unwrap();
        assert_eq!(report.written, [".codex/config.toml"]);
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(codex.exists());
        assert_eq!(fs::read(&claude).unwrap(), applied_claude);
        assert!(setup.settings_remaining("confirmed").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_settings_never_follows_a_destination_link() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let setup = test_setup(temp.path());
        let home = &setup.home;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("config.json");
        fs::write(&target, "{}").unwrap();
        symlink(&target, home.join(".claude.json")).unwrap();
        let root = FileRoot::open_home(home).unwrap();
        assert!(plan_rendered(&setup, &root, home, rendered("source")).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"{}");
    }
}
