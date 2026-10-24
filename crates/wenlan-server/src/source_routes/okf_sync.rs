// SPDX-License-Identifier: Apache-2.0
//! OKF bundle sync: the Directory sync's diff (mtime and hash fast paths,
//! same-bytes rename rebind, vanished-file deletion) with an OKF bundle's
//! rules on top.
//!
//! - Only concept files count: reserved names (`index.md`, `log.md`, an
//!   untyped `INSTRUCTIONS.md`) and folders holding Wenlan's own export marker
//!   are pruned before anything else, and a root that is itself a Wenlan
//!   export is refused.
//! - An `INSTRUCTIONS.md` that cannot be read or parsed right now is neither
//!   a concept nor confirmed untyped. It keeps whatever it last imported and
//!   counts as a file error until it reads again.
//! - A deprecated concept is removed like a vanished file and its hash is
//!   recorded, so it is not read again until it changes.
//! - New and changed concepts go to the document queue in batches: nothing
//!   is handed over while an earlier batch is still being prepared, and at
//!   most [`OKF_BATCH_LIMIT`] per sync, in concept id order. The rest are
//!   counted as waiting. Rebinds and removals are never batched.
//! - Pages linking to a concept that was removed, deprecated, or moved
//!   re-resolve their links, so no edge points at a removed page.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use wenlan_core::db::MemoryDB;
use wenlan_core::document_enrichment::source_page_id;
use wenlan_core::sources::directory::document_source_id;
use wenlan_core::sources::okf;
use wenlan_core::sources::Source;
use wenlan_core::WenlanError;

use super::{
    content_hash_bytes, directory_root_is_live, finalize_sync, is_google_drive_path,
    mark_source_unavailable, BatchCounts, DirectorySyncOutcome, SyncStatsResponse,
};
use crate::error::ServerError;

/// The most concept files one sync hands to the document queue.
pub(crate) const OKF_BATCH_LIMIT: usize = 1_000;

enum BundleScan {
    Unavailable,
    WenlanExport,
    Concepts(okf::ConceptScan),
}

/// Sync an `okf` source. The caller guarantees `source.source_type` is
/// `SourceType::Okf`.
pub(crate) async fn sync_okf_source(
    db: Arc<MemoryDB>,
    source: &Source,
    config: &wenlan_core::config::Config,
) -> Result<DirectorySyncOutcome, ServerError> {
    sync_okf_source_in_batches(db, source, config, OKF_BATCH_LIMIT).await
}

pub(super) async fn sync_okf_source_in_batches(
    db: Arc<MemoryDB>,
    source: &Source,
    config: &wenlan_core::config::Config,
    batch_limit: usize,
) -> Result<DirectorySyncOutcome, ServerError> {
    let id = source.id.clone();
    let root = source.path.clone();
    let root_display = root.display().to_string();

    // Root guard as for a Directory source: a missing or unreadable bundle
    // deletes nothing. A root that became a Wenlan export, or a stored Space
    // that no longer exists, blocks the sync the same way until fixed.
    let scan_root = root.clone();
    let scan = tokio::task::spawn_blocking(move || {
        if !directory_root_is_live(&scan_root) || !scan_root.is_dir() {
            BundleScan::Unavailable
        } else if okf::is_wenlan_export(&scan_root) {
            BundleScan::WenlanExport
        } else {
            BundleScan::Concepts(okf::scan_bundle(&scan_root))
        }
    })
    .await
    .map_err(|error| {
        ServerError::Internal(format!(
            "OKF bundle scan task failed for source {id} at {root_display}: {error}"
        ))
    })?;
    let okf::ConceptScan {
        concepts,
        unresolved,
    } = match scan {
        BundleScan::Unavailable => {
            mark_source_unavailable(&id, "bundle folder is missing or unreadable");
            return Ok(blocked_outcome());
        }
        BundleScan::WenlanExport => {
            mark_source_unavailable(
                &id,
                "folder is a Wenlan OKF export, which Wenlan does not import back",
            );
            return Ok(blocked_outcome());
        }
        BundleScan::Concepts(scan) => scan,
    };
    if let Some(space) = source.space.as_deref() {
        if db.get_space(space).await?.is_none() {
            mark_source_unavailable(&id, &format!("Space '{space}' no longer exists"));
            return Ok(blocked_outcome());
        }
    }

    let knowledge_path = config.knowledge_path_or_default();
    let scanned: HashSet<String> = concepts
        .iter()
        .map(|(_, path)| path.to_string_lossy().to_string())
        .collect();
    // Tracked files that could not be classified are not missing: the file is
    // on disk, so what it imported stays until it reads again.
    let unresolved_keys: HashSet<String> = unresolved
        .iter()
        .map(|(path, _)| path.to_string_lossy().to_string())
        .collect();

    // Tracked files missing from the pruned scan vanished. Their hashes let a
    // same-bytes file at a new path rebind instead of re-importing.
    let mut vanished_by_hash: HashMap<String, (String, String)> = HashMap::new();
    let mut files_to_delete: Vec<(String, String)> = Vec::new();
    let mut tracked_unresolved: HashSet<String> = HashSet::new();
    if let Ok(tracked) = db.list_sync_state_paths(&id).await {
        for tracked_path in tracked {
            if scanned.contains(&tracked_path) {
                continue;
            }
            if unresolved_keys.contains(&tracked_path) {
                tracked_unresolved.insert(tracked_path);
                continue;
            }
            if let Ok(Some(sync_state)) = db.get_sync_state(&id, &tracked_path).await {
                let doc_source_id =
                    document_source_id(&id, Path::new(&tracked_path), Some(&knowledge_path));
                vanished_by_hash.insert(
                    sync_state.content_hash.clone(),
                    (doc_source_id.clone(), tracked_path.clone()),
                );
                files_to_delete.push((tracked_path, doc_source_id));
            }
        }
    }

    // The batch gate: no new work while an earlier batch is unprepared.
    let mut room = if db.count_unprepared_documents_for_source(&id).await? == 0 {
        batch_limit
    } else {
        0
    };

    let mut ingested: usize = 0;
    let mut newly_queued: usize = 0;
    let mut skipped: usize = 0;
    let mut errors: usize = 0;
    let mut file_errors: usize = 0;
    let mut gdrive_errors: usize = 0;
    let mut waiting: usize = 0;
    let mut renamed_files: HashSet<String> = HashSet::new();

    // Only a file that imported before has state to protect, so only it is a
    // file error. One never imported stays skipped quietly, as before.
    for (file_path, detail) in &unresolved {
        if !tracked_unresolved.contains(file_path.to_string_lossy().as_ref()) {
            tracing::debug!(
                "[sync] skipping {} until it reads: {}",
                file_path.display(),
                detail
            );
            continue;
        }
        tracing::warn!(
            "[sync] cannot classify {}: {}; keeping what it last imported",
            file_path.display(),
            detail
        );
        errors += 1;
        file_errors += 1;
        if is_google_drive_path(file_path) {
            gdrive_errors += 1;
        }
    }
    // Concept ids that arrived at a new path, left, or were deprecated, and
    // pages that moved: their linkers re-resolve after the diff.
    let mut moved_concepts: BTreeSet<String> = BTreeSet::new();
    let mut moved_pages: Vec<String> = Vec::new();

    for (concept_id, file_path) in &concepts {
        let file_key = file_path.to_string_lossy().to_string();
        let is_gdrive = is_google_drive_path(file_path);

        let metadata_path = file_path.clone();
        let metadata =
            match tokio::task::spawn_blocking(move || std::fs::metadata(metadata_path)).await {
                Ok(Ok(metadata)) => metadata,
                Ok(Err(e)) => {
                    tracing::warn!("[sync] stat failed for {}: {}", file_path.display(), e);
                    errors += 1;
                    file_errors += 1;
                    if is_gdrive {
                        gdrive_errors += 1;
                    }
                    continue;
                }
                Err(e) => {
                    tracing::warn!("[sync] stat task failed for {}: {}", file_path.display(), e);
                    errors += 1;
                    file_errors += 1;
                    if is_gdrive {
                        gdrive_errors += 1;
                    }
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
        if let Some(ref ss) = existing {
            if ss.mtime_ns == mtime_ns {
                skipped += 1;
                continue;
            }
        }
        // A file never handed over, arriving while the batch is full, waits
        // unread unless a vanished file could be its old path. A changed file
        // already imported is still read: deprecating it is a removal. A file
        // in the queue without a receipt belongs to an earlier batch, not the
        // waiting count.
        let queue_entry = db.get_queue_entry(&id, &file_key).await.ok().flatten();
        if existing.is_none() && queue_entry.is_none() && room == 0 && vanished_by_hash.is_empty() {
            waiting += 1;
            continue;
        }

        let read_path = file_path.clone();
        let read_root = root.clone();
        let (hash, deprecated) = match tokio::task::spawn_blocking(move || {
            std::fs::read(&read_path).map(|bytes| {
                (
                    content_hash_bytes(&bytes),
                    okf::bytes_are_deprecated_concept(&read_root, &read_path, &bytes),
                )
            })
        })
        .await
        {
            Ok(Ok(read)) => read,
            Ok(Err(e)) => {
                tracing::warn!("[sync] read failed for {}: {}", file_path.display(), e);
                errors += 1;
                file_errors += 1;
                if is_gdrive {
                    gdrive_errors += 1;
                }
                continue;
            }
            Err(e) => {
                tracing::warn!("[sync] read task failed for {}: {}", file_path.display(), e);
                errors += 1;
                file_errors += 1;
                if is_gdrive {
                    gdrive_errors += 1;
                }
                continue;
            }
        };
        if let Some(ref ss) = existing {
            if ss.content_hash == hash {
                let _ = db.upsert_sync_state(&id, &file_key, mtime_ns, &hash).await;
                skipped += 1;
                continue;
            }
        }

        if deprecated {
            // Whether it was imported before or never: nothing of it stays,
            // and pages citing it are flagged by the removal.
            match remove_concept(&db, &id, &file_key, &knowledge_path).await {
                Ok(()) => {
                    if let Err(error) = db.upsert_sync_state(&id, &file_key, mtime_ns, &hash).await
                    {
                        tracing::warn!(
                            "[sync] deprecated sync_state failed for {}: {}",
                            file_path.display(),
                            error
                        );
                        errors += 1;
                        file_errors += 1;
                    }
                    moved_concepts.insert(concept_id.clone());
                    skipped += 1;
                }
                Err(error) => {
                    tracing::warn!(
                        "[sync] deprecated concept removal failed for {}: {}",
                        file_path.display(),
                        error
                    );
                    errors += 1;
                    file_errors += 1;
                }
            }
            continue;
        }

        if let Some((old_doc_source_id, old_path)) = vanished_by_hash.remove(&hash) {
            let new_doc_source_id = document_source_id(&id, file_path, Some(&knowledge_path));
            let old_source_page_id = source_page_id(&id, &old_path);
            let new_source_page_id = source_page_id(&id, &file_key);
            match db
                .rebind_source_id_with_source_page(
                    "memory",
                    old_doc_source_id.as_str(),
                    &new_doc_source_id,
                    &old_source_page_id,
                    &new_source_page_id,
                )
                .await
            {
                Ok(()) => {
                    for outcome in [
                        db.delete_sync_state(&id, &old_path).await,
                        db.dequeue_document(&id, &old_path).await,
                        db.upsert_sync_state(&id, &file_key, mtime_ns, &hash).await,
                        db.rekey_okf_concept(&old_source_page_id, &new_source_page_id, concept_id)
                            .await
                            .map(|_| ()),
                    ] {
                        if let Err(error) = outcome {
                            tracing::warn!(
                                "[sync] rename bookkeeping failed for {} (from {}): {}",
                                file_path.display(),
                                old_path,
                                error
                            );
                            errors += 1;
                            file_errors += 1;
                        }
                    }
                    if let Some(old_concept_id) = okf::concept_id(&root, Path::new(&old_path)) {
                        moved_concepts.insert(old_concept_id);
                    }
                    moved_concepts.insert(concept_id.clone());
                    moved_pages.push(new_source_page_id);
                    renamed_files.insert(old_path);
                    skipped += 1;
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        "[sync] rebind yielded for {} (candidate {}): {}; falling back to enqueue",
                        file_path.display(),
                        old_path,
                        error
                    );
                }
            }
        }

        let already_queued =
            queue_entry.is_some_and(|entry| entry.content_hash.as_deref() == Some(hash.as_str()));
        if !already_queued && room == 0 {
            waiting += 1;
            continue;
        }
        match db.enqueue_document(&id, &file_key, Some(&hash)).await {
            Ok(_) => {
                // `ingested` feeds `memory_count`, so counting a row that was
                // already queued at this same hash would inflate the source's
                // total on every re-poll. With a 1,000-file batch a bundle is
                // re-polled routinely while the worker drains, so only a row
                // this sync actually hands over counts.
                if !already_queued {
                    ingested += 1;
                    newly_queued += 1;
                    room -= 1;
                }
            }
            Err(e) => {
                tracing::error!("[sync] enqueue failed for {}: {}", file_path.display(), e);
                errors += 1;
            }
        }
    }

    for (path, doc_source_id) in files_to_delete {
        if renamed_files.contains(&path) {
            continue;
        }
        let _ = db.delete_by_source_id("memory", &doc_source_id).await;
        let _ = db.delete_sync_state(&id, &path).await;
        let _ = db.dequeue_document(&id, &path).await;
        if let Err(error) = db.delete_okf_concept(&source_page_id(&id, &path)).await {
            tracing::warn!("[sync] OKF provenance removal failed for {path}: {error}");
            errors += 1;
            file_errors += 1;
        }
        if let Some(removed_concept_id) = okf::concept_id(&root, Path::new(&path)) {
            moved_concepts.insert(removed_concept_id);
        }
    }

    if !moved_concepts.is_empty() || !moved_pages.is_empty() {
        let moved: Vec<String> = moved_concepts.into_iter().collect();
        if let Err(error) = db
            .refresh_okf_concept_linkers(&id, &moved, &moved_pages)
            .await
        {
            tracing::warn!("[sync] {id}: OKF link refresh failed: {error}");
        }
    }

    let queued = db.count_unprepared_documents_for_source(&id).await?;
    // A file this sync handed over cleanly can still fail in the worker until
    // its retries run out. Those rows leave the gate but never become a page,
    // so counting only this run's read failures would report `errors: 0` for a
    // bundle that imported nothing. `finalize_sync` folds them into `errors`.
    let exhausted = db.count_exhausted_documents_for_source(&id).await?;
    let stats = finalize_sync(
        db,
        &id,
        concepts.len(),
        ingested,
        skipped,
        errors,
        file_errors,
        gdrive_errors,
        Some(BatchCounts {
            queued_files: queued,
            waiting_files: waiting as u64,
            exhausted_files: exhausted,
        }),
    )
    .await?;
    Ok(DirectorySyncOutcome {
        stats,
        newly_queued,
    })
}

/// Remove an imported concept's chunks (flagging pages that cite it), its
/// provenance and links, and any queued work for it.
async fn remove_concept(
    db: &MemoryDB,
    source_id: &str,
    file_key: &str,
    knowledge_path: &Path,
) -> Result<(), WenlanError> {
    let doc_source_id = document_source_id(source_id, Path::new(file_key), Some(knowledge_path));
    db.delete_by_source_id("memory", &doc_source_id).await?;
    db.delete_okf_concept(&source_page_id(source_id, file_key))
        .await?;
    db.dequeue_document(source_id, file_key).await
}

fn blocked_outcome() -> DirectorySyncOutcome {
    DirectorySyncOutcome {
        stats: SyncStatsResponse {
            files_found: 0,
            ingested: 0,
            skipped: 0,
            errors: 0,
            error_detail: None,
            paused: None,
            queued_files: None,
            waiting_files: None,
            skipped_files: Vec::new(),
        },
        newly_queued: 0,
    }
}

#[cfg(test)]
#[path = "okf_sync_tests.rs"]
mod tests;
