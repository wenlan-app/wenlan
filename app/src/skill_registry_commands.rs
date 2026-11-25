// SPDX-License-Identifier: AGPL-3.0-only
//! Tauri commands for finding and installing other people's skills. The
//! logic lives in `wenlan_core::skill_registry`, and each action that the
//! daemon also offers runs the same `skill_registry::actions` step the
//! daemon's route runs, so the desktop app refuses exactly the skills the
//! browser and the terminal refuse (UX-SKILL-12, UX-SKILL-13). This file
//! only picks the AI tool folders on this computer.

use std::path::PathBuf;

use wenlan_core::skill_registry::{
    self as registry, actions, Endpoints, InstalledSkill, LocalSkill, ReqwestFetch, Scope,
    UpdateOutcome,
};
use wenlan_types::skill_registry::{
    InstalledSkillRow, SkillImportResult, SkillInstallRequest, SkillInstallResult, SkillPreview,
    SkillSearchResponse,
};

fn tool_skill_dirs() -> Vec<PathBuf> {
    let (home, config_dir) = crate::identity_paths::ai_tool_dirs();
    actions::tool_skill_dirs(home.as_deref(), config_dir.as_deref())
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

/// Search, then look up each result's safety checks. A result whose checks
/// cannot be read counts as "warn"; a failing one is left out. The same
/// steps the daemon's route runs.
#[tauri::command]
pub async fn skills_search(query: String) -> Result<SkillSearchResponse, String> {
    actions::search_checked(&http()?, &Endpoints::default(), &query)
        .await
        .map_err(|e| e.to_string())
}

/// Everything in one skill before it is installed, as the daemon's route
/// gives it: each file marked when it is cut or holds characters that would
/// not show, and `cannotInstall` when Wenlan will refuse the skill.
#[tauri::command]
pub async fn skills_preview(source: String, skill_id: String) -> Result<SkillPreview, String> {
    let (mut preview, folder) =
        actions::preview_from_network(&http()?, &Endpoints::default(), &source, &skill_id)
            .await
            .map_err(|e| e.to_string())?;
    preview.installed_as = tokio::task::spawn_blocking(move || {
        actions::installed_as(&store(), folder, &source, &skill_id)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(preview)
}

fn deliver_result(skill: InstalledSkill) -> Result<SkillInstallResult, String> {
    actions::deliver_result(&store(), skill, &tool_skill_dirs()).map_err(|e| e.to_string())
}

/// Install exactly the commit the person looked at. Safety is checked again;
/// a skill that now fails, or that holds text the preview could not show as
/// it is, is refused and nothing is written (`actions::store_and_deliver`,
/// the step the daemon's route runs).
#[tauri::command]
pub async fn skills_install(
    source: String,
    skill_id: String,
    commit: String,
    repo_path: String,
    scope: Scope,
) -> Result<SkillInstallResult, String> {
    let request = SkillInstallRequest {
        source,
        skill_id,
        commit,
        repo_path,
        scope,
    };
    let (fetched, verdict) = actions::fetch_for_install(&http()?, &Endpoints::default(), &request)
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        actions::store_and_deliver(
            &store(),
            &tool_skill_dirs(),
            &fetched,
            verdict,
            &request,
            now(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn skills_installed() -> Result<Vec<InstalledSkillRow>, String> {
    tokio::task::spawn_blocking(|| actions::installed_rows(&store()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn skills_set_additions(
    name: String,
    text: String,
) -> Result<SkillInstallResult, String> {
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
pub async fn skills_set_scope(name: String, scope: Scope) -> Result<SkillInstallResult, String> {
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

/// Take in the named skills found by `skills_scan_local`. Only folders the
/// scan finds again are read, so the UI cannot point this at any other
/// path. A folder holding text that could not be shown as it is is refused
/// (`actions::import_named`, the step the daemon's route runs). Originals
/// are never changed; Wenlan's copy then reaches the other AI tools.
#[tauri::command]
pub async fn skills_import_local(names: Vec<String>) -> Result<SkillImportResult, String> {
    tokio::task::spawn_blocking(move || {
        actions::import_named(&store(), &tool_skill_dirs(), names, now())
    })
    .await
    .map_err(|e| e.to_string())
}
