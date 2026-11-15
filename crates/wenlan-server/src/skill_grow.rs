// SPDX-License-Identifier: Apache-2.0
//! Nightly and on-demand skills grown from library pages.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::HeaderMap;
use axum::response::Json;
use chrono::{Datelike, TimeZone};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wenlan_core::db::skill_grow::{
    SkillInstallState, SkillPageState, StoredSkillSuggestion, SuggestionStatus,
};
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_core::skill_grow::{self, MergedLines, OldSkillCandidate, SkillPageInput, SpaceLines};
use wenlan_types::authorship::SentenceAuthor;
use wenlan_types::requests::{CreateConceptRequest, UpdatePageRequest};
use wenlan_types::{
    SkillBorrowRecord, SkillLine, SkillReportRequest, SkillSuggestion,
    SkillSuggestionActionResponse, SkillsRefreshResponse, WriteSpaceTarget,
};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::SharedState;

const MANAGED_MARKER: &str = ".wenlan-managed";
const MANAGED_VALUE: &str = "skills-from-library-v1\n";
const GENERAL_ID: &str = "__wenlan_me__";
const MAX_PAGES_PER_SPACE: usize = 80;
const MAX_UNMANAGED_SKILLS: usize = 40;
static SKILLS_MUTATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route("/api/skills/refresh", post(handle_refresh))
        .route("/api/skills/stop", post(handle_stop))
        .route("/api/skills/suggestions", get(handle_suggestions))
        .route(
            "/api/skills/suggestions/{id}/apply",
            post(handle_apply_suggestion),
        )
        .route(
            "/api/skills/suggestions/{id}/dismiss",
            post(handle_dismiss_suggestion),
        )
        .route(
            "/api/skills/suggestions/{id}/undo",
            post(handle_undo_suggestion),
        )
        .route(
            "/api/skills/record",
            get(handle_record).post(handle_self_report),
        )
}

#[derive(Debug, Clone)]
struct SkillDirs {
    home: Option<PathBuf>,
    config: Option<PathBuf>,
}

impl SkillDirs {
    fn system() -> Self {
        Self {
            home: dirs::home_dir(),
            config: dirs::config_dir(),
        }
    }
}

#[derive(Debug)]
struct DesiredSkill {
    name: String,
    content: String,
}

#[derive(Debug)]
struct SourceResult {
    state: SkillPageState,
    lines: Vec<SkillLine>,
    wrote: bool,
    modified_at: i64,
}

struct SourcePageSpec<'a> {
    space_id: &'a str,
    space_name: &'a str,
    skill_slug: &'a str,
    title: &'a str,
    target: WriteSpaceTarget,
    workspace: Option<String>,
    extracted: &'a [SkillLine],
}

fn private_space(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "personal" | "private"
    )
}

fn source_title(space: &str) -> String {
    format!("How I work — {space}")
}

fn page_modified_at(page: &wenlan_types::Page) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&page.last_modified)
        .map(|at| at.timestamp())
        .unwrap_or_default()
}

fn content_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn disambiguate_space_slugs(spaces: &mut [SpaceLines]) {
    let slug_counts: HashMap<String, usize> =
        spaces.iter().fold(HashMap::new(), |mut counts, space| {
            *counts.entry(space.slug.clone()).or_default() += 1;
            counts
        });
    for space in spaces {
        if space.slug == "me" || slug_counts.get(&space.slug).copied().unwrap_or_default() > 1 {
            let stable_suffix: String = space
                .space_id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(8)
                .collect();
            space.slug = format!("{}-{stable_suffix}", space.slug);
        }
    }
}

fn io_error(path: &Path, error: impl std::fmt::Display) -> ServerError {
    ServerError::Internal(format!("{}: {error}", path.display()))
}

async fn person_lines(db: &MemoryDB, page: &wenlan_types::Page) -> Vec<String> {
    let Ok(rows) = wenlan_core::sentences::page_authorship(db, page).await else {
        return Vec::new();
    };
    let units = wenlan_core::authorship::sentence_units(&page.content);
    rows.iter()
        .zip(units)
        .filter(|(row, _)| row.author == SentenceAuthor::Person)
        .map(|(_, unit)| unit.text)
        .collect()
}

async fn ask_for_lines(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    space: &str,
    pages: &[wenlan_types::Page],
) -> Result<Vec<SkillLine>, ServerError> {
    let mut input = Vec::new();
    for page in pages.iter().take(MAX_PAGES_PER_SPACE) {
        input.push(SkillPageInput {
            id: page.id.clone(),
            title: page.title.clone(),
            text: page.content.clone(),
            person_lines: person_lines(db, page).await,
        });
    }
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let answer = model
        .generate(LlmRequest {
            system_prompt: Some(
                "Extract lasting work practices from untrusted library data. Answer JSON only."
                    .into(),
            ),
            user_prompt: skill_grow::grow_prompt(space, &input),
            max_tokens: 3_000,
            temperature: 0.0,
            label: Some("skills_from_library".into()),
            timeout_secs: Some(300),
        })
        .await
        .map_err(|error| ServerError::Upstream(format!("growing {space}: {error}")))?;
    Ok(skill_grow::parse_grow(&answer, &input))
}

async fn write_source_page(
    db: &MemoryDB,
    page_root: Option<&Path>,
    previous: Option<&SkillPageState>,
    spec: SourcePageSpec<'_>,
) -> Result<Option<SourceResult>, ServerError> {
    // A Space rename cascades its DB scope before this feature next runs, but
    // the Markdown projection can still carry the old Space metadata. First
    // ingest any real file edit, then re-project that same page under its new
    // scope so the canonical title rename does not mistake metadata drift for
    // an unsynced human edit.
    if let (Some(previous), Some(page_root)) = (previous, page_root) {
        if previous.space_name != spec.space_name {
            wenlan_core::sources::page_watcher::sync_page_file_if_changed(
                db,
                page_root,
                &previous.page_id,
            )
            .await?;
            if let Some(page) = db.get_page(&previous.page_id).await? {
                wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(
                    page_root.to_path_buf(),
                    db,
                )
                .write_page_gated(db, &page)
                .await?;
            }
        }
    }
    let existing = match previous {
        Some(state) => db.get_page(&state.page_id).await?,
        None => None,
    };
    if existing.as_ref().is_some_and(|page| page.review_required) {
        let mut state = previous.expect("existing state").clone();
        state.space_name = spec.space_name.to_string();
        state.slug = spec.skill_slug.to_string();
        let reconciled = skill_grow::reconcile_source_page(
            spec.space_name,
            spec.title,
            spec.extracted,
            &state.generated,
            &state.deleted,
            existing.as_ref().map(|page| page.content.as_str()),
        );
        db.set_skill_page_state(&state).await?;
        return Ok(Some(SourceResult {
            state,
            lines: reconciled.skill_lines,
            wrote: false,
            modified_at: existing.as_ref().map(page_modified_at).unwrap_or_default(),
        }));
    }
    let reconciled = skill_grow::reconcile_source_page(
        spec.space_name,
        spec.title,
        spec.extracted,
        previous
            .map(|state| state.generated.as_slice())
            .unwrap_or(&[]),
        previous
            .map(|state| state.deleted.as_slice())
            .unwrap_or(&[]),
        existing.as_ref().map(|page| page.content.as_str()),
    );
    // A Space below the three-line floor gets no new source page or file.
    // Existing source pages still update so edits/deletions remain visible.
    if existing.is_none() && reconciled.skill_lines.len() < 3 && spec.space_id != GENERAL_ID {
        return Ok(None);
    }

    let (page_id, wrote) = match existing {
        None => {
            let result = wenlan_core::post_write::create_page(
                db,
                CreateConceptRequest {
                    title: spec.title.to_string(),
                    content: reconciled.body.clone(),
                    summary: None,
                    entity_id: None,
                    space: spec.target,
                    source_memory_ids: Vec::new(),
                    // This generated page has no memory ids to cite, so the
                    // canonical create contract requires `authored`. The
                    // non-person agent name records its birth as
                    // `agent_create`; later human edits are still protected
                    // sentence by sentence.
                    creation_kind: Some("authored".into()),
                    workspace: spec.workspace,
                },
                "wenlan-skills",
                page_root,
            )
            .await?;
            if result.attached_to.is_some() || !result.wrote {
                return Err(ServerError::Conflict(format!(
                    "a page named {title:?} already exists; Wenlan did not claim or overwrite it",
                    title = spec.title
                )));
            }
            (result.id, true)
        }
        Some(mut page) => {
            if page.title != spec.title {
                wenlan_core::post_write::rename_page(
                    db,
                    &page.id,
                    spec.title,
                    page.version,
                    &wenlan_core::read_scope::ReadScope::Global,
                    page_root,
                )
                .await?;
                page = db
                    .get_page(&page.id)
                    .await?
                    .ok_or_else(|| ServerError::Internal("skill source page disappeared".into()))?;
            }
            if page.content == reconciled.body {
                (page.id, false)
            } else {
                let result = wenlan_core::post_write::update_page(
                    db,
                    &page.id,
                    UpdatePageRequest {
                        content: reconciled.body.clone(),
                        source_memory_ids: Vec::new(),
                        expected_version: Some(page.version),
                        caller_id: None,
                        operation_id: None,
                    },
                    "agent_refresh",
                    false,
                    page_root,
                    None,
                )
                .await?;
                if result.revision_card_id.is_some() || result.gated {
                    return Ok(None);
                }
                (page.id, result.wrote)
            }
        }
    };
    let modified_at = db
        .get_page(&page_id)
        .await?
        .as_ref()
        .map(page_modified_at)
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let state = SkillPageState {
        scope_kind: if spec.space_id == GENERAL_ID {
            "personal_general".into()
        } else {
            "personal_space".into()
        },
        space_id: spec.space_id.to_string(),
        space_name: spec.space_name.to_string(),
        slug: spec.skill_slug.to_string(),
        page_id,
        generated: reconciled.generated,
        deleted: reconciled.deleted,
        last_grown_at: chrono::Utc::now().timestamp(),
    };
    db.set_skill_page_state(&state).await?;
    Ok(Some(SourceResult {
        state,
        lines: reconciled.skill_lines,
        wrote,
        modified_at,
    }))
}

fn replaceable_managed_dir(path: &Path) -> Result<Option<bool>, ServerError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Some(false)),
        Err(error) => Err(io_error(path, error)),
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => Ok(None),
        Ok(_) => {
            let marker = path.join(MANAGED_MARKER);
            match std::fs::read_to_string(&marker) {
                Ok(value) if value == MANAGED_VALUE => Ok(Some(true)),
                Ok(_) => Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(io_error(&marker, error)),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallOutcome {
    Written,
    Unchanged,
    Unmanaged,
}

fn write_managed_skill(
    parent: &Path,
    name: &str,
    content: &str,
) -> Result<InstallOutcome, ServerError> {
    let target = parent.join(name);
    let replace = match replaceable_managed_dir(&target)? {
        Some(replace) => replace,
        None => return Ok(InstallOutcome::Unmanaged),
    };
    if replace
        && std::fs::read_to_string(target.join("SKILL.md"))
            .ok()
            .as_deref()
            == Some(content)
    {
        return Ok(InstallOutcome::Unchanged);
    }
    std::fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let unique = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let staged = parent.join(format!(".{name}.wenlan-stage-{unique}"));
    std::fs::create_dir(&staged).map_err(|error| io_error(&staged, error))?;
    if let Err(error) = (|| {
        std::fs::write(staged.join("SKILL.md"), content)
            .map_err(|error| io_error(&staged.join("SKILL.md"), error))?;
        std::fs::write(staged.join(MANAGED_MARKER), MANAGED_VALUE)
            .map_err(|error| io_error(&staged.join(MANAGED_MARKER), error))?;
        Ok::<_, ServerError>(())
    })() {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }
    let aside = parent.join(format!(".{name}.wenlan-old-{unique}"));
    if replace {
        std::fs::rename(&target, &aside).map_err(|error| io_error(&target, error))?;
    }
    if let Err(error) = std::fs::rename(&staged, &target) {
        if replace {
            let _ = std::fs::rename(&aside, &target);
        }
        let _ = std::fs::remove_dir_all(&staged);
        return Err(io_error(&target, error));
    }
    if replace {
        std::fs::remove_dir_all(&aside).map_err(|error| io_error(&aside, error))?;
    }
    Ok(InstallOutcome::Written)
}

fn remove_managed_skill(path: &Path) -> Result<bool, ServerError> {
    if replaceable_managed_dir(path)? != Some(true) {
        return Ok(false);
    }
    std::fs::remove_dir_all(path).map_err(|error| io_error(path, error))?;
    Ok(true)
}

async fn connected_skill_dirs(
    db: &MemoryDB,
    dirs: &SkillDirs,
) -> Result<Vec<(String, PathBuf)>, ServerError> {
    let mut out = Vec::new();
    for agent in db
        .list_agents()
        .await?
        .into_iter()
        .filter(|agent| agent.enabled)
    {
        let Some(location) = wenlan_core::skill_locations::skills_dir_for(
            &agent.name,
            dirs.home.as_deref(),
            dirs.config.as_deref(),
        ) else {
            continue;
        };
        match location {
            Ok(path) => out.push((agent.name, path)),
            Err(what) => tracing::warn!("[skills] {}: could not determine {what}", agent.name),
        }
    }
    Ok(out)
}

async fn install_desired(
    db: &MemoryDB,
    dirs: &SkillDirs,
    desired: &[DesiredSkill],
    report: &mut SkillsRefreshResponse,
) -> Result<Vec<PathBuf>, ServerError> {
    let tool_dirs = connected_skill_dirs(db, dirs).await?;
    let mut wanted = HashSet::new();
    let mut parents = HashSet::new();
    for (tool, parent) in &tool_dirs {
        parents.insert(parent.clone());
        for skill in desired {
            let key = (tool.clone(), skill.name.clone());
            wanted.insert(key.clone());
            match write_managed_skill(parent, &skill.name, &skill.content)? {
                InstallOutcome::Written => report.skills_written += 1,
                InstallOutcome::Unchanged => report.skills_unchanged += 1,
                InstallOutcome::Unmanaged => {
                    report.skills_skipped += 1;
                    continue;
                }
            }
            db.set_skill_install_state(&SkillInstallState {
                tool: tool.clone(),
                name: skill.name.clone(),
                dir: parent.join(&skill.name).to_string_lossy().into_owned(),
                content_hash: content_hash(&skill.content),
            })
            .await?;
        }
    }
    for old in db.skill_install_states().await? {
        if wanted.contains(&(old.tool.clone(), old.name.clone())) {
            continue;
        }
        let _ = remove_managed_skill(Path::new(&old.dir))?;
        db.delete_skill_install_state(&old.tool, &old.name).await?;
    }
    Ok(parents.into_iter().collect())
}

fn unmanaged_skill_files(parents: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for parent in parents {
        let Ok(entries) = std::fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_UNMANAGED_SKILLS {
                return out;
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_dir() || meta.file_type().is_symlink() || path.join(MANAGED_MARKER).exists()
            {
                continue;
            }
            let file = path.join("SKILL.md");
            if !file.is_file() || !seen.insert(file.clone()) {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("skill")
                .to_string();
            out.push((name, file));
        }
    }
    out
}

async fn refresh_suggestions(
    db: &MemoryDB,
    model: &dyn LlmProvider,
    parents: &[PathBuf],
    candidates: &[(SkillLine, i64)],
) -> Result<usize, ServerError> {
    let stored: HashMap<String, StoredSkillSuggestion> = db
        .skill_suggestions()
        .await?
        .into_iter()
        .map(|item| (item.suggestion.id.clone(), item))
        .collect();
    let mut written = 0;
    let mut current = HashSet::new();
    for (name, file) in unmanaged_skill_files(parents) {
        let Ok(existing) = std::fs::read_to_string(&file) else {
            continue;
        };
        let modified = std::fs::metadata(&file)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(i64::MAX);
        let review: Vec<OldSkillCandidate> = candidates
            .iter()
            .cloned()
            .map(|(line, source_at)| OldSkillCandidate {
                line,
                newer_than_skill: source_at > modified,
            })
            .collect();
        if review.is_empty() {
            continue;
        }
        let answer = model
            .generate(LlmRequest {
                system_prompt: Some(
                    "Compare an unmanaged skill with newer library decisions. Answer JSON only."
                        .into(),
                ),
                user_prompt: skill_grow::old_skill_prompt(&name, &existing, &review),
                max_tokens: 1_500,
                temperature: 0.0,
                label: Some("skills_conflict_check".into()),
                timeout_secs: Some(300),
            })
            .await
            .map_err(|error| ServerError::Upstream(format!("checking skill {name}: {error}")))?;
        for suggestion in
            skill_grow::parse_old_skill_suggestions(&answer, &name, &existing, &review)
        {
            current.insert(suggestion.id.clone());
            if stored.get(&suggestion.id).is_some_and(|item| {
                matches!(
                    item.status,
                    SuggestionStatus::Dismissed | SuggestionStatus::Applied
                )
            }) {
                continue;
            }
            db.set_skill_suggestion(&StoredSkillSuggestion {
                suggestion,
                skill_file: file.to_string_lossy().into_owned(),
                status: SuggestionStatus::Pending,
                backup_file: None,
            })
            .await?;
            written += 1;
        }
    }
    for (id, old) in stored {
        if old.status == SuggestionStatus::Pending && !current.contains(&id) {
            db.delete_skill_suggestion(&id).await?;
        }
    }
    Ok(written)
}

async fn refresh_unlocked(
    shared: &SharedState,
    model: &dyn LlmProvider,
    dirs: SkillDirs,
) -> Result<SkillsRefreshResponse, ServerError> {
    if !wenlan_core::config::load_config().skills_from_library {
        return Err(ServerError::Conflict(
            "skills_from_library is off; enable it before refreshing".into(),
        ));
    }
    let (db, page_root) = {
        let state = shared.read().await;
        (
            state.db.clone().ok_or(ServerError::DbNotInitialized)?,
            state.lint_config.page_root().map(Path::to_path_buf),
        )
    };
    let previous: HashMap<String, SkillPageState> = db
        .skill_page_states()
        .await?
        .into_iter()
        .map(|state| (state.space_id.clone(), state))
        .collect();
    let source_ids: HashSet<String> = previous
        .values()
        .map(|state| state.page_id.clone())
        .collect();
    let spaces = db.list_spaces().await?;
    let active_ids: HashSet<String> = spaces.iter().map(|space| space.id.clone()).collect();
    let mut extracted = Vec::new();
    for space in spaces.iter().filter(|space| !private_space(&space.name)) {
        let pages = db
            .list_pages_by_space("active", Some(&space.name), 1_000, 0)
            .await?;
        let pages: Vec<_> = pages
            .into_iter()
            .filter(|page| !source_ids.contains(&page.id))
            .filter(|page| !page.review_required)
            .filter(|page| !skill_grow::page_is_not_for_ai_tools(&page.content))
            .collect();
        let lines = ask_for_lines(&db, model, &space.name, &pages).await?;
        extracted.push(SpaceLines {
            space_id: space.id.clone(),
            space_name: space.name.clone(),
            slug: skill_grow::slugify(&space.name),
            lines,
        });
    }
    let MergedLines {
        general,
        mut spaces,
    } = skill_grow::merge_shared(extracted);
    disambiguate_space_slugs(&mut spaces);
    let mut report = SkillsRefreshResponse {
        spaces_read: spaces.len(),
        ..Default::default()
    };
    let mut desired = Vec::new();
    let mut suggestion_candidates = Vec::new();
    for space in spaces {
        let title = source_title(&space.space_name);
        let result = write_source_page(
            &db,
            page_root.as_deref(),
            previous.get(&space.space_id),
            SourcePageSpec {
                space_id: &space.space_id,
                space_name: &space.space_name,
                skill_slug: &space.slug,
                title: &title,
                target: WriteSpaceTarget::Named(space.space_name.clone()),
                workspace: Some(space.space_name.clone()),
                extracted: &space.lines,
            },
        )
        .await?;
        let Some(result) = result else {
            report.skills_skipped += 1;
            continue;
        };
        report.source_pages_written += usize::from(result.wrote);
        for line in &result.lines {
            suggestion_candidates.push((line.clone(), result.modified_at));
        }
        if result.lines.len() < 3 {
            report.skills_skipped += 1;
            continue;
        }
        let name = format!("wenlan-{}", result.state.slug);
        desired.push(DesiredSkill {
            content: skill_grow::render_skill(&name, &space.space_name, &result.lines),
            name,
        });
    }
    if !general.is_empty() {
        if let Some(result) = write_source_page(
            &db,
            page_root.as_deref(),
            previous.get(GENERAL_ID),
            SourcePageSpec {
                space_id: GENERAL_ID,
                space_name: "all Spaces",
                skill_slug: "me",
                title: "How I work across Spaces",
                target: WriteSpaceTarget::Uncategorized,
                workspace: None,
                extracted: &general,
            },
        )
        .await?
        {
            report.source_pages_written += usize::from(result.wrote);
            for line in &result.lines {
                suggestion_candidates.push((line.clone(), result.modified_at));
            }
            if !result.lines.is_empty() {
                desired.push(DesiredSkill {
                    name: "wenlan-me".into(),
                    content: skill_grow::render_skill("wenlan-me", "all Spaces", &result.lines),
                });
            }
        }
    }
    // Space deletion removes its ledger state. Its installed folder is removed
    // by install_desired's stale-install pass below.
    for state in previous
        .values()
        .filter(|state| state.space_id != GENERAL_ID && !active_ids.contains(&state.space_id))
    {
        db.delete_skill_page_state(&state.space_id).await?;
    }
    let parents = install_desired(&db, &dirs, &desired, &mut report).await?;
    report.suggestions = refresh_suggestions(&db, model, &parents, &suggestion_candidates).await?;
    Ok(report)
}

async fn refresh_with(
    shared: &SharedState,
    model: &dyn LlmProvider,
    dirs: SkillDirs,
) -> Result<SkillsRefreshResponse, ServerError> {
    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    refresh_unlocked(shared, model, dirs).await
}

pub async fn run_nightly_if_due(
    shared: &SharedState,
    model: &dyn LlmProvider,
) -> Result<Option<SkillsRefreshResponse>, ServerError> {
    if !wenlan_core::config::load_config().skills_from_library {
        return Ok(None);
    }
    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    let db = shared
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let today = chrono::Local::now().date_naive();
    if let Some(last) = db.skill_last_nightly().await? {
        if chrono::Local
            .timestamp_opt(last, 0)
            .single()
            .is_some_and(|at| at.date_naive() == today)
        {
            return Ok(None);
        }
    }
    let result = refresh_unlocked(shared, model, SkillDirs::system()).await?;
    db.set_skill_last_nightly(chrono::Utc::now().timestamp())
        .await?;
    Ok(Some(result))
}

pub async fn handle_refresh(
    State(state): State<SharedState>,
) -> Result<Json<SkillsRefreshResponse>, ServerError> {
    let models = crate::things_intake::models(&state)
        .await
        .ok_or_else(|| ServerError::Conflict("no grouping model is available".into()))?;
    Ok(Json(
        refresh_with(&state, models.group.as_ref(), SkillDirs::system()).await?,
    ))
}

pub async fn handle_stop(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let mut removed = 0usize;
    for install in db.skill_install_states().await? {
        removed += usize::from(remove_managed_skill(Path::new(&install.dir))?);
        db.delete_skill_install_state(&install.tool, &install.name)
            .await?;
    }
    wenlan_core::config::update_config(|config| {
        config.skills_from_library = false;
        Ok(())
    })
    .map_err(|error| ServerError::Internal(error.to_string()))?;
    Ok(Json(
        serde_json::json!({ "removed": removed, "enabled": false }),
    ))
}

pub async fn handle_suggestions(
    State(state): State<SharedState>,
) -> Result<Json<Vec<SkillSuggestion>>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let suggestions = db
        .skill_suggestions()
        .await?
        .into_iter()
        .filter(|item| item.status == SuggestionStatus::Pending)
        .map(|item| item.suggestion)
        .collect();
    Ok(Json(suggestions))
}

fn staged_file(path: &Path, content: &str) -> Result<PathBuf, ServerError> {
    let parent = path
        .parent()
        .ok_or_else(|| ServerError::Internal("skill file has no parent".into()))?;
    let unique = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let staged = parent.join(format!(".SKILL.md.wenlan-stage-{unique}"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|error| io_error(&staged, error))?;
    std::io::Write::write_all(&mut file, content.as_bytes())
        .map_err(|error| io_error(&staged, error))?;
    file.sync_all().map_err(|error| io_error(&staged, error))?;
    Ok(staged)
}

/// Cross-platform file replacement with a durable one-step Undo. Renaming an
/// existing path directly over another is not portable to Windows, so the old
/// file itself becomes the backup before the staged file takes its place.
fn replace_with_backup(path: &Path, content: &str, backup: &Path) -> Result<(), ServerError> {
    match std::fs::symlink_metadata(backup) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(backup, error)),
        Ok(_) => {
            return Err(ServerError::Conflict(
                "this skill already has an available Undo; use it before another edit".into(),
            ))
        }
    }
    let staged = staged_file(path, content)?;
    if let Err(error) = std::fs::rename(path, backup) {
        let _ = std::fs::remove_file(&staged);
        return Err(io_error(path, error));
    }
    if let Err(error) = std::fs::rename(&staged, path) {
        let _ = std::fs::rename(backup, path);
        let _ = std::fs::remove_file(&staged);
        return Err(io_error(path, error));
    }
    Ok(())
}

fn restore_backup(path: &Path, backup: &Path) -> Result<(), ServerError> {
    let parent = path
        .parent()
        .ok_or_else(|| ServerError::Internal("skill file has no parent".into()))?;
    let unique = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let aside = parent.join(format!(".SKILL.md.wenlan-undone-{unique}"));
    std::fs::rename(path, &aside).map_err(|error| io_error(path, error))?;
    if let Err(error) = std::fs::rename(backup, path) {
        let _ = std::fs::rename(&aside, path);
        return Err(io_error(backup, error));
    }
    std::fs::remove_file(&aside).map_err(|error| io_error(&aside, error))
}

async fn stored_suggestion(db: &MemoryDB, id: &str) -> Result<StoredSkillSuggestion, ServerError> {
    db.skill_suggestions()
        .await?
        .into_iter()
        .find(|item| item.suggestion.id == id)
        .ok_or_else(|| ServerError::NotFound("skill suggestion not found".into()))
}

pub async fn handle_apply_suggestion(
    State(state): State<SharedState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SkillSuggestionActionResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let mut stored = stored_suggestion(&db, &id).await?;
    if stored.status != SuggestionStatus::Pending {
        return Err(ServerError::Conflict("suggestion is not pending".into()));
    }
    let file = PathBuf::from(&stored.skill_file);
    let folder = file
        .parent()
        .ok_or_else(|| ServerError::Internal("skill file has no folder".into()))?;
    if folder.join(MANAGED_MARKER).exists() {
        return Err(ServerError::Conflict(
            "Wenlan-managed skills are not edited through suggestions".into(),
        ));
    }
    let meta = std::fs::symlink_metadata(&file).map_err(|error| io_error(&file, error))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(ServerError::Conflict(
            "skill file is not a regular file".into(),
        ));
    }
    let before = std::fs::read_to_string(&file).map_err(|error| io_error(&file, error))?;
    let backup = folder.join("SKILL.md.wenlan-backup");
    let after = if stored.suggestion.old_line.is_empty() {
        format!(
            "{}\n- {} — Source: {}\n",
            before.trim_end(),
            stored.suggestion.new_line,
            stored.suggestion.source_page
        )
    } else {
        let matches = before
            .lines()
            .filter(|line| *line == stored.suggestion.old_line)
            .count();
        if matches != 1 {
            return Err(ServerError::Conflict(
                "the old skill line changed; refresh suggestions".into(),
            ));
        }
        let replacement = if stored.suggestion.old_line.trim_start().starts_with("- ") {
            format!(
                "- {} — Source: {}",
                stored.suggestion.new_line, stored.suggestion.source_page
            )
        } else {
            stored.suggestion.new_line.clone()
        };
        before.replacen(&stored.suggestion.old_line, &replacement, 1)
    };
    replace_with_backup(&file, &after, &backup)?;
    stored.status = SuggestionStatus::Applied;
    stored.backup_file = Some(backup.to_string_lossy().into_owned());
    db.set_skill_suggestion(&stored).await?;
    Ok(Json(SkillSuggestionActionResponse {
        suggestion: stored.suggestion,
        applied: true,
        undo_available: true,
    }))
}

pub async fn handle_dismiss_suggestion(
    State(state): State<SharedState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SkillSuggestionActionResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let mut stored = stored_suggestion(&db, &id).await?;
    stored.status = SuggestionStatus::Dismissed;
    stored.backup_file = None;
    db.set_skill_suggestion(&stored).await?;
    Ok(Json(SkillSuggestionActionResponse {
        suggestion: stored.suggestion,
        applied: false,
        undo_available: false,
    }))
}

pub async fn handle_undo_suggestion(
    State(state): State<SharedState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SkillSuggestionActionResponse>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let mut stored = stored_suggestion(&db, &id).await?;
    if stored.status != SuggestionStatus::Applied {
        return Err(ServerError::Conflict(
            "suggestion has no available Undo".into(),
        ));
    }
    let backup = stored
        .backup_file
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| ServerError::Conflict("suggestion backup is unavailable".into()))?;
    let file = Path::new(&stored.skill_file);
    let folder = file
        .parent()
        .ok_or_else(|| ServerError::Internal("skill file has no folder".into()))?;
    if folder.join(MANAGED_MARKER).exists() {
        return Err(ServerError::Conflict(
            "Wenlan-managed skills are not edited through suggestions".into(),
        ));
    }
    for candidate in [file, backup.as_path()] {
        let meta =
            std::fs::symlink_metadata(candidate).map_err(|error| io_error(candidate, error))?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(ServerError::Conflict(
                "skill or its Undo backup is not a regular file".into(),
            ));
        }
    }
    restore_backup(file, &backup)?;
    stored.status = SuggestionStatus::Undone;
    stored.backup_file = None;
    db.set_skill_suggestion(&stored).await?;
    Ok(Json(SkillSuggestionActionResponse {
        suggestion: stored.suggestion,
        applied: false,
        undo_available: false,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RecordQuery {
    week: Option<String>,
}

fn week_bounds(week: Option<&str>) -> Result<(i64, i64), ServerError> {
    let date = match week {
        Some(raw) => chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|_| ServerError::BadRequest("week must be YYYY-MM-DD".into()))?,
        None => chrono::Local::now().date_naive(),
    };
    let monday = date - chrono::Duration::days(i64::from(date.weekday().num_days_from_monday()));
    let start = chrono::Local
        .from_local_datetime(&monday.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .ok_or_else(|| ServerError::BadRequest("week start is not a local time".into()))?
        .timestamp();
    Ok((start, start + 7 * 24 * 60 * 60))
}

pub async fn handle_record(
    State(state): State<SharedState>,
    Query(query): Query<RecordQuery>,
) -> Result<Json<Vec<SkillBorrowRecord>>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let (since, until) = week_bounds(query.week.as_deref())?;
    Ok(Json(db.skill_borrow_records(since, until).await?))
}

pub async fn handle_self_report(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<SkillReportRequest>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let tool = crate::memory_routes::extract_agent_name(&headers, None);
    if tool == "unknown" {
        return Err(ServerError::BadRequest("x-agent-name is required".into()));
    }
    let recorded = db
        .record_skill_self_report(&tool, &request.page_id, &request.report)
        .await?;
    if !recorded {
        return Err(ServerError::NotFound("skill source page not found".into()));
    }
    Ok(Json(
        serde_json::json!({ "recorded": true, "label": "AI reported" }),
    ))
}

#[cfg(test)]
#[path = "skill_grow_test.rs"]
mod tests;
