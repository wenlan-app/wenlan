// SPDX-License-Identifier: Apache-2.0
//! The daily run of a Space bound to a repository: once a day, after the
//! hour the user picked, start the user's own AI tool in the background to
//! bring the Space's pages up to date (`steep`) and write the pages still
//! missing (`wiki`), then publish them to the repository as a pull request.
//!
//! Off until the user turns it on per Space (`wenlan wiki bind --daily`); it
//! spends that tool's own quota. A day's run is marked started before it
//! begins, so a failing run is not retried until the next day.

use crate::state::SharedState;
use std::path::PathBuf;
use std::time::Duration;
use wenlan_core::repo_wiki;
use wenlan_types::repo_wiki::RepoWikiBinding;

/// How often the bindings are looked at.
const TICK: Duration = Duration::from_secs(10 * 60);
/// Longest one AI tool step may take before it is stopped.
const STEP_TIMEOUT: Duration = Duration::from_secs(45 * 60);

/// Start the daily-run loop. It keeps its own clock (an hour of the user's
/// choosing), apart from the scheduler's ambient poll.
pub fn start(shared: SharedState, shutdown: tokio::sync::watch::Receiver<bool>) {
    tokio::spawn(run(shared, shutdown));
}

/// Run until shutdown, starting each bound Space's daily run when it is due.
async fn run(shared: SharedState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            return;
        }
        let now = chrono::Local::now();
        let bindings = match tokio::task::spawn_blocking(|| {
            repo_wiki::load_bindings_at(&repo_wiki::store_path())
        })
        .await
        {
            Ok(Ok(bindings)) => bindings,
            Ok(Err(error)) => {
                tracing::warn!("[repo_wiki] could not read bindings: {error}");
                continue;
            }
            Err(error) => {
                tracing::warn!("[repo_wiki] bindings task failed: {error}");
                continue;
            }
        };
        for binding in bindings
            .into_iter()
            .filter(|binding| repo_wiki::daily_due(binding, now))
        {
            if let Err(error) = run_once(&shared, &binding).await {
                tracing::warn!("[repo_wiki] daily run of {} failed: {error}", binding.space);
            }
        }
    }
}

/// Spaces whose run is under way, so a second start waits for the next one.
static RUNNING: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Marks a Space's run as under way until dropped.
struct RunGuard(String);

impl RunGuard {
    fn claim(space: &str) -> Option<Self> {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        running
            .insert(space.to_owned())
            .then(|| Self(space.to_owned()))
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Whether a run of `space` is under way.
pub(crate) fn is_running(space: &str) -> bool {
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(space)
}

/// One daily run of `binding`, now. Also used by `POST .../wiki/run`.
pub(crate) async fn run_once(
    shared: &SharedState,
    binding: &RepoWikiBinding,
) -> Result<wenlan_types::repo_wiki::RepoWikiPublishResult, String> {
    let space = binding.space.clone();
    let Some(_guard) = RunGuard::claim(&space) else {
        return Err(format!("a run of {space} is already under way"));
    };
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    {
        let space = space.clone();
        let date = date.clone();
        tokio::task::spawn_blocking(move || {
            repo_wiki::mark_daily_started_at(&repo_wiki::store_path(), &space, &date)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    }
    let agent = binding
        .daily
        .as_ref()
        .map(|daily| daily.agent)
        .unwrap_or(wenlan_types::repo_wiki::RepoWikiAgent::Claude);
    match repo_wiki::find_agent(agent) {
        Some(program) => {
            for verb in ["steep", "wiki"] {
                run_step(&program, agent, binding, verb, &date).await;
            }
        }
        None => tracing::warn!(
            "[repo_wiki] daily run of {space}: {agent:?} is not installed; publishing what is there"
        ),
    }
    let db = {
        let state = shared.read().await;
        state.db.clone()
    }
    .ok_or_else(|| "the database is not ready".to_string())?;
    crate::repo_wiki_routes::publish_space(&db, &space)
        .await
        .map_err(|e| format!("{e:?}"))
}

/// Run one AI tool step in the repository, with its output kept in a log
/// file under the data root.
async fn run_step(
    program: &PathBuf,
    agent: wenlan_types::repo_wiki::RepoWikiAgent,
    binding: &RepoWikiBinding,
    verb: &str,
    date: &str,
) {
    let logs = repo_wiki::worktrees_root().join("logs");
    let log_path = logs.join(format!(
        "{}-{date}-{verb}.log",
        repo_wiki::worktree_for(&binding.space)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "space".into())
    ));
    let mut command = tokio::process::Command::new(program);
    command
        .args(repo_wiki::daily_agent_args(agent, &binding.space, verb))
        .current_dir(&binding.repo_root)
        // The daemon is already running; the tool's Wenlan connection is
        // locked to this Space and named for the run.
        .env("WENLAN_NO_AUTOSTART", "1")
        .env("WENLAN_SPACE", &binding.space)
        .env("WENLAN_AGENT_NAME", "wenlan-daily")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let started = std::time::Instant::now();
    tracing::info!(
        "[repo_wiki] daily {verb} for {} with {}",
        binding.space,
        program.display()
    );
    let output = match tokio::time::timeout(STEP_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            tracing::warn!("[repo_wiki] could not start {}: {error}", program.display());
            return;
        }
        Err(_) => {
            tracing::warn!(
                "[repo_wiki] daily {verb} for {} stopped after {} minutes",
                binding.space,
                STEP_TIMEOUT.as_secs() / 60
            );
            return;
        }
    };
    let mut log = format!(
        "exit: {}\nseconds: {}\n\n--- stdout ---\n",
        output.status,
        started.elapsed().as_secs()
    );
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    log.push_str("\n--- stderr ---\n");
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    if let Err(error) = tokio::fs::create_dir_all(&logs).await {
        tracing::warn!("[repo_wiki] could not create {}: {error}", logs.display());
    } else if let Err(error) = tokio::fs::write(&log_path, log).await {
        tracing::warn!(
            "[repo_wiki] could not write {}: {error}",
            log_path.display()
        );
    }
    tracing::info!(
        "[repo_wiki] daily {verb} for {} finished ({}) in {}s; log {}",
        binding.space,
        output.status,
        started.elapsed().as_secs(),
        log_path.display()
    );
}
