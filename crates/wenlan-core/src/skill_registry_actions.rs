// SPDX-License-Identifier: Apache-2.0
//! Each skill action from start to finish, as the daemon's routes run it,
//! so the terminal and AI tools get one set of checks and answers. The
//! desktop app's own commands (`app/src/skill_registry_commands.rs`) do not
//! call these yet: they carry their own copy of the same steps.

use std::path::{Path, PathBuf};

use wenlan_types::skill_registry::{
    InstalledSkillRow, SkillImportResult, SkillInstallRequest, SkillInstallResult, SkillPreview,
    SkillPreviewFile, SkillSearchResponse, SkillSearchRow,
};

use super::{
    deliver, fetch_skill, folder_name, front_matter_field, head_commit, hidden_from_the_person,
    import_local_fully_shown, list_installed, preview_files, read_additions, read_record,
    runnable_files, safety, scan_local, search, store_skill, Endpoints, Fetch, FetchedSkill,
    InstalledSkill, RegistryError, Verdict,
};

/// The skills folders of the AI tools installed on this computer. A tool
/// counts as installed when its own folder exists (`~/.claude`, `~/.cursor`,
/// ...). Codex reads the shared `~/.agents/skills`.
pub fn tool_skill_dirs(home: Option<&Path>, config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !out.contains(&dir) {
            out.push(dir);
        }
    };
    if let Some(home) = home {
        if home.join(".claude").is_dir() {
            push(home.join(".claude").join("skills"));
        }
        if home.join(".codex").is_dir() || home.join(".agents").join("skills").is_dir() {
            push(home.join(".agents").join("skills"));
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
        if let Some(Ok(dir)) = crate::skill_locations::skills_dir_for(client, home, config_dir) {
            if dir.parent().is_some_and(|tool| tool.is_dir()) {
                push(dir);
            }
        }
    }
    out
}

/// Search, then look up each result's safety checks. A result whose checks
/// cannot be read counts as "warn"; a failing one is left out.
pub async fn search_checked(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    query: &str,
) -> Result<SkillSearchResponse, RegistryError> {
    let (hits, via) = search(http, endpoints, query).await?;
    let checks = futures::future::join_all(hits.iter().map(|hit| async move {
        safety(http, endpoints, &hit.source, &hit.skill_id)
            .await
            .map(|report| report.verdict)
            .unwrap_or(Verdict::Warn)
    }))
    .await;
    let rows = hits
        .into_iter()
        .zip(checks)
        .filter(|(_, verdict)| *verdict != Verdict::Fail)
        .map(|(hit, verdict)| SkillSearchRow { hit, verdict })
        .collect();
    Ok(SkillSearchResponse { via, rows })
}

/// Everything a person looks at before installing: the files at the
/// repository's newest commit and the safety checks. A failing skill is
/// refused here, before any file is fetched.
pub async fn preview(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    root: &Path,
    source: &str,
    skill_id: &str,
) -> Result<SkillPreview, RegistryError> {
    let (mut preview, folder) = preview_from_network(http, endpoints, source, skill_id).await?;
    preview.installed_as = installed_as(root, folder, source, skill_id);
    Ok(preview)
}

/// The folder name this skill is already held under, if it is.
pub fn installed_as(
    root: &Path,
    folder: Option<String>,
    source: &str,
    skill_id: &str,
) -> Option<String> {
    folder.filter(|name| {
        read_record(root, name).is_some_and(|r| r.source == source && r.skill_id == skill_id)
    })
}

/// The network half of a preview, with `installed_as` left empty, and the
/// folder name an install would use (for `installed_as`, which reads disk).
pub async fn preview_from_network(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    source: &str,
    skill_id: &str,
) -> Result<(SkillPreview, Option<String>), RegistryError> {
    let report = safety(http, endpoints, source, skill_id).await?;
    if report.verdict == Verdict::Fail {
        return Err(RegistryError::FailedSafetyCheck);
    }
    let commit = head_commit(http, endpoints, source).await?;
    let fetched = fetch_skill(http, endpoints, source, skill_id, &commit, None).await?;
    let skill_md = fetched
        .files
        .iter()
        .find(|f| f.path == "SKILL.md")
        .map(|f| String::from_utf8_lossy(&f.bytes).to_string())
        .unwrap_or_default();
    let mut files: Vec<SkillPreviewFile> = preview_files(&fetched.files)
        .into_iter()
        .map(|file| SkillPreviewFile {
            path: file.path,
            text: file.text,
            cut: file.cut,
        })
        .collect();
    // SKILL.md first, the rest in path order.
    files.sort_by_key(|f| (f.path != "SKILL.md", f.path.clone()));
    let tree_path = if fetched.repo_path.is_empty() {
        String::new()
    } else {
        format!("/{}", fetched.repo_path)
    };
    let preview = SkillPreview {
        source_url: format!("https://github.com/{source}/tree/{commit}{tree_path}"),
        skills_sh_url: format!("https://skills.sh/{source}/{skill_id}"),
        name: front_matter_field(&skill_md, "name"),
        description: front_matter_field(&skill_md, "description"),
        runnable: runnable_files(&fetched.files),
        cannot_install: hidden_from_the_person(&fetched.files),
        files,
        safety: report,
        installed_as: None,
        source: source.to_string(),
        skill_id: skill_id.to_string(),
        commit,
        repo_path: fetched.repo_path,
    };
    Ok((preview, folder_name(&skill_md, skill_id)))
}

/// The network half of an install: the safety checks again, then exactly
/// the commit and folder the person looked at.
pub async fn fetch_for_install(
    http: &dyn Fetch,
    endpoints: &Endpoints,
    request: &SkillInstallRequest,
) -> Result<(FetchedSkill, Verdict), RegistryError> {
    let report = safety(http, endpoints, &request.source, &request.skill_id).await?;
    let fetched = fetch_skill(
        http,
        endpoints,
        &request.source,
        &request.skill_id,
        &request.commit,
        Some(&request.repo_path),
    )
    .await?;
    Ok((fetched, report.verdict))
}

/// Copy a held skill into the AI tools' folders and say where it went.
pub fn deliver_result(
    root: &Path,
    skill: InstalledSkill,
    tool_dirs: &[PathBuf],
) -> Result<SkillInstallResult, RegistryError> {
    let (tools, skipped) = deliver(root, &skill.name, tool_dirs)?;
    let show = |paths: Vec<PathBuf>| paths.into_iter().map(|p| p.display().to_string()).collect();
    Ok(SkillInstallResult {
        skill,
        tools: show(tools),
        skipped: show(skipped),
    })
}

/// The disk half of an install. A skill that failed its checks, or that
/// holds text a preview could not show in full, is refused and nothing is
/// written.
pub fn store_and_deliver(
    root: &Path,
    tool_dirs: &[PathBuf],
    fetched: &FetchedSkill,
    verdict: Verdict,
    request: &SkillInstallRequest,
    now: i64,
) -> Result<SkillInstallResult, RegistryError> {
    if let Some(reason) = hidden_from_the_person(&fetched.files) {
        return Err(RegistryError::Refused(reason));
    }
    let skill = store_skill(root, fetched, verdict, request.scope.clone(), now)?;
    deliver_result(root, skill, tool_dirs)
}

/// Every skill Wenlan holds, with the person's additions.
pub fn installed_rows(root: &Path) -> Vec<InstalledSkillRow> {
    list_installed(root)
        .into_iter()
        .map(|skill| InstalledSkillRow {
            additions: read_additions(root, &skill.name),
            skill,
        })
        .collect()
}

/// Take in the named skills a scan of the tool folders finds. Only folders
/// the scan finds again are read, so a caller cannot point this at any other
/// path. Originals are never changed; Wenlan's copy then reaches the other
/// AI tools.
pub fn import_named(
    root: &Path,
    tool_dirs: &[PathBuf],
    names: Vec<String>,
    now: i64,
) -> SkillImportResult {
    let found = scan_local(root, tool_dirs);
    let mut out = SkillImportResult {
        imported: Vec::new(),
        failed: Vec::new(),
    };
    for name in names {
        let Some(local) = found.iter().find(|s| s.name == name) else {
            out.failed.push((name, "not found on this computer".into()));
            continue;
        };
        match import_local_fully_shown(root, Path::new(&local.path), now) {
            Ok(skill) => {
                if let Err(error) = deliver(root, &skill.name, tool_dirs) {
                    log::warn!("[skills] took in {name} but could not copy it: {error}");
                }
                out.imported.push(name);
            }
            Err(error) => out.failed.push((name, error.to_string())),
        }
    }
    out
}
