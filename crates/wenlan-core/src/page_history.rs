// SPDX-License-Identifier: Apache-2.0
//! One history per page (UX-HIST-1/2).
//!
//! A page's history is one list of three kinds of change: the person's edits,
//! AI edits, and "a page this one relies on got a new version". The first two
//! come from the page's own changelog; the third is derived from the writes of
//! the pages it relies on (its `page_basis` rows), so nothing new is stored.
//! With `include_text` every edit carries the page text at that version and
//! every source update the source's text at its new version, so each entry can
//! be opened to see what changed.

use crate::db::{MemoryDB, PageHistoryEntry};
use crate::error::WenlanError;
use std::collections::{HashMap, HashSet};
use wenlan_types::responses::{
    PageChangelogEntry, PageHistoryItem, PageHistoryKind, PageHistorySource,
};

/// Versions read per page when texts or source updates are needed.
const HISTORY_SCAN: i64 = 1_000;

/// How long a run of the person's saves may span and still be listed as one
/// change. The editor saves each pause (650 ms), so without this every pause
/// is its own entry. Notion records one version per 10 minutes of active
/// editing: "A new version of a particular page will be recorded every 10
/// minutes as you actively edit it."
/// (https://www.notion.com/help/duplicate-delete-and-restore-content)
pub const SAVE_RUN_SECS: i64 = 10 * 60;

/// Writers whose saves are folded into one change: the editor and an edit of
/// the page's file. Both save on every pause.
pub(crate) fn folds(edited_by: Option<&str>) -> bool {
    matches!(edited_by, Some("manual_edit" | "fs_edit"))
}

/// Lists a run of the person's saves as one change (UX-HIST-3).
///
/// Only the list is folded. Every version stays stored under its own number,
/// so a citing note, a basis, a restore or a shared folder that names a
/// version still finds it. A run is consecutive versions by the same writer
/// whose first and last save are at most [`SAVE_RUN_SECS`] apart. The folded
/// entry is the run's last version; `first_version` names where it starts and
/// `changed_sentences` counts against the version before the run. `keep` are
/// versions that something else in the history points to (a version taken
/// from a shared folder names the one edited here); a run never hides them.
pub fn fold_save_runs(
    mut items: Vec<PageHistoryItem>,
    rows: &[PageHistoryEntry],
    keep: &HashSet<i64>,
) -> Vec<PageHistoryItem> {
    items.sort_by_key(|item| item.version.unwrap_or(0));
    let mut out: Vec<PageHistoryItem> = Vec::with_capacity(items.len());
    // Where the open run starts: its first version and when that was saved.
    let mut run: Option<(i64, i64)> = None;
    for item in items {
        let joins = match (out.last(), run, item.version) {
            (Some(last), Some((_, started)), Some(version)) => {
                folds(item.edited_by.as_deref())
                    && last.edited_by == item.edited_by
                    && last.agent == item.agent
                    && last.version == Some(version - 1)
                    && !keep.contains(&(version - 1))
                    && !keep.contains(&version)
                    && item.at - started <= SAVE_RUN_SECS
            }
            _ => false,
        };
        if joins {
            let (first, _) = run.expect("a joined run is open");
            let before = first - 1;
            let mut merged = item;
            merged.first_version = Some(first);
            merged.changed_sentences = changed_between(rows, before, merged.version);
            *out.last_mut().expect("a joined run has an entry") = merged;
        } else {
            run = match (folds(item.edited_by.as_deref()), item.version) {
                (true, Some(version)) => Some((version, item.at)),
                _ => None,
            };
            out.push(item);
        }
    }
    out
}

/// Sentences changed from `before` to `after`; the page's empty start when
/// `before` is 0. `None` when either text is no longer kept.
fn changed_between(rows: &[PageHistoryEntry], before: i64, after: Option<i64>) -> Option<u32> {
    let after = rows.iter().find(|row| Some(row.version) == after)?;
    let before = if before <= 0 {
        ""
    } else {
        rows.iter()
            .find(|row| row.version == before)?
            .content
            .as_str()
    };
    Some(crate::authorship::changed_sentence_count(
        before,
        &after.content,
    ))
}

/// Changelog writers that are the person: the editor, an edit of the file,
/// and putting back an archived page.
pub fn kind_of_edit(edited_by: &str) -> PageHistoryKind {
    match edited_by {
        "manual_edit" | "fs_edit" | "restore" | "person_create" => PageHistoryKind::PersonEdit,
        // The person on another of their own computers.
        crate::shared_space::OWN_DEVICE_PERSON => PageHistoryKind::PersonEdit,
        // Another member of a shared Space, in person.
        _ if matches!(
            crate::shared_space::shared_writer(edited_by),
            Some((crate::shared_space::version::AuthorKind::Person, _))
        ) =>
        {
            PageHistoryKind::PersonEdit
        }
        _ => PageHistoryKind::AiEdit,
    }
}

/// How many sentences the write that produced `version` added, changed or
/// removed against the version before it. `rows` is the page's history,
/// newest first. `None` when `version` or the one before it is not in `rows`
/// (the first version counts every sentence as added).
pub fn changed_sentences_at(rows: &[PageHistoryEntry], version: i64) -> Option<u32> {
    let at = rows.iter().position(|row| row.version == version)?;
    let before = match rows.get(at + 1) {
        Some(row) => row.content.as_str(),
        None if version <= 1 => "",
        None => return None,
    };
    Some(crate::authorship::changed_sentence_count(
        before,
        &rows[at].content,
    ))
}

fn texts_by_version(rows: &[PageHistoryEntry]) -> HashMap<i64, &str> {
    rows.iter()
        .map(|row| (row.version, row.content.as_str()))
        .collect()
}

/// Puts each version's text on its changelog entry.
pub async fn attach_texts(
    db: &MemoryDB,
    page_id: &str,
    entries: &mut [PageChangelogEntry],
) -> Result<(), WenlanError> {
    let rows = db.list_page_history(page_id, HISTORY_SCAN).await?;
    let texts = texts_by_version(&rows);
    for entry in entries {
        entry.text = texts.get(&entry.version).map(|t| (*t).to_string());
    }
    Ok(())
}

/// The ids of the pages `page_id` relies on.
pub async fn basis_page_ids(db: &MemoryDB, page_id: &str) -> Result<Vec<String>, WenlanError> {
    Ok(db
        .list_page_basis(page_id)
        .await?
        .into_iter()
        .map(|basis| basis.basis_page_id)
        .collect())
}

/// The merged history, newest first. `entries` is the page's changelog (with
/// texts already attached when `include_text`); `visible_bases` are the
/// relied-on pages this caller may see; updates of any other are left out.
pub async fn build_page_history(
    db: &MemoryDB,
    page_id: &str,
    entries: &[PageChangelogEntry],
    visible_bases: &HashSet<String>,
    include_text: bool,
) -> Result<Vec<PageHistoryItem>, WenlanError> {
    let own = db.list_page_history(page_id, HISTORY_SCAN).await?;
    let mut items: Vec<PageHistoryItem> = entries
        .iter()
        .map(|entry| PageHistoryItem {
            changed_sentences: changed_sentences_at(&own, entry.version),
            kind: kind_of_edit(&entry.edited_by),
            at: entry.at,
            version: Some(entry.version),
            edited_by: Some(entry.edited_by.clone()),
            agent: entry.agent.clone(),
            delta_summary: entry.delta_summary.clone(),
            text: entry.text.clone(),
            source: None,
            person_asked: entry.person_asked.clone(),
            restored_from: entry.restored_from,
            first_version: None,
        })
        .collect();

    // The changelog starts empty, so the page's first version has no entry.
    // It is listed as the page's birth, so the first edit has a version to
    // be compared with. Who made it decides the kind: the person, or else
    // the AI (an older `create` row whose maker is not known is the AI's
    // kind too; its `edited_by` stays `create`).
    if !entries.iter().any(|entry| entry.version == 1) {
        if let Some(birth) = own.iter().find(|row| row.version == 1) {
            let person = match db.get_page(page_id).await? {
                Some(page) => {
                    crate::sentences::first_version_author(&page, &own)
                        == Some(crate::authorship::VersionAuthor::Person)
                }
                None => false,
            };
            items.push(PageHistoryItem {
                kind: if person {
                    PageHistoryKind::PersonEdit
                } else {
                    PageHistoryKind::AiEdit
                },
                at: birth.created_at,
                version: Some(1),
                edited_by: Some(birth.edited_by.clone()),
                agent: None,
                delta_summary: None,
                text: include_text.then(|| birth.content.clone()),
                source: None,
                changed_sentences: changed_sentences_at(&own, 1),
                person_asked: None,
                restored_from: None,
                first_version: None,
            });
        }
    }

    // This page's version at a moment: the newest edit at or before it.
    let mut edits: Vec<(i64, i64)> = items
        .iter()
        .filter_map(|item| Some((item.at, item.version?)))
        .collect();
    edits.sort_unstable();
    let version_at = |at: i64| {
        edits
            .iter()
            .take_while(|(edit_at, _)| *edit_at <= at)
            .last()
            .map(|(_, version)| *version)
    };
    // A version taken from a shared folder names the one edited here; both
    // stay their own entries so "see mine" can open it.
    let keep: HashSet<i64> = entries
        .iter()
        .filter_map(|entry| entry.concurrent_with.map(|other| [entry.version, other]))
        .flatten()
        .collect();
    let mut items = fold_save_runs(items, &own, &keep);

    for basis in db.list_page_basis(page_id).await? {
        if !visible_bases.contains(&basis.basis_page_id) {
            continue;
        }
        let rows = db
            .list_page_history(&basis.basis_page_id, HISTORY_SCAN)
            .await?;
        // A version this page has not been checked against yet is always an
        // update; an older one counts when it was written after this page
        // started relying on the source.
        for row in rows
            .iter()
            .filter(|row| row.version > basis.confirmed_version || row.created_at > basis.added_at)
        {
            items.push(PageHistoryItem {
                kind: PageHistoryKind::SourceUpdate,
                at: row.created_at,
                version: version_at(row.created_at),
                edited_by: Some(row.edited_by.clone()),
                agent: None,
                delta_summary: None,
                text: None,
                source: Some(PageHistorySource {
                    page_id: basis.basis_page_id.clone(),
                    title: basis.title.clone(),
                    version: row.version,
                    relied_on_version: basis.confirmed_version,
                    pending: row.version > basis.confirmed_version,
                    text: include_text.then(|| row.content.clone()),
                }),
                changed_sentences: None,
                person_asked: None,
                restored_from: None,
                first_version: None,
            });
        }
    }

    items.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| b.version.unwrap_or(0).cmp(&a.version.unwrap_or(0)))
    });
    Ok(items)
}

#[cfg(test)]
#[path = "page_history_test.rs"]
mod tests;
