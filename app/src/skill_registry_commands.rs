// SPDX-License-Identifier: AGPL-3.0-only
//! Tauri commands for finding and installing other people's skills. The
//! logic lives in `wenlan_core::skill_registry`; this file picks the AI tool
//! folders on this computer and turns results into plain shapes for the UI.

use std::path::PathBuf;

use serde::Serialize;
use wenlan_core::skill_registry::{
    self as registry, Endpoints, InstalledSkill, LocalSkill, ReqwestFetch, SafetyReport, Scope,
    SearchVia, SkillHit, UpdateOutcome, Verdict,
};

/// The skills folders of the AI tools installed on this computer. A tool
/// counts as installed when its own folder exists (`~/.claude`, `~/.cursor`,
/// ...). Codex reads the shared `~/.agents/skills`.
fn tool_skill_dirs() -> Vec<PathBuf> {
    let (home, config_dir) = crate::identity_paths::ai_tool_dirs();
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !out.contains(&dir) {
            out.push(dir);
        }
    };
    if let Some(home) = home.as_deref() {
        if home.join(".claude").is_dir() {
            push(home.join(".claude/skills"));
        }
        if home.join(".codex").is_dir() || home.join(".agents/skills").is_dir() {
            push(home.join(".agents/skills"));
        }
    }
    for client in [
        "cursor",
        "gemini_cli",
        "copilot_cli",
        "windsurf",
        "kiro",
        "opencode",
        "kimi_code",
    ] {
        if let Some(Ok(dir)) =
            crate::skills_install::skills_dir_for(client, home.as_deref(), config_dir.as_deref())
        {
            if dir.parent().is_some_and(|tool| tool.is_dir()) {
                push(dir);
            }
        }
    }
    out
}

fn store() -> PathBuf {
    registry::default_wenlan_root()
}

fn http() -> Result<ReqwestFetch, String> {
    ReqwestFetch::new()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRow {
    #[serde(flatten)]
    pub hit: SkillHit,
    pub verdict: Verdict,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub via: SearchVia,
    /// Failing skills are never included.
    pub rows: Vec<SearchRow>,
}

/// Search, then look up each result's safety checks. A result whose checks
/// cannot be read counts as "warn".
#[tauri::command]
pub async fn skills_search(query: String) -> Result<SearchResponse, String> {
    let http = http()?;
    let endpoints = Endpoints::default();
    let (hits, via) = registry::search(&http, &endpoints, &query)
        .await
        .map_err(|e| e.to_string())?;
    let checks = futures_util::future::join_all(hits.iter().map(|hit| {
        let (http, endpoints) = (&http, &endpoints);
        async move {
            registry::safety(http, endpoints, &hit.source, &hit.skill_id)
                .await
                .map(|r| r.verdict)
                .unwrap_or(Verdict::Warn)
        }
    }))
    .await;
    let rows = hits
        .into_iter()
        .zip(checks)
        .filter(|(_, verdict)| *verdict != Verdict::Fail)
        .map(|(hit, verdict)| SearchRow { hit, verdict })
        .collect();
    Ok(SearchResponse { via, rows })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewFile {
    pub path: String,
    /// `None` for a file that is not text.
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreview {
    pub source: String,
    pub skill_id: String,
    pub commit: String,
    pub repo_path: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub files: Vec<PreviewFile>,
    /// Files that could run on this computer. Wenlan never runs them.
    pub runnable: Vec<String>,
    pub safety: SafetyReport,
    pub source_url: String,
    pub skills_sh_url: String,
    /// Already installed under this folder name.
    pub installed_as: Option<String>,
}

#[tauri::command]
pub async fn skills_preview(source: String, skill_id: String) -> Result<SkillPreview, String> {
    let http = http()?;
    let endpoints = Endpoints::default();
    let safety = registry::safety(&http, &endpoints, &source, &skill_id)
        .await
        .map_err(|e| e.to_string())?;
    if safety.verdict == Verdict::Fail {
        return Err(registry::RegistryError::FailedSafetyCheck.to_string());
    }
    let commit = registry::head_commit(&http, &endpoints, &source)
        .await
        .map_err(|e| e.to_string())?;
    let fetched = registry::fetch_skill(&http, &endpoints, &source, &skill_id, &commit, None)
        .await
        .map_err(|e| e.to_string())?;
    let skill_md = fetched
        .files
        .iter()
        .find(|f| f.path == "SKILL.md")
        .map(|f| String::from_utf8_lossy(&f.bytes).to_string())
        .unwrap_or_default();
    let mut files: Vec<PreviewFile> = registry::preview_text(&fetched.files)
        .into_iter()
        .map(|(path, text)| PreviewFile { path, text })
        .collect();
    // SKILL.md first, the rest in path order.
    files.sort_by_key(|f| (f.path != "SKILL.md", f.path.clone()));
    let folder = registry::folder_name(&skill_md, &skill_id);
    let installed_as = folder.filter(|name| {
        registry::read_record(&store(), name)
            .is_some_and(|r| r.source == source && r.skill_id == skill_id)
    });
    let tree_path = if fetched.repo_path.is_empty() {
        String::new()
    } else {
        format!("/{}", fetched.repo_path)
    };
    Ok(SkillPreview {
        source_url: format!("https://github.com/{source}/tree/{commit}{tree_path}"),
        skills_sh_url: format!("https://skills.sh/{source}/{skill_id}"),
        name: registry::front_matter_field(&skill_md, "name"),
        description: registry::front_matter_field(&skill_md, "description"),
        runnable: registry::runnable_files(&fetched.files),
        files,
        safety,
        installed_as,
        source,
        skill_id,
        commit,
        repo_path: fetched.repo_path,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub skill: InstalledSkill,
    pub tools: Vec<String>,
    /// Same-named folders that belong to the person, left alone.
    pub skipped: Vec<String>,
}

fn deliver_result(skill: InstalledSkill) -> Result<InstallResult, String> {
    let (tools, skipped) =
        registry::deliver(&store(), &skill.name, &tool_skill_dirs()).map_err(|e| e.to_string())?;
    let show = |paths: Vec<PathBuf>| paths.into_iter().map(|p| p.display().to_string()).collect();
    Ok(InstallResult {
        skill,
        tools: show(tools),
        skipped: show(skipped),
    })
}

/// Install exactly the commit the person looked at. Safety is checked again;
/// a skill that now fails is refused.
#[tauri::command]
pub async fn skills_install(
    source: String,
    skill_id: String,
    commit: String,
    repo_path: String,
    scope: Scope,
) -> Result<InstallResult, String> {
    let http = http()?;
    let endpoints = Endpoints::default();
    let safety = registry::safety(&http, &endpoints, &source, &skill_id)
        .await
        .map_err(|e| e.to_string())?;
    let fetched = registry::fetch_skill(
        &http,
        &endpoints,
        &source,
        &skill_id,
        &commit,
        Some(&repo_path),
    )
    .await
    .map_err(|e| e.to_string())?;
    let skill = tokio::task::spawn_blocking(move || {
        registry::store_skill(&store(), &fetched, safety.verdict, scope, now())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || deliver_result(skill))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRow {
    #[serde(flatten)]
    pub skill: InstalledSkill,
    pub additions: String,
}

#[tauri::command]
pub async fn skills_installed() -> Result<Vec<InstalledRow>, String> {
    tokio::task::spawn_blocking(|| {
        let store = store();
        registry::list_installed(&store)
            .into_iter()
            .map(|skill| InstalledRow {
                additions: registry::read_additions(&store, &skill.name),
                skill,
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn skills_set_additions(name: String, text: String) -> Result<InstallResult, String> {
    tokio::task::spawn_blocking(move || {
        let store = store();
        registry::set_additions(&store, &name, &text).map_err(|e| e.to_string())?;
        let skill = registry::read_record(&store, &name).ok_or("skill not found")?;
        deliver_result(skill)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn skills_set_scope(name: String, scope: Scope) -> Result<InstallResult, String> {
    tokio::task::spawn_blocking(move || {
        let skill = registry::set_scope(&store(), &name, scope).map_err(|e| e.to_string())?;
        deliver_result(skill)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn skills_remove(name: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        registry::remove(&store(), &name, &tool_skill_dirs()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Check every installed skill for an author update. Ones that pass every
/// check are updated and re-delivered; the rest are reported.
#[tauri::command]
pub async fn skills_refresh() -> Result<Vec<UpdateOutcome>, String> {
    let http = http()?;
    let endpoints = Endpoints::default();
    let store = store();
    let mut out = Vec::new();
    for skill in registry::list_installed(&store) {
        let outcome =
            match registry::refresh_one(&http, &endpoints, &store, &skill.name, now()).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    log::warn!(
                        "[skills] could not check {} for updates: {error}",
                        skill.name
                    );
                    continue;
                }
            };
        if outcome.outcome == "updated" {
            if let Some(updated) = registry::read_record(&store, &skill.name) {
                if let Err(error) = deliver_result(updated) {
                    log::warn!(
                        "[skills] updated {} but could not copy it: {error}",
                        skill.name
                    );
                }
            }
        }
        out.push(outcome);
    }
    Ok(out)
}

/// Skills already in this computer's AI tool folders that Wenlan does not
/// hold yet.
#[tauri::command]
pub async fn skills_scan_local() -> Result<Vec<LocalSkill>, String> {
    tokio::task::spawn_blocking(|| registry::scan_local(&store(), &tool_skill_dirs()))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub imported: Vec<String>,
    /// Name and reason for each skill that could not be taken in.
    pub failed: Vec<(String, String)>,
}

/// Take in the named skills found by `skills_scan_local`. Only folders the
/// scan finds again are read, so the UI cannot point this at any other
/// path. Originals are never changed; Wenlan's copy then reaches the other
/// AI tools.
#[tauri::command]
pub async fn skills_import_local(names: Vec<String>) -> Result<ImportResult, String> {
    tokio::task::spawn_blocking(move || {
        let store = store();
        let tools = tool_skill_dirs();
        let found = registry::scan_local(&store, &tools);
        let mut out = ImportResult {
            imported: Vec::new(),
            failed: Vec::new(),
        };
        for name in names {
            let Some(local) = found.iter().find(|s| s.name == name) else {
                out.failed.push((name, "not found on this computer".into()));
                continue;
            };
            match registry::import_local(&store, std::path::Path::new(&local.path), now()) {
                Ok(skill) => {
                    if let Err(error) = registry::deliver(&store, &skill.name, &tools) {
                        log::warn!("[skills] took in {name} but could not copy it: {error}");
                    }
                    out.imported.push(name);
                }
                Err(error) => out.failed.push((name, error.to_string())),
            }
        }
        out
    })
    .await
    .map_err(|e| e.to_string())
}
