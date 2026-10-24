// SPDX-License-Identifier: Apache-2.0
//! Obsidian vault and Logseq graph sync: the one routine the HTTP handler and
//! the background scheduler share, so a vault follows edits made in Obsidian
//! (or a graph edits made in Logseq) the same way a plain folder does. Below,
//! "vault" covers both and "note" a vault note or a Logseq page.
//!
//! - A note is stored as chunk documents `<source>::<path>::<n>` that pass the
//!   quality gate, embedded at once (no document queue, no LLM).
//! - Unchanged notes are skipped on mtime, then on content hash.
//! - A changed note replaces its chunks; chunks the new text no longer has are
//!   removed, so a shorter note leaves no stale tail behind.
//! - A note that vanished from the vault (deleted or renamed) loses its
//!   chunks and sync state; a renamed note is read again at its new path, so
//!   nothing is stored twice.
//! - A missing or unreadable vault root marks the source unavailable and
//!   deletes nothing, like the Directory sync's root guard.
//! - A note the quality gate rejects keeps no sync state, so a later daemon
//!   (with possibly looser rules) reads it again. Within one daemon run its
//!   mtime is remembered so the 30 second poll does not re-read it each time.
//! - One sync per source runs at a time. The HTTP handler waits for a running
//!   sync; the scheduler skips a source whose sync is still running.
//! - The scheduler's pass stores at most [`OBSIDIAN_POLL_NOTE_LIMIT`] changed
//!   notes ([`OBSIDIAN_BUSY_NOTE_LIMIT`] while the machine is busy), so it
//!   never holds the serial poll loop for a whole vault; the rest are picked
//!   up by the next polls. The HTTP sync has no limit.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use wenlan_core::db::MemoryDB;
use wenlan_core::quality_gate::QualityGate;
use wenlan_core::sources::{logseq, obsidian};
use wenlan_core::sources::{RawDocument, Source, SourceType};

use super::{
    content_hash, directory_root_is_live, finalize_sync, is_google_drive_path,
    mark_source_unavailable, SkippedFile, SyncStatsResponse, SKIPPED_FILES_LIMIT,
};
use crate::error::ServerError;

/// The most changed notes one scheduler pass stores per vault.
pub(crate) const OBSIDIAN_POLL_NOTE_LIMIT: usize = 50;

/// The most changed notes one scheduler pass stores per vault while the CPU
/// is busy: enough that an edit made in Obsidian still shows up within a
/// poll or two, small enough that embedding it costs little.
pub(crate) const OBSIDIAN_BUSY_NOTE_LIMIT: usize = 5;

type SyncLock = Arc<tokio::sync::Mutex<()>>;

static SYNC_LOCKS: LazyLock<Mutex<HashMap<String, SyncLock>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(source id, note path)` -> mtime of a note the quality gate rejected in
/// this daemon run.
static GATE_REJECTED: LazyLock<Mutex<HashMap<(String, String), i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn sync_lock(source_id: &str) -> SyncLock {
    SYNC_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(source_id.to_string())
        .or_default()
        .clone()
}

fn gate_rejected_at(source_id: &str, path: &str) -> Option<i64> {
    GATE_REJECTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&(source_id.to_string(), path.to_string()))
        .copied()
}

fn set_gate_rejected(source_id: &str, path: &str, mtime_ns: Option<i64>) {
    let mut rejected = GATE_REJECTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (source_id.to_string(), path.to_string());
    match mtime_ns {
        Some(mtime_ns) => {
            rejected.insert(key, mtime_ns);
        }
        None => {
            rejected.remove(&key);
        }
    }
}

/// Sync an Obsidian or Logseq source, waiting for a sync of the same source
/// that is already running.
pub(crate) async fn sync_obsidian_source(
    db: Arc<MemoryDB>,
    quality_gate: &QualityGate,
    source: &Source,
) -> Result<SyncStatsResponse, ServerError> {
    let lock = sync_lock(&source.id);
    let _guard = lock.lock().await;
    sync_obsidian_source_locked(db, quality_gate, source, None).await
}

/// Sync an Obsidian or Logseq source unless a sync of it is already running, in which
/// case return `None` at once. Used by the background poll, so it stores at
/// most `note_limit` changed notes.
pub(crate) async fn try_sync_obsidian_source(
    db: Arc<MemoryDB>,
    quality_gate: &QualityGate,
    source: &Source,
    note_limit: usize,
) -> Option<Result<SyncStatsResponse, ServerError>> {
    let lock = sync_lock(&source.id);
    let _guard = lock.try_lock().ok()?;
    Some(sync_obsidian_source_locked(db, quality_gate, source, Some(note_limit)).await)
}

fn note_chunk_prefix(source_id: &str, path: &str) -> String {
    format!("{source_id}::{path}::")
}

/// Every chunk document id stored for one note. The suffix must be a chunk
/// number, so `a.md` never claims the chunks of a note at `a.md::x.md`.
async fn note_chunk_ids(
    db: &MemoryDB,
    source_id: &str,
    path: &str,
) -> Result<Vec<String>, ServerError> {
    let prefix = note_chunk_prefix(source_id, path);
    let ids = db.list_source_ids_with_prefix("memory", &prefix).await?;
    Ok(ids
        .into_iter()
        .filter(|id| {
            id.strip_prefix(&prefix).is_some_and(|rest| {
                !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .collect())
}

/// Delete the chunks of one note that are not in `keep`. Returns how many
/// deletes failed.
async fn delete_note_chunks_except(
    db: &MemoryDB,
    source_id: &str,
    path: &str,
    keep: &HashSet<String>,
) -> usize {
    let ids = match note_chunk_ids(db, source_id, path).await {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!("[sync] chunk lookup failed for {path}: {error}");
            return 1;
        }
    };
    let mut failures = 0;
    for id in ids.into_iter().filter(|id| !keep.contains(id)) {
        if let Err(error) = db.delete_by_source_id("memory", &id).await {
            tracing::warn!("[sync] chunk delete failed for {id}: {error}");
            failures += 1;
        }
    }
    failures
}

fn vault_root_is_live(path: &Path) -> bool {
    path.is_dir() && directory_root_is_live(path)
}

/// The note files of a vault or graph. Wenlan's own pages folder is left
/// out when the person put it inside the vault: those are pages, not notes.
fn scan_notes(source_type: &SourceType, root: &Path) -> Vec<std::path::PathBuf> {
    let notes = match source_type {
        SourceType::Logseq => logseq::scan_graph(root),
        _ => obsidian::scan_vault(root),
    };
    let pages = wenlan_core::config::load_config().knowledge_path_or_default();
    wenlan_core::pages_folder::without_pages_folder(root, &pages, notes)
}

fn note_documents(
    source_type: &SourceType,
    source_id: &str,
    path: &Path,
    content: &str,
    mtime: i64,
) -> Vec<RawDocument> {
    match source_type {
        SourceType::Logseq => logseq::page_to_documents(source_id, path, content, mtime),
        _ => obsidian::note_to_documents(source_id, path, content, mtime),
    }
}

async fn sync_obsidian_source_locked(
    db: Arc<MemoryDB>,
    quality_gate: &QualityGate,
    source: &Source,
    note_limit: Option<usize>,
) -> Result<SyncStatsResponse, ServerError> {
    let id = source.id.clone();

    // Vault files are frequently cloud "online-only" placeholders: every
    // filesystem call below runs off the runtime so one on-demand download
    // cannot stall a tokio worker.
    let vault_root = source.path.clone();
    let vault_display = vault_root.display().to_string();
    let source_type = source.source_type.clone();
    let md_files = tokio::task::spawn_blocking(move || {
        vault_root_is_live(&vault_root).then(|| scan_notes(&source_type, &vault_root))
    })
    .await
    .map_err(|error| {
        ServerError::Internal(format!(
            "vault scan task failed for source {id} at {vault_display}: {error}"
        ))
    })?;
    // Root guard: a vault that is gone or unreadable is "unavailable", not
    // "every note deleted". Nothing is removed; a later sync against a live
    // root flips the source back to Active.
    let Some(md_files) = md_files else {
        mark_source_unavailable(&id, "source path is missing or unreadable");
        return Ok(SyncStatsResponse {
            files_found: 0,
            ingested: 0,
            skipped: 0,
            errors: 0,
            error_detail: None,
            paused: None,
            queued_files: None,
            waiting_files: None,
            skipped_files: Vec::new(),
        });
    };

    let mut ingested: usize = 0;
    let mut skipped: usize = 0;
    let mut errors: usize = 0;
    // Per-file error counters used for the GDrive threshold. Kept separate
    // from `errors` (which also tallies per-chunk upsert failures) so the
    // threshold comparison stays in consistent "files" units.
    let mut file_errors: usize = 0;
    let mut gdrive_errors: usize = 0;
    let mut stored_notes: usize = 0;
    let mut skipped_files: Vec<SkippedFile> = Vec::new();
    let mut note_skipped = |path: &Path, reason: &str| {
        tracing::info!(
            "[sync] {id}: stored nothing from {} ({reason})",
            path.display()
        );
        if skipped_files.len() < SKIPPED_FILES_LIMIT {
            skipped_files.push(SkippedFile {
                path: path.to_string_lossy().to_string(),
                reason: reason.to_string(),
            });
        }
    };

    // Notes that left the vault: a deletion, or the old half of a rename.
    let scanned: HashSet<String> = md_files
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    let tracked = db.list_sync_state_paths(&id).await.unwrap_or_default();
    for tracked_path in tracked.iter().filter(|path| !scanned.contains(*path)) {
        let failures = delete_note_chunks_except(&db, &id, tracked_path, &HashSet::new()).await;
        if failures > 0 {
            // Keep the sync state so the next sync retries the cleanup.
            errors += failures;
            continue;
        }
        if let Err(error) = db.delete_sync_state(&id, tracked_path).await {
            tracing::warn!("[sync] sync state delete failed for {tracked_path}: {error}");
            errors += 1;
        }
        set_gate_rejected(&id, tracked_path, None);
    }

    for file_path in &md_files {
        if note_limit.is_some_and(|limit| stored_notes >= limit) {
            // The rest keep their old sync state and are read next time.
            break;
        }
        let file_key = file_path.to_string_lossy().to_string();
        let is_gdrive = is_google_drive_path(file_path);
        let mut count_file_error = |errors: &mut usize| {
            *errors += 1;
            file_errors += 1;
            if is_gdrive {
                gdrive_errors += 1;
            }
        };

        let metadata_path = file_path.clone();
        let metadata =
            match tokio::task::spawn_blocking(move || std::fs::metadata(metadata_path)).await {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    tracing::warn!("[sync] stat failed for {}: {}", file_path.display(), e);
                    count_file_error(&mut errors);
                    continue;
                }
                Err(e) => {
                    tracing::warn!("[sync] stat task failed for {}: {}", file_path.display(), e);
                    count_file_error(&mut errors);
                    continue;
                }
            };
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);

        let existing = db.get_sync_state(&id, &file_key).await.ok().flatten();
        if existing.as_ref().is_some_and(|ss| ss.mtime_ns == mtime_ns)
            || gate_rejected_at(&id, &file_key) == Some(mtime_ns)
        {
            skipped += 1;
            continue;
        }

        let read_path = file_path.clone();
        let content =
            match tokio::task::spawn_blocking(move || std::fs::read_to_string(read_path)).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    tracing::warn!("[sync] read failed for {}: {}", file_path.display(), e);
                    count_file_error(&mut errors);
                    continue;
                }
                Err(e) => {
                    tracing::warn!("[sync] read task failed for {}: {}", file_path.display(), e);
                    count_file_error(&mut errors);
                    continue;
                }
            };

        // Hash check: mtime changed but content didn't, update mtime only.
        let hash = content_hash(&content);
        if existing.as_ref().is_some_and(|ss| ss.content_hash == hash) {
            let _ = db.upsert_sync_state(&id, &file_key, mtime_ns, &hash).await;
            skipped += 1;
            continue;
        }

        let mtime_secs = mtime_ns / 1_000_000_000;
        let docs = note_documents(&source.source_type, &id, file_path, &content, mtime_secs);
        if docs.is_empty() {
            // MOC, empty Logseq page or one of Wenlan's own projected pages:
            // nothing to store,
            // so drop what an earlier version of the note left and record
            // the state.
            errors += delete_note_chunks_except(&db, &id, &file_key, &HashSet::new()).await;
            let _ = db.upsert_sync_state(&id, &file_key, mtime_ns, &hash).await;
            set_gate_rejected(&id, &file_key, None);
            note_skipped(
                file_path,
                match source.source_type {
                    SourceType::Logseq => obsidian::SKIP_REASON_EMPTY,
                    _ => obsidian::empty_note_reason(file_path, &content),
                },
            );
            skipped += 1;
            continue;
        }

        let filtered: Vec<_> = docs
            .into_iter()
            .filter(|d| quality_gate.check_content(&d.content).admitted)
            .collect();

        // Everything filtered out (pure boilerplate): the earlier text is
        // gone from the note, so its chunks go too, but the note is NOT
        // marked synced. A later daemon re-reads it, so it can be ingested if
        // the quality gate rules are loosened (invariant from PR #57).
        if filtered.is_empty() {
            errors += delete_note_chunks_except(&db, &id, &file_key, &HashSet::new()).await;
            if existing.is_some() {
                let _ = db.delete_sync_state(&id, &file_key).await;
            }
            set_gate_rejected(&id, &file_key, Some(mtime_ns));
            note_skipped(file_path, "too_short");
            skipped += 1;
            continue;
        }

        let count = filtered.len();
        let keep: HashSet<String> = filtered.iter().map(|d| d.source_id.clone()).collect();
        match db.upsert_documents(filtered).await {
            Ok(_) => {
                ingested += count;
                stored_notes += 1;
                // Upsert first, then drop the old tail, so the note is never
                // missing from search in between.
                let failures = delete_note_chunks_except(&db, &id, &file_key, &keep).await;
                errors += failures;
                if failures == 0 {
                    let _ = db.upsert_sync_state(&id, &file_key, mtime_ns, &hash).await;
                }
                set_gate_rejected(&id, &file_key, None);
            }
            Err(e) => {
                tracing::error!("[sync] upsert failed for {}: {}", file_path.display(), e);
                errors += count;
            }
        }
    }

    let stats = finalize_sync(
        db.clone(),
        &id,
        md_files.len(),
        ingested,
        skipped,
        errors,
        file_errors,
        gdrive_errors,
        None,
    )
    .await?;
    record_stored_count(&db, &id).await;
    Ok(SyncStatsResponse {
        skipped_files,
        ..stats
    })
}

/// `finalize_sync` adds this sync's upserts to `memory_count`, which for a
/// vault that is re-read on every edit counts each edit again. Store the
/// number of chunk documents the vault holds now instead.
async fn record_stored_count(db: &MemoryDB, source_id: &str) {
    let stored = match db
        .list_source_ids_with_prefix("memory", &format!("{source_id}::"))
        .await
    {
        Ok(ids) => ids.len() as u64,
        Err(error) => {
            tracing::warn!("[sync] stored count failed for {source_id}: {error}");
            return;
        }
    };
    let _ = wenlan_core::config::update_config(|config| {
        if let Some(src) = config.sources.iter_mut().find(|s| s.id == source_id) {
            src.memory_count = stored;
        }
        Ok(())
    });
}

#[cfg(test)]
#[path = "obsidian_sync_tests.rs"]
mod obsidian_sync_tests;
