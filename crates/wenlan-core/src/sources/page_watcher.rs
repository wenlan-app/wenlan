// SPDX-License-Identifier: Apache-2.0
//! Poll-based filesystem watcher for the page projection at
//! `~/.wenlan/pages/*.md`.
//!
//! md is canonical. When the user opens a page in Obsidian / VS Code / etc.
//! and saves a change, the daemon's DB row goes stale. This module walks the
//! projection on a scheduler tick, compares each file against its DB
//! counterpart, and applies the edit back through `update_page_content`
//! with `link_reason = "fs_edit"`. That flips `user_edited` (same effect
//! as a `manual_edit` write via POST /api/pages/{id}) so the refinery's
//! re-distill escalation branch fires instead of clobbering the user's
//! prose.
//!
//! Conflict direction: if the md's `origin_version` is less than the DB's,
//! the daemon wrote last and the md on disk is stale (probably from a
//! refinery cycle that hasn't re-projected yet). We skip rather than
//! roll the DB back. The reverse (md ahead, DB behind) doesn't happen in
//! practice — the daemon is the only writer that bumps the version
//! column.
//!
//! v1 is poll-only, runs on the same cadence as the refinery scheduler
//! tick. An event-driven `notify` watcher is a follow-up if 30-second
//! latency proves too slow.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::sources::obsidian;
use std::collections::HashMap;
use std::path::Path;

/// Counts the watcher reports back to the scheduler tick. Useful for the
/// daily-status feed once the UI surface lands; for now the daemon just
/// logs `applied` so an operator can see edits flowing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WatcherStats {
    /// Total .md files inspected this pass.
    pub scanned: usize,
    /// Edits successfully written back to the DB.
    pub applied: usize,
    /// File had no `origin_id` in the frontmatter — most likely a user-
    /// added md the daemon hasn't claimed. Skipped, not auto-promoted.
    pub skipped_no_origin_id: usize,
    /// Frontmatter named a page_id the DB doesn't know. Dangling md from
    /// an archived/deleted page or a manually-pasted file.
    pub skipped_unknown_page: usize,
    /// md version stamp is behind the DB. Daemon wrote last; md will get
    /// re-projected on the next daemon write, no need to reflect it back.
    pub skipped_daemon_ahead: usize,
    /// md body matches DB content. No-op.
    pub skipped_unchanged: usize,
    /// IO or parse failures — surfaced via `log::warn!` per file.
    pub errors: usize,
    /// Pages archived because the person deleted their file in the folder.
    pub archived_deleted: usize,
    /// Set when so many page files vanished at once that none was archived.
    pub held_missing: Option<usize>,
    /// Notes the person put in the folder that became pages this pass.
    pub adopted: usize,
}

/// The mass-deletion hold last logged, so an unchanged hold logs once.
static LAST_HELD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Archive each page whose file the person deleted from the folder (see
/// [`crate::export::knowledge::note_missing_pages`] for what counts). The
/// archive is the app's own: the page leaves the pages list, its history
/// stays, and restoring it writes the file back where it was.
async fn archive_deleted_pages(db: &MemoryDB, knowledge_path: &Path, stats: &mut WatcherStats) {
    use std::sync::atomic::Ordering;
    let now = chrono::Utc::now().timestamp();
    let found = match crate::export::knowledge::note_missing_pages(knowledge_path, now) {
        Ok(found) => found,
        Err(error) => {
            log::warn!("[page-watcher] could not check for deleted pages: {error}");
            return;
        }
    };
    if let Some(gone) = found.held {
        stats.held_missing = Some(gone);
        if LAST_HELD.swap(gone, Ordering::Relaxed) != gone {
            log::warn!(
                "[page-watcher] {gone} page files are gone from the folder at once; \
                 archiving none of them until most are back or they are archived in the app"
            );
        }
        return;
    }
    LAST_HELD.store(0, Ordering::Relaxed);
    for id in found.deleted {
        let version = match db.get_page(&id).await {
            Ok(Some(page)) if page.status == "active" => page.version,
            Ok(_) => continue,
            Err(error) => {
                warn_once(&id, &format!("[page-watcher] {id}: {error}"));
                stats.errors += 1;
                continue;
            }
        };
        // Wenlan may have written the file again since the check; and only
        // the version seen here is archived, so a write after this keeps it.
        match crate::export::knowledge::page_file_still_gone(knowledge_path, &id) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                warn_once(&id, &format!("[page-watcher] {id}: {error}"));
                stats.errors += 1;
                continue;
            }
        }
        let version = match db.archive_page_at_version(&id, version).await {
            Ok(Some(version)) => version,
            Ok(None) => continue,
            Err(error) => {
                warn_once(
                    &id,
                    &format!("[page-watcher] could not archive deleted page {id}: {error}"),
                );
                stats.errors += 1;
                continue;
            }
        };
        let projection = crate::export::knowledge::KnowledgeProjectionWrite::new(
            knowledge_path.to_path_buf(),
            db,
        );
        match projection
            .archive_page_if_still_archived(db, &id, version)
            .await
        {
            Ok(_) => {
                stats.archived_deleted += 1;
                log::info!("[page-watcher] page {id} was deleted in the folder; archived it");
            }
            Err(error) => {
                // The page is archived; its record is cleaned at the next start.
                warn_once(
                    &id,
                    &format!("[page-watcher] archived deleted page {id}; record kept: {error}"),
                );
                stats.errors += 1;
            }
        }
    }
}

/// Logs `message` the first time `key` fails in this process, then at debug
/// level, so a page that cannot be archived does not warn every pass.
fn warn_once(key: &str, message: &str) {
    static WARNED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
        std::sync::Mutex::new(None);
    let first = WARNED
        .lock()
        .map(|mut warned| {
            warned
                .get_or_insert_with(Default::default)
                .insert(key.to_string())
        })
        .unwrap_or(true);
    if first {
        log::warn!("{message}");
    } else {
        log::debug!("{message}");
    }
}

/// Notes adopted in one pass at most; the rest follow in the next ones, so
/// a folder of old notes does not hold the pass for long.
const ADOPT_PER_PASS: usize = 50;

/// A note is adopted only once it has not changed for this long, so the id
/// line is never written while an editor is still saving it.
const ADOPT_AFTER_UNCHANGED: std::time::Duration = std::time::Duration::from_secs(10);

/// Scan `knowledge_path` for page md files, reflect external edits back
/// into the DB. Idempotent and safe to call on every scheduler tick.
pub async fn sync_filesystem_edits(
    db: &MemoryDB,
    knowledge_path: &Path,
) -> Result<WatcherStats, WenlanError> {
    sync_pass(db, knowledge_path, None).await
}

/// [`sync_filesystem_edits`] for only the files in `changed`, for a
/// watcher that already knows which files were saved since its last look.
pub async fn sync_filesystem_edits_in(
    db: &MemoryDB,
    knowledge_path: &Path,
    changed: &[String],
) -> Result<WatcherStats, WenlanError> {
    sync_pass(db, knowledge_path, Some(changed)).await
}

/// When each page file was last seen by [`sync_page_file_if_changed`].
static SEEN: std::sync::LazyLock<std::sync::Mutex<HashMap<String, std::time::SystemTime>>> =
    std::sync::LazyLock::new(Default::default);

/// Reads one page's file back into the DB when it was saved since the last
/// look, so a page open in the app shows an Obsidian edit within seconds
/// instead of waiting for the scheduler's pass. Returns whether it applied.
pub async fn sync_page_file_if_changed(
    db: &MemoryDB,
    knowledge_path: &Path,
    page_id: &str,
) -> Result<bool, WenlanError> {
    let writer = crate::export::knowledge::KnowledgeWriter::new(knowledge_path.to_path_buf(), db);
    let Some(file) = writer.page_filename(page_id) else {
        return Ok(false);
    };
    let modified = match crate::export::knowledge::read_knowledge_markdown(knowledge_path, &file) {
        Ok((_, modified)) => modified,
        Err(WenlanError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    let key = knowledge_path.join(&file).to_string_lossy().into_owned();
    let unchanged = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        == Some(&modified);
    if unchanged {
        return Ok(false);
    }
    let stats = sync_filesystem_edits_in(db, knowledge_path, &[file]).await?;
    SEEN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, modified);
    Ok(stats.applied > 0)
}

/// One pass at a time: the scheduler's full pass and the quick watcher
/// would otherwise apply the same edit twice.
static PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn sync_pass(
    db: &MemoryDB,
    knowledge_path: &Path,
    only: Option<&[String]>,
) -> Result<WatcherStats, WenlanError> {
    let _pass = PASS.lock().await;
    let mut stats = WatcherStats::default();
    if !knowledge_path.exists() {
        return Ok(stats);
    }
    // A page the person moved or renamed in the folder keeps its identity:
    // its record follows the file before edits are looked for, so an edit
    // made together with the move is read back too.
    match crate::export::knowledge::follow_external_moves(knowledge_path) {
        Ok(0) => {}
        Ok(followed) => {
            log::info!("[page-watcher] followed {followed} page(s) moved in the folder")
        }
        Err(error) => log::warn!("[page-watcher] could not follow moved pages: {error}"),
    }
    // A file deleted in the folder archives its page, which a restore brings
    // back. Only a full pass decides, since a targeted one did not look.
    if only.is_none() {
        finish_pending_adoptions(db, knowledge_path).await;
        archive_deleted_pages(db, knowledge_path, &mut stats).await;
    }
    // Recovery precedes discovery, so a pending move cannot resurrect its old path.
    let targets = crate::export::knowledge::knowledge_edit_targets(knowledge_path)?;
    let person = crate::export::knowledge::person_page_files(knowledge_path)?;
    let mut paths = crate::export::knowledge::knowledge_markdown_paths(knowledge_path)?;
    if let Some(only) = only {
        paths.retain(|path| only.contains(path));
    }
    let mut unclaimed = Vec::new();
    for relative in paths {
        let path = knowledge_path.join(&relative);
        stats.scanned += 1;
        match sync_one_file(db, &relative, knowledge_path, &targets, &person).await {
            Ok(Outcome::Applied { page_id }) => {
                stats.applied += 1;
                log::info!(
                    "[page-watcher] applied fs_edit for {page_id} from {}",
                    path.display()
                );
            }
            Ok(Outcome::SkippedNoOriginId) => {
                stats.skipped_no_origin_id += 1;
                unclaimed.push(relative);
            }
            Ok(Outcome::SkippedUnknownPage { page_id }) => {
                stats.skipped_unknown_page += 1;
                log::debug!(
                    "[page-watcher] {} references unknown page {page_id}",
                    path.display()
                );
            }
            Ok(Outcome::SkippedDaemonAhead { page_id }) => {
                stats.skipped_daemon_ahead += 1;
                log::debug!("[page-watcher] {page_id}: md trails DB version, skipping");
            }
            Ok(Outcome::Unchanged) => stats.skipped_unchanged += 1,
            Err(e) => {
                stats.errors += 1;
                log::warn!("[page-watcher] {}: {e}", path.display());
            }
        }
    }
    // Notes the person put in the folder become pages. Only on a full
    // pass: the quick one looks at a page already open in the app.
    if only.is_none() {
        for relative in unclaimed {
            if stats.adopted >= ADOPT_PER_PASS {
                break;
            }
            match adopt_note(db, knowledge_path, &relative, &person).await {
                Ok(Some(page_id)) => {
                    stats.adopted += 1;
                    log::info!("[page-watcher] {relative} became page {page_id}");
                }
                Ok(None) => {}
                Err(e) => {
                    stats.errors += 1;
                    log::warn!("[page-watcher] could not adopt {relative}: {e}");
                }
            }
        }
    }
    Ok(stats)
}

/// Whether a note in the folder is left as a file and not made a page:
/// Obsidian templates, sync apps' conflict copies, Excalidraw drawings
/// (stored as Markdown), and Wenlan's own index and log. Folders whose name
/// starts with a dot (`.obsidian/`, `.trash/`) are never read at all.
fn left_as_a_file(relative: &str) -> bool {
    let lower = relative.to_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    parts
        .iter()
        .any(|folder| matches!(*folder, "templates" | "_templates" | "template"))
        || name.contains("conflicted copy")
        || name.contains("sync-conflict")
        || name.contains("(conflict")
        || name.ends_with(".excalidraw.md")
        || crate::export::knowledge::is_reserved_okf_filename(name)
}

/// The page title of a note: its frontmatter `title`, else its file name.
fn note_title(relative: &str, fm: &obsidian::NoteFrontmatter) -> String {
    fm.get_str("title")
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let name = relative.rsplit('/').next().unwrap_or(relative);
            name.strip_suffix(".md").unwrap_or(name).trim().to_string()
        })
}

/// Makes the note at `relative`, which has no `origin_id`, a page: the
/// person's writing (an authored page, so the AI only proposes changes to
/// it), with its text exactly as in the file. The file gets one line, the
/// page id, at the top of its frontmatter; everything else in it stays.
/// Returns the page id, or `None` when the note is left alone (empty, still
/// being saved, or one of [`left_as_a_file`]).
async fn adopt_note(
    db: &MemoryDB,
    knowledge_path: &Path,
    relative: &str,
    person: &HashMap<String, crate::export::knowledge::PersonPageFile>,
) -> Result<Option<String>, WenlanError> {
    if left_as_a_file(relative) {
        return Ok(None);
    }
    let (raw, modified) =
        crate::export::knowledge::read_knowledge_markdown(knowledge_path, relative)?;
    let settled = std::time::SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= ADOPT_AFTER_UNCHANGED);
    if !settled || crate::export::knowledge::origin_id_line(&raw).is_some() {
        return Ok(None);
    }
    // Already a page: an editor that had the note open since before its id
    // line was added saved over it. The line goes back; the text is kept and
    // read back as the person's edit on the next pass.
    if let Some((page_id, _)) = person.iter().find(|(_, file)| file.file == relative) {
        crate::export::knowledge::restamp_person_note(knowledge_path, relative, &raw, page_id)?;
        log::info!("[page-watcher] put the id line of page {page_id} back into {relative}");
        return Ok(None);
    }
    let text = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let (fm, _) = obsidian::extract_frontmatter(text);
    let content = projected_content(text);
    if content.trim().is_empty()
        || crate::export::provenance::validate_canonical_page_content(&content).is_err()
    {
        return Ok(None);
    }
    let page_id = crate::pages::new_page_id();
    let stamped =
        crate::export::knowledge::stamp_person_note(knowledge_path, relative, &raw, &page_id)?;
    create_adopted_page(
        db,
        knowledge_path,
        relative,
        &page_id,
        &stamped,
        &note_title(relative, &fm),
    )
    .await?;
    Ok(Some(page_id))
}

async fn create_adopted_page(
    db: &MemoryDB,
    knowledge_path: &Path,
    relative: &str,
    page_id: &str,
    stamped: &str,
    title: &str,
) -> Result<(), WenlanError> {
    if db.get_page(page_id).await?.is_none() {
        let req = wenlan_types::requests::CreateConceptRequest {
            title: title.to_string(),
            content: projected_content(stamped),
            summary: None,
            entity_id: None,
            space: wenlan_types::WriteSpaceTarget::Uncategorized,
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".to_string()),
            workspace: None,
        };
        crate::post_write::page_write(
            db,
            crate::post_write::PageWrite::Create {
                page_id: Some(page_id),
                req,
                agent: "fs_edit",
                // The file is already there: the page is recorded at it
                // below, not written again.
                knowledge_path: None,
                page_min_cluster_size: usize::MAX,
                page_match_threshold: 1.0,
                citations_json: None,
                basis_marks: Vec::new(),
                basis_space_guard: None,
                skip_protected_title_match: true,
            },
        )
        .await?;
    }
    let page = db
        .get_page(page_id)
        .await?
        .ok_or_else(|| WenlanError::Conflict("page_adoption_lost".into()))?;
    crate::export::knowledge::record_person_note(knowledge_path, relative, &page)
}

/// Finishes adoptions a stop interrupted after the id line was written.
async fn finish_pending_adoptions(db: &MemoryDB, knowledge_path: &Path) {
    let pending = match crate::export::knowledge::pending_adoptions(knowledge_path) {
        Ok(pending) => pending,
        Err(error) => {
            log::warn!("[page-watcher] could not read pending adoptions: {error}");
            return;
        }
    };
    for (page_id, mut relative) in pending {
        let result = async {
            let raw = match crate::export::knowledge::read_knowledge_markdown(
                knowledge_path,
                &relative,
            ) {
                Ok((raw, _)) => raw,
                Err(WenlanError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    // Renamed or moved since its id line went in: finish it
                    // where it is now.
                    match find_note_with_id(knowledge_path, &page_id)? {
                        Some((moved, raw)) => {
                            relative = moved;
                            raw
                        }
                        None => {
                            return crate::export::knowledge::drop_pending_adoption(
                                knowledge_path,
                                &page_id,
                            );
                        }
                    }
                }
                Err(error) => return Err(error),
            };
            if crate::export::knowledge::origin_id_line(&raw).as_deref() != Some(page_id.as_str()) {
                // The id line never landed, or the person took it out: the
                // note is adopted afresh when it is seen without one.
                return crate::export::knowledge::drop_pending_adoption(knowledge_path, &page_id);
            }
            let content = projected_content(&raw);
            if db.get_page(&page_id).await?.is_none()
                && (content.trim().is_empty()
                    || crate::export::provenance::validate_canonical_page_content(&content)
                        .is_err())
            {
                // Emptied (or given Wenlan's own sources marker) since: it
                // cannot be a page now, so the id line comes out again.
                return crate::export::knowledge::unstamp_person_note(
                    knowledge_path,
                    &relative,
                    &raw,
                    &page_id,
                );
            }
            let (fm, _) = obsidian::extract_frontmatter(&raw);
            create_adopted_page(
                db,
                knowledge_path,
                &relative,
                &page_id,
                &raw,
                &note_title(&relative, &fm),
            )
            .await
        }
        .await;
        if let Err(error) = result {
            log::warn!("[page-watcher] could not finish adopting {relative}: {error}");
        }
    }
}

/// The note in the folder carrying `page_id`, with its text.
fn find_note_with_id(
    knowledge_path: &Path,
    page_id: &str,
) -> Result<Option<(String, String)>, WenlanError> {
    for relative in crate::export::knowledge::knowledge_markdown_paths(knowledge_path)? {
        let Ok((raw, _)) =
            crate::export::knowledge::read_knowledge_markdown(knowledge_path, &relative)
        else {
            continue;
        };
        if crate::export::knowledge::origin_id_line(&raw).as_deref() == Some(page_id) {
            return Ok(Some((relative, raw)));
        }
    }
    Ok(None)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

enum Outcome {
    Applied { page_id: String },
    SkippedNoOriginId,
    SkippedUnknownPage { page_id: String },
    SkippedDaemonAhead { page_id: String },
    Unchanged,
}

use crate::export::provenance::{
    page_body_from_projection as projected_content, same_page_text_ignoring_save_noise,
};

async fn sync_one_file(
    db: &MemoryDB,
    relative: &str,
    knowledge_path: &Path,
    targets: &HashMap<String, String>,
    person: &HashMap<String, crate::export::knowledge::PersonPageFile>,
) -> Result<Outcome, WenlanError> {
    let (raw, _) = crate::export::knowledge::read_knowledge_markdown(knowledge_path, relative)?;
    let (fm, _) = obsidian::extract_frontmatter(&raw);
    // Line by line too, for a note whose frontmatter YAML does not parse.
    let page_id = match fm
        .get_str("origin_id")
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| crate::export::knowledge::origin_id_line(&raw))
    {
        Some(id) => id,
        None => return Ok(Outcome::SkippedNoOriginId),
    };

    let existing = match db.get_page(&page_id).await? {
        Some(p) => p,
        None => return Ok(Outcome::SkippedUnknownPage { page_id }),
    };
    if existing.status == "archived"
        && crate::export::knowledge::KnowledgeWriter::new(knowledge_path.to_path_buf(), db)
            .page_filename(&page_id)
            .is_none()
    {
        bring_back_archived_page(db, knowledge_path, relative, &raw, existing).await?;
        return Ok(Outcome::Applied { page_id });
    }

    // Skip when md is stale relative to DB. `origin_version` in the
    // frontmatter is written by KnowledgeWriter::render_markdown and
    // tracks the DB's version column at projection time. If the daemon
    // bumped version without re-projecting (e.g. refinery write without
    // KnowledgeWriter call), the md is behind — we'd otherwise roll the
    // DB back to a stale body.
    // Shared with the startup reconcile, which repairs exactly the files this
    // branch declines to touch — they must read the stamp identically or
    // reconcile would overwrite what this skip protects.
    // A note the person put in the folder carries no version stamp: its
    // bytes are compared with the ones recorded for it instead.
    let person_file = person.get(&page_id).filter(|file| file.file == relative);
    if let Some(file) = person_file {
        if file.digest.as_deref() == Some(sha256_hex(raw.as_bytes()).as_str()) {
            return Ok(Outcome::Unchanged);
        }
    } else {
        let md_version = crate::export::knowledge::projected_origin_version(&fm);
        if md_version < existing.version {
            return Ok(Outcome::SkippedDaemonAhead { page_id });
        }
    }

    // Decode BOTH sides with the projection's own envelope decoder: it drops
    // the writer-owned separators and the export-only Sources block (which
    // the projected file always carries and Page.content never does) and
    // keeps every authored byte. Comparing against a fresh render of the DB
    // row, link conversion included, means a fresh export is never mistaken
    // for an external edit; a raw compare would mark every page
    // user_edited on the first tick and lock the refinery out.
    let body_norm = projected_content(&raw);
    let fresh = crate::export::knowledge::render_markdown_for(&existing);
    // Line endings and whitespace after the last visible character are what
    // an editor changes on save without the author typing (see
    // `same_page_text_ignoring_save_noise`); they are not a human edit.
    // Indentation, leading blank lines and inner trailing spaces are.
    let mut existing = existing;
    let mut fresh = fresh;
    if let Some(file) = person_file {
        // The person changed the `title:` in their own frontmatter: the page
        // is renamed to it, as a rename in the app would.
        let title_of = |yaml: &str| {
            serde_yaml::from_str::<HashMap<String, serde_yaml::Value>>(yaml)
                .ok()
                .and_then(|fields| {
                    fields
                        .get("title")
                        .and_then(serde_yaml::Value::as_str)
                        .map(|title| title.trim().to_string())
                })
                .filter(|title| !title.is_empty())
        };
        let now = crate::export::knowledge::adoption_frontmatter(&raw)
            .as_deref()
            .and_then(title_of);
        if let Some(title) = now.filter(|title| {
            Some(title) != title_of(&file.frontmatter).as_ref() && *title != existing.title
        }) {
            match crate::post_write::rename_page(
                db,
                &page_id,
                &title,
                existing.version,
                &crate::read_scope::ReadScope::Global,
                None,
            )
            .await
            {
                Ok(_) => {
                    if let Some(renamed) = db.get_page(&page_id).await? {
                        existing = renamed;
                        fresh = crate::export::knowledge::render_markdown_for(&existing);
                    }
                }
                Err(error) => log::warn!(
                    "[page-watcher] {relative}: could not rename page {page_id} to its \
                     new title: {error}"
                ),
            }
        }
        if same_page_text_ignoring_save_noise(&body_norm, &projected_content(&fresh)) {
            // The text is the page's; only the frontmatter (or how the
            // editor saved it) changed. Kept for Wenlan's next write; the
            // person's file is never rewritten for it.
            crate::export::knowledge::note_person_file(
                knowledge_path,
                &page_id,
                &raw,
                Some(existing.version),
            )?;
            return Ok(Outcome::Unchanged);
        }
        // An edit: the frontmatter as it is now goes with it.
        crate::export::knowledge::note_person_file(knowledge_path, &page_id, &raw, None)?;
    } else if same_page_text_ignoring_save_noise(&body_norm, &projected_content(&fresh)) {
        // The page text is unchanged, but the file's body bytes may still
        // differ from what Wenlan writes: the user touched the protected
        // Sources block, or an editor rewrote line endings or trailing
        // whitespace. Re-project from DB truth so the file stops lying about
        // its sources and holds the bytes the writer recorded. The writer
        // accepts replacing such a file because its `adopted` check uses the
        // same save-noise rule. Frontmatter-only differences are left alone.
        let on_disk = obsidian::split_frontmatter_untrimmed(&raw).map(|(_, body)| body);
        let projected = obsidian::split_frontmatter_untrimmed(&fresh).map(|(_, body)| body);
        if on_disk != projected {
            let writer =
                crate::export::knowledge::KnowledgeWriter::new(knowledge_path.to_path_buf(), db);
            // Only re-project if the writer already maps this page to a file (state
            // entry present) — otherwise write_page's unique_filename would fork a
            // `<slug>-2.md` duplicate against the on-disk file we're reading (real on a
            // vault synced without `.wenlan/state.json`). Without state we can't safely
            // pick the canonical file; leave it as-is until a genuine re-distill
            // re-establishes the mapping. The page row is untouched either way.
            if writer.page_filename(&existing.id).is_some() {
                if targets.get(&page_id).map(String::as_str) != Some(relative) {
                    return Err(WenlanError::Conflict(
                        "page_projection_identity_ambiguous".into(),
                    ));
                }
                // The permit is taken before the projection lock, not inside it,
                // so no await happens while the lock is held. Skipping is right
                // here — unlike the receipted repair path, this is an ambient
                // reaction to a file the user touched, and the honest response
                // to "this page may not be projected" is to leave it alone. The
                // startup invariant is the only pass that evicts hidden pages,
                // so an ungated rewrite here would outlive the ceremony.
                match crate::truth_adapter::page_write_permit(db, &existing.id).await? {
                    Some(permit) => {
                        writer
                            .begin_projection_write()
                            .write_page_permitted(&permit, &existing)?;
                    }
                    None => log::info!(
                        "[truth] not re-projecting page {} after an edit — an automatic \
                         reader may not see it",
                        existing.id
                    ),
                }
            }
        }
        return Ok(Outcome::Unchanged);
    }

    // External moves and conflict copies are deliberately unsupported: the same
    // Page must have exactly one claimant at its recorded path before an edit.
    if targets.get(&page_id).map(String::as_str) != Some(relative) {
        return Err(WenlanError::Conflict(
            "page_projection_identity_ambiguous".into(),
        ));
    }

    // The user edited prose, not the memory provenance -- sources change
    // only via /distill refresh or explicit POST. `existing` here is a
    // snapshot taken above, before this function's only other await
    // (`page_write_permit`, only reached on the Unchanged branch above) and
    // well before the write's own CAS loop, so a source attached via API in
    // that window must not be reverted by handing the write a stale
    // `source_memory_ids` list. `update_page_preserving_sources` re-reads
    // the source list from the exact Page generation the CAS loop selects
    // (page_dispatch.rs's `UpdatePreservingSources`), so `req`'s
    // `source_memory_ids` below is never read for the actual write --
    // keeping this snapshot value on the struct is only to satisfy
    // `UpdatePageRequest`'s shape.
    let req = wenlan_types::requests::UpdatePageRequest {
        content: body_norm,
        source_memory_ids: existing.source_memory_ids.clone(),
        expected_version: None,
        caller_id: None,
        operation_id: None,
    };
    // Pass knowledge_path so update_page re-projects the md with the new
    // version stamp; without that the next tick would see origin_version
    // trailing the DB and skip as SkippedDaemonAhead.
    // knowledge_path=Some: page_watcher IS the fs writer; update_page
    //   re-projects rather than skipping the write.
    crate::post_write::page_write(
        db,
        crate::post_write::PageWrite::UpdatePreservingSources {
            page_id: &page_id,
            req,
            edited_by: "fs_edit",
            knowledge_path: Some(knowledge_path),
            basis: crate::db::BasisWrite::default(),
        },
    )
    .await?;

    Ok(Outcome::Applied { page_id })
}

/// The file of an archived page is back in the folder (put back from the
/// trash, a git branch switched back): the page is restored, takes the
/// file's text when it differs, and is recorded at that file again.
async fn bring_back_archived_page(
    db: &MemoryDB,
    knowledge_path: &Path,
    relative: &str,
    raw: &str,
    archived: crate::pages::Page,
) -> Result<(), WenlanError> {
    crate::post_write::restore_page(
        db,
        &archived.id,
        None,
        archived.version,
        &crate::read_scope::ReadScope::Global,
        None,
    )
    .await
    .map_err(|error| WenlanError::Conflict(format!("page_restore_failed: {error}")))?;
    let mut page = db
        .get_page(&archived.id)
        .await?
        .ok_or_else(|| WenlanError::Conflict("page_restore_lost".into()))?;
    let body = projected_content(raw);
    let rendered = crate::export::knowledge::render_markdown_for(&page);
    if !same_page_text_ignoring_save_noise(&body, &projected_content(&rendered)) {
        crate::post_write::page_write(
            db,
            crate::post_write::PageWrite::UpdatePreservingSources {
                page_id: &page.id,
                req: wenlan_types::requests::UpdatePageRequest {
                    content: body,
                    source_memory_ids: page.source_memory_ids.clone(),
                    expected_version: Some(page.version),
                    caller_id: None,
                    operation_id: None,
                },
                edited_by: "fs_edit",
                knowledge_path: None,
                basis: crate::db::BasisWrite::default(),
            },
        )
        .await?;
        page = db
            .get_page(&archived.id)
            .await?
            .ok_or_else(|| WenlanError::Conflict("page_restore_lost".into()))?;
    }
    crate::export::knowledge::reclaim_returned_page(knowledge_path, relative, &page)?;
    // A note the person put in the folder is theirs word for word; only a
    // file Wenlan wrote gets its stamp brought up to date.
    if crate::export::knowledge::has_origin_version(raw) {
        crate::export::knowledge::KnowledgeProjectionWrite::new(knowledge_path.to_path_buf(), db)
            .write_page_gated(db, &page)
            .await?;
    }
    log::info!(
        "[page-watcher] {relative} is back in the folder; restored page {}",
        page.id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDB;
    use crate::events::NoopEmitter;
    use crate::export::knowledge::KnowledgeWriter;
    use crate::pages::Page;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn fresh_db() -> (MemoryDB, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let emitter: Arc<dyn crate::events::EventEmitter> = Arc::new(NoopEmitter);
        let db = MemoryDB::new(&path, emitter).await.unwrap();
        (db, dir)
    }

    /// Writes a note the way a person would, saved long enough ago that the
    /// watcher takes it as settled.
    fn drop_note(dir: &Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        settle(&path);
    }

    fn settle(path: &Path) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - 2 * ADOPT_AFTER_UNCHANGED)
            .unwrap();
    }

    fn origin_id_of(path: &Path) -> String {
        let raw = std::fs::read_to_string(path).unwrap();
        obsidian::extract_frontmatter(&raw)
            .0
            .get_str("origin_id")
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn notes_put_in_the_folder_become_pages_and_keep_their_text() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let book =
            "---\ntags: [reading]\naliases: [Books]\n---\n# Dune\n\nGreat [[Arrakis]] book.\n";
        drop_note(dir.path(), "Reading/Book notes.md", book);
        drop_note(dir.path(), "plain.md", "Just text\n");
        // Left as files.
        drop_note(dir.path(), "2026-11-03.md", "\n");
        drop_note(dir.path(), "Templates/Daily.md", "# {{date}}\n");
        drop_note(dir.path(), "plain (conflicted copy).md", "Just text\n");
        drop_note(dir.path(), ".obsidian/workspace.md", "x\n");
        drop_note(dir.path(), "Drawing.excalidraw.md", "x\n");

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert_eq!(stats.adopted, 2, "{stats:?}");

        let book_path = dir.path().join("Reading/Book notes.md");
        let book_id = origin_id_of(&book_path);
        // One line added, as the first key; nothing else in the file moved.
        assert_eq!(
            std::fs::read_to_string(&book_path).unwrap(),
            format!("---\norigin_id: {book_id}\ntags: [reading]\naliases: [Books]\n---\n# Dune\n\nGreat [[Arrakis]] book.\n")
        );
        let page = db.get_page(&book_id).await.unwrap().unwrap();
        assert_eq!(page.title, "Book notes");
        assert_eq!(page.content, "# Dune\n\nGreat [[Arrakis]] book.");
        // The person's writing: the AI proposes changes, never makes them.
        assert_eq!(page.creation_kind, "authored");
        let writer = tracked_writer(&db, dir.path());
        assert_eq!(
            writer.page_filename(&book_id).unwrap(),
            "Reading/Book notes.md"
        );

        let plain_path = dir.path().join("plain.md");
        let plain_id = origin_id_of(&plain_path);
        assert_eq!(
            std::fs::read_to_string(&plain_path).unwrap(),
            format!("---\norigin_id: {plain_id}\n---\n\nJust text\n")
        );
        assert_eq!(
            db.get_page(&plain_id).await.unwrap().unwrap().content,
            "Just text"
        );
        for left in [
            "2026-11-03.md",
            "Templates/Daily.md",
            "plain (conflicted copy).md",
            "Drawing.excalidraw.md",
        ] {
            assert!(
                !std::fs::read_to_string(dir.path().join(left))
                    .unwrap()
                    .contains("origin_id"),
                "{left}"
            );
        }

        // A second pass finds nothing to do.
        let again = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(
            (again.adopted, again.applied, again.errors),
            (0, 0, 0),
            "{again:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&book_path).unwrap(),
            format!("---\norigin_id: {book_id}\ntags: [reading]\naliases: [Books]\n---\n# Dune\n\nGreat [[Arrakis]] book.\n")
        );

        // The person edits the text, and adds a key: the edit is read back,
        // and the file keeps their frontmatter.
        let edited = format!(
            "---\norigin_id: {book_id}\ntags: [reading, scifi]\naliases: [Books]\nrating: 5\n---\n# Dune\n\nGreat [[Arrakis]] book. Read it twice.\n"
        );
        std::fs::write(&book_path, &edited).unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.applied, stats.errors), (1, 0), "{stats:?}");
        let page = db.get_page(&book_id).await.unwrap().unwrap();
        assert_eq!(
            page.content,
            "# Dune\n\nGreat [[Arrakis]] book. Read it twice."
        );
        assert!(page.user_edited);
        let after = std::fs::read_to_string(&book_path).unwrap();
        assert!(
            after.starts_with(&format!(
                "---\norigin_id: {book_id}\ntags: [reading, scifi]\naliases: [Books]\nrating: 5\n---\n"
            )),
            "{after}"
        );
        assert!(!after.contains("origin_version"), "{after}");

        // Wenlan writes the page (an edit in the app): the person's
        // frontmatter stays, with the new text under it.
        let mut next = page.clone();
        next.content = "# Dune\n\nEdited in Wenlan.".into();
        next.version += 1;
        project_page(&db, &writer, &next);
        assert_eq!(
            std::fs::read_to_string(&book_path).unwrap(),
            format!("---\norigin_id: {book_id}\ntags: [reading, scifi]\naliases: [Books]\nrating: 5\n---\n\n# Dune\n\nEdited in Wenlan.\n")
        );
    }

    #[tokio::test]
    async fn a_note_still_being_saved_is_left_until_it_settles() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.md");
        std::fs::write(&path, "Typing\n").unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.adopted, 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Typing\n");
        settle(&path);
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.adopted, 1, "{stats:?}");
    }

    #[tokio::test]
    async fn an_adoption_stopped_after_the_id_line_is_finished() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        drop_note(dir.path(), "kept.md", "Text\n");
        drop_note(dir.path(), "gone.md", "Other\n");
        // Stopped after the id line was written, before the page existed.
        crate::export::knowledge::stamp_person_note(dir.path(), "kept.md", "Text\n", "page_kept")
            .unwrap();
        crate::export::knowledge::stamp_person_note(dir.path(), "gone.md", "Other\n", "page_gone")
            .unwrap();
        std::fs::remove_file(dir.path().join("gone.md")).unwrap();

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert_eq!(
            db.get_page("page_kept").await.unwrap().unwrap().content,
            "Text"
        );
        assert!(db.get_page("page_gone").await.unwrap().is_none());
        assert!(crate::export::knowledge::pending_adoptions(dir.path())
            .unwrap()
            .is_empty());
        assert_eq!(
            tracked_writer(&db, dir.path())
                .page_filename("page_kept")
                .unwrap(),
            "kept.md"
        );
    }

    #[tokio::test]
    async fn pages_can_be_renamed_once_notes_are_adopted() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        drop_note(
            dir.path(),
            "Plan.md",
            "---\ntitle: Plan\ntags: [work]\n---\nShip it.\n",
        );
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        let id = origin_id_of(&dir.path().join("Plan.md"));
        let page = db.get_page(&id).await.unwrap().unwrap();

        crate::post_write::rename_page(
            &db,
            &id,
            "Launch plan",
            page.version,
            &crate::read_scope::ReadScope::Global,
            Some(dir.path()),
        )
        .await
        .unwrap();
        let raw = std::fs::read_to_string(dir.path().join("Plan.md")).unwrap();
        assert!(raw.contains("title: \"Launch plan\""), "{raw}");
        assert!(raw.contains("tags: [work]"), "{raw}");
        assert!(raw.contains("Ship it."), "{raw}");
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.applied, stats.errors), (0, 0), "{stats:?}");
        assert_eq!(
            db.get_page(&id).await.unwrap().unwrap().title,
            "Launch plan"
        );
    }

    #[tokio::test]
    async fn a_title_changed_in_the_note_renames_its_page() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Draft.md");
        drop_note(dir.path(), "Draft.md", "---\ntitle: Draft\n---\nBody.\n");
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        let id = origin_id_of(&path);
        let raw = std::fs::read_to_string(&path)
            .unwrap()
            .replace("title: Draft", "title: Final");
        std::fs::write(&path, &raw).unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert_eq!(db.get_page(&id).await.unwrap().unwrap().title, "Final");

        // Wenlan's next write keeps the person's title.
        let mut page = db.get_page(&id).await.unwrap().unwrap();
        page.content = "Body, edited in Wenlan.".into();
        page.version += 1;
        project_page(&db, &tracked_writer(&db, dir.path()), &page);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("title: Final"), "{after}");
        assert!(after.contains("edited in Wenlan"), "{after}");
    }

    #[tokio::test]
    async fn notes_with_loose_yaml_or_a_byte_order_mark_still_sync() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        // `title: Meeting: notes` is not valid YAML; Obsidian still opens it.
        let loose = "---\ntitle: Meeting: notes\n---\nAgenda.\n";
        let bom = "\u{feff}---\ntags: [x]\n---\nWith a mark.\n";
        drop_note(dir.path(), "loose.md", loose);
        drop_note(dir.path(), "bom.md", bom);
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.adopted, stats.errors), (2, 0), "{stats:?}");

        let bom_raw = std::fs::read_to_string(dir.path().join("bom.md")).unwrap();
        assert!(bom_raw.starts_with("---\norigin_id: "), "{bom_raw:?}");
        assert!(
            bom_raw.ends_with("tags: [x]\n---\nWith a mark.\n"),
            "{bom_raw:?}"
        );
        let bom_id = origin_id_of(&dir.path().join("bom.md"));
        assert_eq!(
            db.get_page(&bom_id).await.unwrap().unwrap().content,
            "With a mark."
        );

        let path = dir.path().join("loose.md");
        let raw = std::fs::read_to_string(&path).unwrap();
        let id = crate::export::knowledge::origin_id_line(&raw).unwrap();
        std::fs::write(&path, raw.replace("Agenda.", "Agenda, then lunch.")).unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(
            (stats.applied, stats.adopted, stats.errors),
            (1, 0, 0),
            "{stats:?}"
        );
        assert_eq!(
            db.get_page(&id).await.unwrap().unwrap().content,
            "Agenda, then lunch."
        );
    }

    #[tokio::test]
    async fn a_save_that_drops_the_id_line_keeps_the_page_and_the_edit() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("open.md");
        drop_note(dir.path(), "open.md", "First draft.\n");
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        let id = origin_id_of(&path);
        // The editor had the note open from before the id line and saves.
        drop_note(dir.path(), "open.md", "First draft, finished.\n");

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.adopted, stats.errors), (0, 0), "{stats:?}");
        assert_eq!(origin_id_of(&path), id);
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.applied, stats.errors), (1, 0), "{stats:?}");
        assert_eq!(
            db.get_page(&id).await.unwrap().unwrap().content,
            "First draft, finished."
        );
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("First draft, finished."));
    }

    #[tokio::test]
    async fn a_stopped_adoption_follows_a_rename_and_undoes_an_emptied_note() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        drop_note(dir.path(), "before.md", "Moving text\n");
        drop_note(dir.path(), "emptied.md", "Soon gone\n");
        let stamp = |relative: &str, text: &str, id: &str| {
            crate::export::knowledge::stamp_person_note(dir.path(), relative, text, id).unwrap()
        };
        stamp("before.md", "Moving text\n", "page_moving");
        let stamped = stamp("emptied.md", "Soon gone\n", "page_emptied");
        std::fs::rename(dir.path().join("before.md"), dir.path().join("after.md")).unwrap();
        std::fs::write(
            dir.path().join("emptied.md"),
            stamped.replace("Soon gone\n", ""),
        )
        .unwrap();

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert_eq!(
            tracked_writer(&db, dir.path())
                .page_filename("page_moving")
                .unwrap(),
            "after.md"
        );
        assert!(db.get_page("page_emptied").await.unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("emptied.md")).unwrap(),
            ""
        );
        assert!(crate::export::knowledge::pending_adoptions(dir.path())
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn an_adopted_note_moved_to_another_folder_keeps_syncing() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        drop_note(
            dir.path(),
            "Inbox/idea.md",
            "---\ntags: [idea]\n---\nA thought.\n",
        );
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        let id = origin_id_of(&dir.path().join("Inbox/idea.md"));
        std::fs::create_dir_all(dir.path().join("Ideas")).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("Inbox/idea.md")).unwrap();
        std::fs::remove_file(dir.path().join("Inbox/idea.md")).unwrap();
        std::fs::write(
            dir.path().join("Ideas/idea.md"),
            raw.replace("A thought.", "A thought, filed."),
        )
        .unwrap();

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.applied, stats.errors), (1, 0), "{stats:?}");
        assert_eq!(
            db.get_page(&id).await.unwrap().unwrap().content,
            "A thought, filed."
        );
        assert_eq!(
            tracked_writer(&db, dir.path()).page_filename(&id).unwrap(),
            "Ideas/idea.md"
        );
    }

    fn tracked_writer(db: &MemoryDB, path: &Path) -> KnowledgeWriter {
        KnowledgeWriter::new(path.to_path_buf(), db)
    }

    fn project_page(db: &MemoryDB, writer: &KnowledgeWriter, page: &Page) -> String {
        let guard = db.begin_page_projection_write();
        writer.write_page(&guard, page).unwrap()
    }

    fn write_page_md(dir: &std::path::Path, page: &Page, body_override: Option<&str>) {
        let body = body_override.unwrap_or(&page.content);
        let domain_line = match &page.space {
            Some(d) => format!("space: {d}\n"),
            None => String::new(),
        };
        let content = format!(
            "---\ntitle: \"{}\"\n{}origin_id: {}\norigin_version: {}\ncreated: {}\nmodified: {}\n---\n\n{}\n",
            page.title,
            domain_line,
            page.id,
            page.version,
            page.created_at.chars().take(10).collect::<String>(),
            page.last_modified.chars().take(10).collect::<String>(),
            body
        );
        std::fs::write(dir.join(format!("{}.md", slug(&page.title))), content).unwrap();
    }

    fn slug(s: &str) -> String {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect()
    }

    fn sample_page(id: &str, title: &str, content: &str) -> Page {
        let now = chrono::Utc::now().to_rfc3339();
        Page {
            id: id.to_string(),
            title: title.to_string(),
            summary: None,
            content: content.to_string(),
            entity_id: None,
            space: None,
            source_memory_ids: vec!["mem_seed".to_string()],
            version: 1,
            status: "active".to_string(),
            created_at: now.clone(),
            last_compiled: now.clone(),
            last_modified: now,
            sources_updated_count: 0,
            stale_reason: None,
            pending_rebuild: None,
            refresh_blocked_reason: None,
            review_required: false,
            user_edited: false,
            relevance_score: 0.0,
            last_edited_by: None,
            last_edited_at: None,
            human_edited_at: None,
            last_delta_summary: None,
            changelog: None,
            creation_kind: "distilled".to_string(),
            review_status: "confirmed".to_string(),
            workspace: None,
            citations: Vec::new(),
            kind: "concept".to_string(),
            truth: None,
        }
    }

    #[test]
    fn projection_decoder_preserves_authored_boundaries_with_or_without_sources() {
        let bodies = [
            "\n\n# A note\n\ntext  \nnext\n\n",
            "    indented code\n    second line\n\n",
            "a final Markdown hard break  ",
            "\u{feff}\r\n  mixed-line-ending source\t \r\n\r\n",
        ];
        for body in bodies {
            for sources in [vec![], vec!["mem_seed".to_string()]] {
                let mut page = sample_page("page_exact", "Exact", body);
                page.source_memory_ids = sources;
                let projected = crate::export::knowledge::render_markdown_for(&page);
                assert_eq!(projected_content(&projected), body);
            }
        }
    }

    #[tokio::test]
    async fn published_body_survives_projection_readback_exactly() {
        use crate::pages::PageDraftPublishOutcome;
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        for (index, body) in [
            "\n\n# A note\n\ntext  \nnext\n\n",
            "    indented code\n    second line\n\n",
            "a final Markdown hard break  ",
            "\u{feff}\r\n  mixed-line-ending source\t \r\n\r\n",
        ]
        .into_iter()
        .enumerate()
        {
            let title = format!("  Exact publish {index}  ");
            let draft = db
                .create_page_draft(&title, body, None, None)
                .await
                .unwrap();
            let PageDraftPublishOutcome::Published(published) = db
                .publish_page_draft(&draft.id, draft.version)
                .await
                .unwrap()
            else {
                panic!("draft must publish");
            };
            assert_eq!(published.title, title.trim());
            assert_eq!(published.content, body);
            let filename = project_page(&db, &writer, &published);
            // This is the same read-back invoked when GET /api/pages/{id}
            // opens a newly published Page, not just a direct DB assertion.
            assert!(
                !sync_page_file_if_changed(&db, knowledge_dir.path(), &published.id)
                    .await
                    .unwrap()
            );
            let after = db.get_page(&published.id).await.unwrap().unwrap();
            assert_eq!(after.content, body);
            assert_eq!(
                after.version, published.version,
                "fresh projection must not create an fs_edit"
            );
            assert_eq!(
                std::fs::read_to_string(knowledge_dir.path().join(filename)).unwrap(),
                crate::export::knowledge::render_markdown_for(&published)
            );
        }
    }

    #[tokio::test]
    async fn external_prose_edit_preserves_authored_boundaries_and_remains_stable() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        let before = "\n\n    original code\n\nLast line  \n\n";
        let edited = "\n\n    changed code\n\nLast line  \n\n\n";
        for (index, source_ids) in [vec![], vec!["mem_seed"]].into_iter().enumerate() {
            let id = format!("page_external_exact_{index}");
            db.insert_page(
                &id,
                &format!("External exact {index}"),
                None,
                before,
                None,
                None,
                &source_ids,
                &chrono::Utc::now().to_rfc3339(),
            )
            .await
            .unwrap();
            let page = db.get_page(&id).await.unwrap().unwrap();
            project_page(&db, &writer, &page);
            let filename = writer.page_filename(&id).expect("projected relative path");
            let path = knowledge_dir.path().join(&filename);
            let projected = std::fs::read_to_string(&path).unwrap();
            assert!(projected.contains(before));
            std::fs::write(&path, projected.replacen(before, edited, 1)).unwrap();
            let stats = sync_filesystem_edits_in(
                &db,
                knowledge_dir.path(),
                std::slice::from_ref(&filename),
            )
            .await
            .unwrap();
            assert_eq!(
                stats.applied, 1,
                "external authored edit must be applied: {stats:?}"
            );
            let after = db.get_page(&id).await.unwrap().unwrap();
            assert_eq!(after.content, edited);
            assert_eq!(after.source_memory_ids, page.source_memory_ids);
            assert_eq!(after.version, page.version + 1);
            let stats = sync_filesystem_edits_in(&db, knowledge_dir.path(), &[filename])
                .await
                .unwrap();
            assert_eq!(stats.applied, 0, "reprojection must stabilize: {stats:?}");
            assert_eq!(db.get_page(&id).await.unwrap().unwrap().content, edited);
        }
    }

    async fn insert_projected_page(
        db: &MemoryDB,
        writer: &KnowledgeWriter,
        id: &str,
        body: &str,
        source_ids: &[&str],
    ) -> (Page, String) {
        db.insert_page(
            id,
            &format!("Title {id}"),
            None,
            body,
            None,
            None,
            source_ids,
            &chrono::Utc::now().to_rfc3339(),
        )
        .await
        .unwrap();
        let page = db.get_page(id).await.unwrap().unwrap();
        project_page(db, writer, &page);
        let filename = writer.page_filename(id).expect("projected relative path");
        (page, filename)
    }

    /// What an editor does when it rewrites every line ending of a file.
    fn to_crlf(text: &str) -> String {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    }

    /// Editors change line endings and end-of-file whitespace on save without
    /// the author typing anything. None of that may version the page, flip
    /// `user_edited`, or lock it for review; the file goes back to the exact
    /// bytes Wenlan writes, so the writer's recorded digest matches again.
    #[tokio::test]
    async fn editor_save_noise_is_not_a_human_edit() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        let bodies = [
            "\n\n    indented code\n\nLast line  \n\n",
            "a final Markdown hard break  ",
            "line one\r\nline two with a CRLF the author typed\r\n",
        ];
        type Noise = (&'static str, fn(&str) -> String);
        let noises: [Noise; 6] = [
            ("appended final newline", |file| format!("{file}\n")),
            ("two appended newlines", |file| format!("{file}\n\n")),
            ("final newline removed", |file| {
                file.trim_end_matches('\n').to_string()
            }),
            ("whitespace at end of file trimmed", |file| {
                format!("{}\n", file.trim_end())
            }),
            ("whole file converted to CRLF", to_crlf),
            ("CRLF plus appended newline", |file| {
                to_crlf(&format!("{file}\n"))
            }),
        ];
        let mut case = 0;
        for body in bodies {
            for source_ids in [vec![], vec!["mem_seed"]] {
                for (noise, apply) in noises {
                    case += 1;
                    let id = format!("page_noise_{case}");
                    let (page, filename) =
                        insert_projected_page(&db, &writer, &id, body, &source_ids).await;
                    let path = knowledge_dir.path().join(&filename);
                    let projected = std::fs::read_to_string(&path).unwrap();
                    std::fs::write(&path, apply(&projected)).unwrap();
                    let stats = sync_filesystem_edits_in(
                        &db,
                        knowledge_dir.path(),
                        std::slice::from_ref(&filename),
                    )
                    .await
                    .unwrap();
                    let context = format!("{noise} on {body:?} with sources {source_ids:?}");
                    assert_eq!(
                        (stats.applied, stats.skipped_unchanged, stats.errors),
                        (0, 1, 0),
                        "{context}: {stats:?}"
                    );
                    let after = db.get_page(&id).await.unwrap().unwrap();
                    assert_eq!(after.content, body, "{context}");
                    assert_eq!(after.version, page.version, "{context}");
                    assert!(!after.user_edited, "{context}");
                    assert!(!after.review_required, "{context}: page was locked");
                    assert_eq!(
                        std::fs::read_to_string(&path).unwrap(),
                        projected,
                        "{context}: file must be re-projected to Wenlan's bytes"
                    );
                }
            }
        }
    }

    /// A file an editor saved with CRLF (or that Git checked out with
    /// `autocrlf`) still carries real edits. They are applied once, read back
    /// with LF because the CRLF is the editor's and not the author's, and the
    /// writer accepts the CRLF file as holding the new text, so the file gets
    /// the new version stamp instead of trailing the DB.
    #[tokio::test]
    async fn crlf_file_with_a_real_edit_is_applied_once_and_reprojected() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        let before = "\n\n    original code\n\nLast line  \n";
        let edited = "\n\n    changed code\n\nLast line  \n";
        for (index, source_ids) in [vec![], vec!["mem_seed"]].into_iter().enumerate() {
            let id = format!("page_crlf_edit_{index}");
            let (page, filename) =
                insert_projected_page(&db, &writer, &id, before, &source_ids).await;
            let path = knowledge_dir.path().join(&filename);
            let projected = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, to_crlf(&projected.replacen(before, edited, 1))).unwrap();
            let changed = std::slice::from_ref(&filename);
            let stats = sync_filesystem_edits_in(&db, knowledge_dir.path(), changed)
                .await
                .unwrap();
            assert_eq!(stats.applied, 1, "{stats:?}");
            let after = db.get_page(&id).await.unwrap().unwrap();
            assert_eq!(after.content, edited);
            assert_eq!(after.version, page.version + 1);
            assert!(after.user_edited);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                crate::export::knowledge::render_markdown_for(&after),
                "the edited file must be re-projected with the new version"
            );
            let stats = sync_filesystem_edits_in(&db, knowledge_dir.path(), changed)
                .await
                .unwrap();
            assert_eq!(
                (stats.applied, stats.skipped_unchanged),
                (0, 1),
                "{stats:?}"
            );
        }
    }

    /// The other side of the save-noise rule: whitespace that changes what
    /// the Markdown renders is the author's edit and is kept byte for byte,
    /// including the first-human-edit lock.
    #[tokio::test]
    async fn whitespace_edits_that_change_the_markdown_are_human_edits() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        let before = "intro\n\nfirst\nsecond\n";
        let edits = [
            (
                "indented into a code block",
                "intro\n\n    first\n    second\n",
            ),
            ("leading blank lines", "\n\nintro\n\nfirst\nsecond\n"),
            ("hard line break", "intro\n\nfirst  \nsecond\n"),
            ("paragraph split", "intro\n\nfirst\n\nsecond\n"),
            ("tab indentation", "intro\n\n\tfirst\nsecond\n"),
        ];
        for (index, (edit, edited)) in edits.into_iter().enumerate() {
            for source_ids in [vec![], vec!["mem_seed"]] {
                let id = format!("page_ws_edit_{index}_{}", source_ids.len());
                let (page, filename) =
                    insert_projected_page(&db, &writer, &id, before, &source_ids).await;
                let path = knowledge_dir.path().join(&filename);
                let projected = std::fs::read_to_string(&path).unwrap();
                std::fs::write(&path, projected.replacen(before, edited, 1)).unwrap();
                let stats = sync_filesystem_edits_in(
                    &db,
                    knowledge_dir.path(),
                    std::slice::from_ref(&filename),
                )
                .await
                .unwrap();
                assert_eq!(stats.applied, 1, "{edit}: {stats:?}");
                let after = db.get_page(&id).await.unwrap().unwrap();
                assert_eq!(after.content, edited, "{edit}");
                assert_eq!(after.version, page.version + 1, "{edit}");
                assert!(after.user_edited, "{edit}");
                assert!(
                    !after.review_required,
                    "{edit}: a human edit protects its sentences, it does not lock the page"
                );
            }
        }
    }

    /// After save noise the file is not byte-for-byte Wenlan's, and the writer
    /// refuses to replace a file holding an external edit. It must still
    /// accept this one (same save-noise rule), or the file would trail the DB
    /// version and the watcher would skip every later edit to it.
    #[tokio::test]
    async fn daemon_write_after_save_noise_still_reaches_the_file() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        for (index, source_ids) in [vec![], vec!["mem_seed"]].into_iter().enumerate() {
            let id = format!("page_noise_then_write_{index}");
            let (_, filename) =
                insert_projected_page(&db, &writer, &id, "body text\n", &source_ids).await;
            let path = knowledge_dir.path().join(&filename);
            let projected = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, to_crlf(&format!("{projected}\n"))).unwrap();
            sync_filesystem_edits_in(&db, knowledge_dir.path(), std::slice::from_ref(&filename))
                .await
                .unwrap();
            let req = wenlan_types::requests::UpdatePageRequest {
                content: "rewritten in the app".to_string(),
                source_memory_ids: Vec::new(),
                expected_version: None,
                caller_id: None,
                operation_id: None,
            };
            crate::post_write::update_page_preserving_sources(
                &db,
                &id,
                req,
                "manual_edit",
                Some(knowledge_dir.path()),
            )
            .await
            .unwrap();
            let after = db.get_page(&id).await.unwrap().unwrap();
            assert_eq!(after.content, "rewritten in the app");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                crate::export::knowledge::render_markdown_for(&after)
            );
        }
    }
    #[tokio::test]
    async fn applies_user_edit_when_md_body_differs() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_a", "Topic A", "original body line");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();

        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);
        write_page_md(knowledge_dir.path(), &page, Some("user-edited body line"));
        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(stats.applied, 1);

        let p = db.get_page("page_a").await.unwrap().unwrap();
        assert_eq!(p.content, "user-edited body line");
        // fs_edit must flip user_edited so refinery escalates instead of
        // overwriting on the next re-distill.
        assert!(p.user_edited);
    }

    #[tokio::test]
    async fn nested_fs_edit_updates_the_same_page_without_recreating_a_flat_copy() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let page = sample_page("page_nested", "Nested Topic", "original nested body");
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &page.created_at,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, dir.path());
        project_page(&db, &writer, &page);
        let source = writer.page_filename(&page.id).unwrap();
        crate::export::knowledge::create_knowledge_folder(dir.path(), "", "Research").unwrap();
        crate::export::knowledge::create_knowledge_folder(dir.path(), "Research", "Rust").unwrap();
        let path = crate::export::knowledge::move_projected_page(
            dir.path(),
            &page.id,
            &source,
            "Research/Rust",
            "watcher-move",
        )
        .unwrap();
        let raw = std::fs::read_to_string(dir.path().join(&path))
            .unwrap()
            .replace("original nested body", "externally edited nested body");
        std::fs::write(dir.path().join(&path), raw).unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.applied, 1);
        let updated = db.get_page(&page.id).await.unwrap().unwrap();
        assert_eq!(updated.content, "externally edited nested body");
        assert!(updated.user_edited);
        assert_eq!(updated.space, page.space);
        assert_eq!(writer.page_filename(&page.id).unwrap(), path);
        assert!(!dir.path().join(&source).exists());
        assert_eq!(updated.source_memory_ids, page.source_memory_ids);
    }

    async fn projected_external_page(
        db: &MemoryDB,
        dir: &TempDir,
    ) -> (Page, KnowledgeWriter, String) {
        let page = sample_page("page_external", "External Topic", "original body");
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &page.created_at,
        )
        .await
        .unwrap();
        let writer = tracked_writer(db, dir.path());
        project_page(db, &writer, &page);
        let source = writer.page_filename(&page.id).unwrap();
        (page, writer, source)
    }

    #[tokio::test]
    async fn a_page_moved_and_renamed_outside_wenlan_keeps_its_identity_and_edit() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let (page, writer, source) = projected_external_page(&db, &dir).await;
        crate::export::knowledge::create_knowledge_folder(dir.path(), "", "Notes").unwrap();
        let moved = dir.path().join("Notes/renamed by me.md");
        std::fs::rename(dir.path().join(&source), &moved).unwrap();
        let raw = std::fs::read_to_string(&moved)
            .unwrap()
            .replace("original body", "an edit made with the move");
        std::fs::write(&moved, &raw).unwrap();

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert_eq!(stats.applied, 1, "{stats:?}");
        let updated = db.get_page(&page.id).await.unwrap().unwrap();
        assert_eq!(updated.content, "an edit made with the move");
        assert!(updated.user_edited);
        // The record follows the file; nothing is written back at the old
        // name and no copy appears.
        assert_eq!(
            writer.page_filename(&page.id).unwrap(),
            "Notes/renamed by me.md"
        );
        assert!(!dir.path().join(&source).exists());
        assert_eq!(
            crate::export::knowledge::knowledge_markdown_paths(dir.path())
                .unwrap()
                .into_iter()
                .filter(|path| path.ends_with(".md") && !path.ends_with("index.md"))
                .collect::<Vec<_>>(),
            ["Notes/renamed by me.md"]
        );

        // A later Wenlan write keeps the person's name and folder.
        let mut next = updated.clone();
        next.content = "written by Wenlan".into();
        next.version += 1;
        project_page(&db, &writer, &next);
        assert_eq!(
            writer.page_filename(&page.id).unwrap(),
            "Notes/renamed by me.md"
        );
        assert!(std::fs::read_to_string(&moved)
            .unwrap()
            .contains("written by Wenlan"));
        assert!(!dir.path().join(&source).exists());
    }

    #[tokio::test]
    async fn a_moved_folder_brings_its_pages_along() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        crate::export::knowledge::create_knowledge_folder(dir.path(), "", "Research").unwrap();
        let page = sample_page("page_in_folder", "Folder Topic", "body");
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &page.created_at,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, dir.path());
        let stored = db.get_page(&page.id).await.unwrap().unwrap();
        project_page(&db, &writer, &stored);
        let first = writer.page_filename(&page.id).unwrap();
        let source = crate::export::knowledge::move_projected_page(
            dir.path(),
            &page.id,
            &first,
            "Research",
            "watcher-folder",
        )
        .unwrap();
        assert!(source.starts_with("Research/"), "{source}");
        std::fs::rename(dir.path().join("Research"), dir.path().join("Reading")).unwrap();

        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        assert_eq!(
            writer.page_filename(&page.id).unwrap(),
            source.replacen("Research/", "Reading/", 1)
        );
        assert_eq!(
            db.get_page(&page.id).await.unwrap().unwrap().content,
            "body"
        );
    }

    #[tokio::test]
    async fn a_copy_of_a_page_cannot_update_or_duplicate_it() {
        for original_kept in [true, false] {
            let (db, _ddir) = fresh_db().await;
            let dir = TempDir::new().unwrap();
            let (page, writer, source) = projected_external_page(&db, &dir).await;
            crate::export::knowledge::create_knowledge_folder(dir.path(), "", "Notes").unwrap();
            let copy = dir.path().join("Notes/external.md");
            std::fs::copy(dir.path().join(&source), &copy).unwrap();
            let second = dir.path().join("Notes/external 2.md");
            if !original_kept {
                // Moved, and copied too: two files claim the page.
                std::fs::rename(dir.path().join(&source), &second).unwrap();
            }
            let raw = std::fs::read_to_string(&copy)
                .unwrap()
                .replace("original body", "external edit that must be preserved");
            std::fs::write(&copy, &raw).unwrap();
            let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
            assert_eq!(stats.applied, 0);
            assert!(stats.errors > 0);
            assert_eq!(
                db.get_page(&page.id).await.unwrap().unwrap().content,
                page.content
            );
            assert_eq!(std::fs::read_to_string(&copy).unwrap(), raw);
            assert_eq!(dir.path().join(&source).exists(), original_kept);
            assert_eq!(writer.page_filename(&page.id).unwrap(), source);
        }
    }

    /// Pretend every page file now gone went missing long ago.
    fn backdate_missing(dir: &Path) {
        let path = dir.join(".wenlan/state.json");
        let mut state: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if let Some(missing) = state
            .get_mut("missing_since")
            .and_then(serde_json::Value::as_object_mut)
        {
            for since in missing.values_mut() {
                let earlier =
                    since.as_i64().unwrap() - 2 * crate::export::knowledge::MISSING_SETTLE_SECS;
                *since = earlier.into();
            }
        }
        std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_page_deleted_in_the_folder_is_archived_and_can_come_back() {
        for whole_folder in [false, true] {
            let (db, _ddir) = fresh_db().await;
            let dir = TempDir::new().unwrap();
            let (page, writer, mut source) = projected_external_page(&db, &dir).await;
            if whole_folder {
                crate::export::knowledge::create_knowledge_folder(dir.path(), "", "Old").unwrap();
                source = crate::export::knowledge::move_projected_page(
                    dir.path(),
                    &page.id,
                    &source,
                    "Old",
                    "delete-folder",
                )
                .unwrap();
                std::fs::remove_dir_all(dir.path().join("Old")).unwrap();
            } else {
                std::fs::remove_file(dir.path().join(&source)).unwrap();
            }

            // Just gone: it may be a save that replaces the file.
            let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
            assert_eq!(stats.archived_deleted, 0, "{stats:?}");
            let stored = db.get_page(&page.id).await.unwrap().unwrap();
            assert_eq!(stored.status, "active");

            backdate_missing(dir.path());
            let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
            assert_eq!(stats.archived_deleted, 1, "{whole_folder} {stats:?}");
            assert_eq!(stats.errors, 0, "{stats:?}");
            let archived = db.get_page(&page.id).await.unwrap().unwrap();
            assert_eq!(archived.status, "archived");
            assert_eq!(archived.content, page.content, "the text is kept");
            assert_eq!(writer.page_filename(&page.id), None);
            assert!(!dir.path().join(&source).exists());

            // Restoring it in the app writes the file back where it was.
            crate::post_write::restore_page(
                &db,
                &page.id,
                None,
                archived.version,
                &crate::read_scope::ReadScope::Global,
                Some(dir.path()),
            )
            .await
            .unwrap();
            let restored = db.get_page(&page.id).await.unwrap().unwrap();
            crate::export::knowledge::KnowledgeProjectionWrite::new(dir.path().to_path_buf(), &db)
                .write_page_gated(&db, &restored)
                .await
                .unwrap()
                .unwrap();
            // Back where it was; a deleted folder is not made again.
            let back = if whole_folder {
                source.trim_start_matches("Old/").to_string()
            } else {
                source.clone()
            };
            assert_eq!(writer.page_filename(&page.id).unwrap(), back);
            assert!(std::fs::read_to_string(dir.path().join(&back))
                .unwrap()
                .contains("original body"));
        }
    }

    #[tokio::test]
    async fn a_file_back_within_the_wait_keeps_its_page() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let (page, _writer, source) = projected_external_page(&db, &dir).await;
        let path = dir.path().join(&source);
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        std::fs::write(&path, &raw).unwrap();
        sync_filesystem_edits(&db, dir.path()).await.unwrap();

        // Gone again later: the wait starts over.
        std::fs::remove_file(&path).unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.archived_deleted, 0, "{stats:?}");
        assert_eq!(
            db.get_page(&page.id).await.unwrap().unwrap().status,
            "active"
        );
    }

    #[tokio::test]
    async fn a_file_put_back_after_its_page_was_archived_brings_the_page_back() {
        for edited in [false, true] {
            let (db, _ddir) = fresh_db().await;
            let dir = TempDir::new().unwrap();
            let (page, writer, source) = projected_external_page(&db, &dir).await;
            let path = dir.path().join(&source);
            let raw = std::fs::read_to_string(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            sync_filesystem_edits(&db, dir.path()).await.unwrap();
            backdate_missing(dir.path());
            sync_filesystem_edits(&db, dir.path()).await.unwrap();
            assert_eq!(
                db.get_page(&page.id).await.unwrap().unwrap().status,
                "archived"
            );

            // Put back from the trash, under another name, edited or not.
            let back = dir.path().join("put back.md");
            let text = if edited {
                raw.replace("original body", "edited while in the trash")
            } else {
                raw.clone()
            };
            std::fs::write(&back, &text).unwrap();
            let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
            assert_eq!((stats.applied, stats.errors), (1, 0), "{edited} {stats:?}");
            let restored = db.get_page(&page.id).await.unwrap().unwrap();
            assert_eq!(restored.status, "active");
            let wanted = if edited {
                "edited while in the trash"
            } else {
                "original body"
            };
            assert_eq!(restored.content, wanted);
            assert_eq!(writer.page_filename(&page.id).unwrap(), "put back.md");
            assert!(std::fs::read_to_string(&back).unwrap().contains(wanted));
            assert!(!path.exists(), "no second copy");

            let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
            assert_eq!((stats.applied, stats.errors), (0, 0), "{stats:?}");
        }
    }

    /// A note the person put in the folder and later deleted stays deleted:
    /// startup does not write it back, its page is archived, and putting the
    /// note back brings the page back with the note's own text and keys.
    #[tokio::test]
    async fn a_deleted_adopted_note_stays_deleted_and_can_come_back() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        drop_note(
            dir.path(),
            "Trip.md",
            "---\ntags: [travel]\n---\nPack light.\n",
        );
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.adopted, 1, "{stats:?}");
        let path = dir.path().join("Trip.md");
        let id = origin_id_of(&path);
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let writer = tracked_writer(&db, dir.path());
        let page = db.get_page(&id).await.unwrap().unwrap();
        let reconciled = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();
        assert_eq!((reconciled.rewritten, reconciled.missing), (0, 1));
        assert!(!path.exists(), "a deleted note came back");

        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        backdate_missing(dir.path());
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((stats.archived_deleted, stats.errors), (1, 0), "{stats:?}");
        assert_eq!(db.get_page(&id).await.unwrap().unwrap().status, "archived");
        assert!(!path.exists());

        std::fs::write(&path, &raw).unwrap();
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.errors, 0, "{stats:?}");
        let back = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(back.status, "active");
        assert_eq!(back.content, "Pack light.");
        assert_eq!(writer.page_filename(&id).unwrap(), "Trip.md");
        // Still the person's note, word for word, keys included.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        let again = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!((again.applied, again.errors), (0, 0), "{again:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[tokio::test]
    async fn a_hold_that_lifts_starts_the_wait_again() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, dir.path());
        let count = crate::export::knowledge::MASS_MISSING_FLOOR + 2;
        let mut files = Vec::new();
        for n in 0..count {
            let page = sample_page(&format!("page_back_{n}"), &format!("Back {n}"), "body");
            db.insert_page(
                &page.id,
                &page.title,
                None,
                &page.content,
                None,
                None,
                &["mem_seed"],
                &page.created_at,
            )
            .await
            .unwrap();
            project_page(&db, &writer, &page);
            let path = dir.path().join(writer.page_filename(&page.id).unwrap());
            files.push((page.id, path.clone(), std::fs::read(&path).unwrap()));
            std::fs::remove_file(&path).unwrap();
        }
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert!(stats.held_missing.is_some(), "{stats:?}");
        backdate_missing(dir.path());
        // Most come back; the rest wait again instead of archiving at once.
        for (_, path, bytes) in files.iter().skip(2) {
            std::fs::write(path, bytes).unwrap();
        }
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(
            (stats.held_missing, stats.archived_deleted),
            (None, 0),
            "{stats:?}"
        );
        for (id, _, _) in &files {
            assert_eq!(db.get_page(id).await.unwrap().unwrap().status, "active");
        }
    }

    #[tokio::test]
    async fn a_moved_page_with_long_frontmatter_is_not_archived() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let (page, writer, source) = projected_external_page(&db, &dir).await;
        let raw = std::fs::read_to_string(dir.path().join(&source)).unwrap();
        let long = format!("---\nnotes: \"{}\"\n", "x".repeat(9000));
        std::fs::remove_file(dir.path().join(&source)).unwrap();
        std::fs::write(
            dir.path().join("renamed.md"),
            raw.replacen("---\n", &long, 1),
        )
        .unwrap();
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        backdate_missing(dir.path());
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.archived_deleted, 0, "{stats:?}");
        assert_eq!(
            db.get_page(&page.id).await.unwrap().unwrap().status,
            "active"
        );
        assert_eq!(writer.page_filename(&page.id).unwrap(), "renamed.md");
    }

    #[tokio::test]
    async fn an_archive_fenced_by_version_leaves_a_page_written_since() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let (page, _writer, _source) = projected_external_page(&db, &dir).await;
        let seen = db.get_page(&page.id).await.unwrap().unwrap().version;
        assert_eq!(
            db.archive_page_at_version(&page.id, seen - 1)
                .await
                .unwrap(),
            None
        );
        assert!(db
            .archive_page_at_version(&page.id, seen)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn many_page_files_gone_at_once_archive_none() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let writer = tracked_writer(&db, dir.path());
        let count = crate::export::knowledge::MASS_MISSING_FLOOR + 2;
        let mut ids = Vec::new();
        for n in 0..count {
            let page = sample_page(&format!("page_mass_{n}"), &format!("Mass {n}"), "body");
            db.insert_page(
                &page.id,
                &page.title,
                None,
                &page.content,
                None,
                None,
                &["mem_seed"],
                &page.created_at,
            )
            .await
            .unwrap();
            project_page(&db, &writer, &page);
            std::fs::remove_file(dir.path().join(writer.page_filename(&page.id).unwrap())).unwrap();
            ids.push(page.id);
        }
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        backdate_missing(dir.path());
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.held_missing, Some(count), "{stats:?}");
        assert_eq!(stats.archived_deleted, 0);
        for id in ids {
            assert_eq!(db.get_page(&id).await.unwrap().unwrap().status, "active");
        }
    }

    #[tokio::test]
    async fn a_page_offloaded_by_icloud_is_not_deleted() {
        let (db, _ddir) = fresh_db().await;
        let dir = TempDir::new().unwrap();
        let (page, _writer, source) = projected_external_page(&db, &dir).await;
        std::fs::remove_file(dir.path().join(&source)).unwrap();
        std::fs::write(dir.path().join(format!(".{source}.icloud")), b"placeholder").unwrap();
        sync_filesystem_edits(&db, dir.path()).await.unwrap();
        backdate_missing(dir.path());
        let stats = sync_filesystem_edits(&db, dir.path()).await.unwrap();
        assert_eq!(stats.archived_deleted, 0, "{stats:?}");
        assert_eq!(
            db.get_page(&page.id).await.unwrap().unwrap().status,
            "active"
        );
    }

    /// Change D (2026-08-24 doc-citation-locator fix): the watcher's sync
    /// used to snapshot `existing.source_memory_ids` and hand it to
    /// `update_page` with `preserve_sources=false` -- the CAS loop re-read
    /// content fresh but wrote the STALE source list, silently reverting a
    /// source attached via API in that window (observed live during the
    /// demo build). `sync_one_file` has no await between its `existing`
    /// snapshot and the write in the content-differs branch, so a
    /// deterministic differential through the real polling path isn't
    /// reachable without a test-only pause hook -- this tests at the update
    /// seam instead (spec's own words): a manually-stale request, the exact
    /// shape `sync_one_file` builds, sent through the fix's call
    /// (`update_page_preserving_sources`) with `edited_by = "fs_edit"`,
    /// after a source was attached to represent the race window.
    #[tokio::test]
    async fn fs_edit_write_preserves_a_source_attached_after_the_snapshot() {
        let (db, _ddir) = fresh_db().await;
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_race",
            "Topic Race",
            None,
            "original body",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();

        // A source attached via API after the watcher would have snapshotted
        // `existing.source_memory_ids`, before the CAS-safe write commits.
        db.link_page_source("page_race", "mem_added_mid_race", "concurrent_attach")
            .await
            .unwrap();

        // The exact request shape sync_one_file builds: content read from
        // disk, and the (now stale) source snapshot read before the
        // concurrent attach above.
        let req = wenlan_types::requests::UpdatePageRequest {
            content: "user-edited body from disk".to_string(),
            source_memory_ids: vec!["mem_seed".to_string()],
            expected_version: None,
            caller_id: None,
            operation_id: None,
        };
        crate::post_write::update_page_preserving_sources(&db, "page_race", req, "fs_edit", None)
            .await
            .unwrap();

        let page = db.get_page("page_race").await.unwrap().unwrap();
        let mut sources = page.source_memory_ids.clone();
        sources.sort();
        assert_eq!(
            sources,
            vec!["mem_added_mid_race".to_string(), "mem_seed".to_string()],
            "a source attached between the watcher's snapshot and its write must survive: {:?}",
            page.source_memory_ids
        );
        assert!(
            page.user_edited,
            "fs_edit must still flip user_edited even on the CAS-safe path"
        );
    }

    #[tokio::test]
    async fn no_op_when_md_matches_db() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_b", "Topic B", "matching content");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        write_page_md(knowledge_dir.path(), &page, None);

        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.skipped_unchanged, 1);
    }

    #[tokio::test]
    async fn skips_md_with_no_origin_id() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        // User dropped a plain md in the page dir with no origin_id.
        std::fs::write(
            knowledge_dir.path().join("freeform.md"),
            "---\ntitle: \"Random\"\n---\n\njust a note",
        )
        .unwrap();

        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.skipped_no_origin_id, 1);
    }

    #[tokio::test]
    async fn skips_md_pointing_at_unknown_page_id() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let ghost = sample_page("page_ghost", "Ghost", "ghost body");
        // DB never inserted page_ghost — md is dangling.
        write_page_md(knowledge_dir.path(), &ghost, None);

        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.skipped_unknown_page, 1);
    }

    #[tokio::test]
    async fn skips_when_md_version_trails_db() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_c",
            "Topic C",
            None,
            "daemon body v3",
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        // Daemon bumped version to 3 via two updates without re-projecting md.
        db.update_page_content("page_c", "daemon body v2", &["mem_seed"], "re_distill")
            .await
            .unwrap();
        db.update_page_content("page_c", "daemon body v3", &["mem_seed"], "re_distill")
            .await
            .unwrap();

        // md frontmatter stamps origin_version: 1 — the projection from
        // the original insert. md body differs from DB.
        let mut stale = sample_page("page_c", "Topic C", "old body from disk");
        stale.version = 1;
        write_page_md(knowledge_dir.path(), &stale, None);

        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        // Daemon ahead — don't roll DB back to the disk's old body.
        assert_eq!(stats.applied, 0);
        assert_eq!(stats.skipped_daemon_ahead, 1);
        let p = db.get_page("page_c").await.unwrap().unwrap();
        assert_eq!(p.content, "daemon body v3");
        assert!(!p.user_edited);
    }

    #[tokio::test]
    async fn applies_consecutive_user_edits() {
        // Regression: after apply, md must be re-projected so the frontmatter
        // origin_version stays in sync with the DB. Otherwise the second
        // edit lands as SkippedDaemonAhead.
        //
        // Seed the page via KnowledgeWriter so state.json + filename + DB
        // all line up the way they would in production. Subsequent edits
        // rewrite that same file in place.
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_x", "Topic X", "original body");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);
        let md_path = knowledge_dir
            .path()
            .join(writer.page_filename(&page.id).expect("state has file"));

        // Edit #1 — replace body.
        let projected = std::fs::read_to_string(&md_path).unwrap();
        let header_end = projected.find("\n---\n\n").expect("frontmatter present");
        let head = &projected[..header_end + "\n---\n\n".len()];
        std::fs::write(&md_path, format!("{head}edit one\n")).unwrap();
        let s1 = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(s1.applied, 1, "edit one must apply; got {:?}", s1);

        // Edit #2 — same path, new body. The watcher's re-projection
        // bumped origin_version, so a fresh read picks up the new header.
        let projected = std::fs::read_to_string(&md_path).unwrap();
        let header_end = projected.find("\n---\n\n").expect("frontmatter present");
        let head = &projected[..header_end + "\n---\n\n".len()];
        std::fs::write(&md_path, format!("{head}edit two — second pass\n")).unwrap();
        let s2 = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(s2.applied, 1, "edit two must apply; got {:?}", s2);
        let p = db.get_page("page_x").await.unwrap().unwrap();
        assert_eq!(p.content, "edit two — second pass");
    }

    #[tokio::test]
    async fn reads_back_one_open_page_only_when_its_file_was_saved() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_open", "Open page", "every Friday");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);
        let md_path = knowledge_dir
            .path()
            .join(writer.page_filename(&page.id).expect("state has file"));

        // First look records the file; nothing to apply.
        assert!(
            !sync_page_file_if_changed(&db, knowledge_dir.path(), &page.id)
                .await
                .unwrap()
        );

        let projected = std::fs::read_to_string(&md_path).unwrap();
        let header_end = projected.find("\n---\n\n").expect("frontmatter present");
        let head = &projected[..header_end + "\n---\n\n".len()];
        std::fs::write(&md_path, format!("{head}every Thursday\n")).unwrap();
        let file = std::fs::File::options().write(true).open(&md_path).unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();

        assert!(
            sync_page_file_if_changed(&db, knowledge_dir.path(), &page.id)
                .await
                .unwrap()
        );
        assert_eq!(
            db.get_page("page_open").await.unwrap().unwrap().content,
            "every Thursday"
        );
        // An unknown page and an unchanged file are both no-ops.
        assert!(
            !sync_page_file_if_changed(&db, knowledge_dir.path(), "page_missing")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn nonexistent_knowledge_path_is_a_noop() {
        let (db, _ddir) = fresh_db().await;
        let missing = std::path::PathBuf::from("/tmp/origin-page-watcher-nonexistent");
        let stats = sync_filesystem_edits(&db, &missing).await.unwrap();
        assert_eq!(stats, WatcherStats::default());
    }

    #[tokio::test]
    async fn freshly_projected_page_is_unchanged_not_user_edited() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_fresh", "Fresh Topic", "## Overview\nbody prose here");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_seed"],
            &now,
        )
        .await
        .unwrap();
        // Project via the live writer — the .md now carries the delimiter
        // Sources block + sources: frontmatter that Page.content lacks.
        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);

        // First watcher tick must see the projection as Unchanged, NOT a
        // user edit — otherwise the refinery is locked out of every page.
        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(
            stats.applied, 0,
            "fresh projection must not apply; got {:?}",
            stats
        );
        // The projection writer also emits the reserved OKF `index.md`,
        // which the watcher scans and skips (no `origin_id`).
        assert_eq!(
            stats.scanned, 2,
            "page plus reserved index.md should be scanned; got {:?}",
            stats
        );
        assert_eq!(
            stats.skipped_no_origin_id, 1,
            "reserved index.md must be skipped, not applied; got {:?}",
            stats
        );
        assert_eq!(
            stats.skipped_unchanged, 1,
            "fresh projection must be skipped_unchanged; got {:?}",
            stats
        );
        let p = db.get_page("page_fresh").await.unwrap().unwrap();
        assert!(!p.user_edited);
    }

    #[tokio::test]
    async fn genuine_prose_edit_with_block_present_applies_block_free() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_edit", "Edit Topic", "## Overview\noriginal prose");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            &page.id,
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_e"],
            &now,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        let fname = project_page(&db, &writer, &page);
        let md_path = knowledge_dir.path().join(&fname);

        // The projected file has the Sources block. User edits the PROSE only,
        // leaving the block intact.
        let projected = std::fs::read_to_string(&md_path).unwrap();
        let edited = projected.replace("original prose", "edited prose by the user");
        assert_ne!(edited, projected, "prose replacement must have matched");
        std::fs::write(&md_path, &edited).unwrap();

        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(
            stats.applied, 1,
            "genuine prose edit must apply; got {:?}",
            stats
        );
        let p = db.get_page("page_edit").await.unwrap().unwrap();
        assert!(p.content.contains("edited prose by the user"));
        assert!(
            !p.content
                .contains(crate::export::provenance::SOURCES_BLOCK_START),
            "the generated block must NOT be persisted into Page.content"
        );
        assert!(p.user_edited, "a real prose edit flags user_edited");

        // The block-free body means no `mem_*` rows reach the wikilink graph
        // via replace_page_links.
        let links = db
            .get_page_outbound_links_scoped("page_edit", &crate::read_scope::ReadScope::Global)
            .await
            .unwrap();
        assert!(
            !links.iter().any(|l| l.label.starts_with("mem_")),
            "no mem_* link rows should persist; got {:?}",
            links
        );
    }

    #[tokio::test]
    async fn edited_sources_block_on_disk_is_reprojected_to_db_truth() {
        use crate::export::provenance::{SOURCES_BLOCK_END, SOURCES_BLOCK_START};
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let mut page = sample_page("page_block", "Block Topic", "## Overview\ncanonical prose");
        page.source_memory_ids = vec!["mem_real".to_string()];
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_block",
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_real"],
            &now,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);
        let md_path = knowledge_dir
            .path()
            .join(writer.page_filename(&page.id).expect("state has file"));

        // User tampers with ONLY the protected block (adds a fake source),
        // leaving the canonicalized prose identical.
        let projected = std::fs::read_to_string(&md_path).unwrap();
        let tampered = projected.replace(
            &format!("{SOURCES_BLOCK_START}\n## Sources\n- [[mem_real]]\n{SOURCES_BLOCK_END}"),
            &format!("{SOURCES_BLOCK_START}\n## Sources\n- [[mem_real]]\n- [[mem_FAKE]]\n{SOURCES_BLOCK_END}"),
        );
        assert_ne!(
            tampered, projected,
            "replacement must have matched the block"
        );
        std::fs::write(&md_path, &tampered).unwrap();

        let stats = sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();
        assert_eq!(stats.applied, 0); // canonicalized prose unchanged → not a user edit
        let p = db.get_page("page_block").await.unwrap().unwrap();
        assert!(!p.content.contains(SOURCES_BLOCK_START)); // DB stays canonical
        assert!(!p.content.contains("mem_FAKE"));
        assert!(!p.user_edited);
        // The file on disk was re-projected from DB truth — fake source gone.
        let after = std::fs::read_to_string(&md_path).unwrap();
        assert!(!after.contains("mem_FAKE"));
        assert!(after.contains("[[mem_real]]"));
    }

    #[tokio::test]
    async fn no_duplicate_file_when_state_missing_on_reprojection() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page("page_nostate", "No State", "## Overview\nold body");
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_nostate",
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_z"],
            &now,
        )
        .await
        .unwrap();
        // Write an OLD-format .md directly (no write_page → no state.json entry),
        // with frontmatter origin_id matching the DB row. The prose body equals
        // the DB content, but the file lacks the delimiter Sources block a fresh
        // render adds — so the canonicalized bodies are EQUAL (reaching the
        // equal-canon branch) while raw_body != fresh_body (which would trigger
        // re-projection if not for the missing-state guard). origin_version: 1
        // matches the DB row so the watcher does not bail via SkippedDaemonAhead.
        let md = format!(
            "---\ntitle: \"{}\"\norigin_id: page_nostate\norigin_version: 1\n---\n\n## Overview\nold body\n",
            page.title
        );
        std::fs::write(knowledge_dir.path().join("no-state.md"), &md).unwrap();

        sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();

        // No duplicate page file forked.
        let md_files: Vec<_> = std::fs::read_dir(knowledge_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            !md_files.iter().any(|n| n.contains("-2")),
            "must not fork a duplicate; got {:?}",
            md_files
        );
    }

    /// Task 5: pure projection + watcher tick with NO user edit must produce
    /// zero `mem_*` page_link rows.  Distinct from Task 4's test, which edits
    /// prose; here we never touch the file after `KnowledgeWriter::write_page`.
    /// The on-disk file carries `[[mem_alpha]]`/`[[mem_beta]]` inside the
    /// generated Sources block, but the canonicalized body written back to the
    /// DB must never include them, so no `mem_*` link rows are ever created.
    #[tokio::test]
    async fn projecting_provenance_creates_no_mem_page_links_no_edit() {
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let page = sample_page(
            "page_links",
            "Links Topic",
            "## Overview\nrefers to [[Real Topic]] but not memories",
        );
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_links",
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_alpha", "mem_beta"],
            &now,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);

        // Pure projection + a watcher tick with NO user edit. The on-disk file
        // carries the Sources block (with [[mem_alpha]]/[[mem_beta]]), but the
        // canonicalized body written to the DB must never include them, so no
        // mem_* link rows are ever created.
        sync_filesystem_edits(&db, knowledge_dir.path())
            .await
            .unwrap();

        let links = db
            .get_page_outbound_links_scoped("page_links", &crate::read_scope::ReadScope::Global)
            .await
            .unwrap();
        let mem_links: Vec<_> = links
            .iter()
            .filter(|l| l.label.starts_with("mem_"))
            .collect();
        assert!(
            mem_links.is_empty(),
            "no page_links row may target a mem_* id; got {:?}",
            mem_links.iter().map(|l| &l.label).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn full_provenance_round_trip_is_stable() {
        use crate::export::provenance::SOURCES_BLOCK_START;
        let (db, _ddir) = fresh_db().await;
        let knowledge_dir = TempDir::new().unwrap();
        let mut page = sample_page(
            "page_rt",
            "RT Topic",
            "## Overview\nstable prose with [[Other]]",
        );
        page.source_memory_ids = vec!["mem_one".to_string(), "mem_two".to_string()];
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page(
            "page_rt",
            &page.title,
            None,
            &page.content,
            None,
            None,
            &["mem_one", "mem_two"],
            &now,
        )
        .await
        .unwrap();
        let writer = tracked_writer(&db, knowledge_dir.path());
        project_page(&db, &writer, &page);

        // Two consecutive ticks — both Unchanged, DB never gains the block.
        for _ in 0..2 {
            let stats = sync_filesystem_edits(&db, knowledge_dir.path())
                .await
                .unwrap();
            assert_eq!(stats.applied, 0);
            assert_eq!(stats.skipped_unchanged, 1);
        }
        let p = db.get_page("page_rt").await.unwrap().unwrap();
        assert!(!p.content.contains(SOURCES_BLOCK_START));
        assert!(!p.user_edited);
        // Stubs resolve for both cited memories.
        assert!(knowledge_dir
            .path()
            .join("_sources")
            .join("mem_one.md")
            .exists());
        assert!(knowledge_dir
            .path()
            .join("_sources")
            .join("mem_two.md")
            .exists());
        // No mem_* page_links.
        let links = db
            .get_page_outbound_links_scoped("page_rt", &crate::read_scope::ReadScope::Global)
            .await
            .unwrap();
        assert!(links.iter().all(|l| !l.label.starts_with("mem_")));
    }
}
