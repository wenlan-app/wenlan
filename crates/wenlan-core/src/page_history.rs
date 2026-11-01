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

/// Changelog writers that are the person: the editor, an edit of the file,
/// and putting back an archived page.
pub fn kind_of_edit(edited_by: &str) -> PageHistoryKind {
    match edited_by {
        "manual_edit" | "fs_edit" | "restore" | "person_create" => PageHistoryKind::PersonEdit,
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
        })
        .collect();

    // This page's version at a moment: the newest edit at or before it.
    let mut edits: Vec<(i64, i64)> = entries.iter().map(|e| (e.at, e.version)).collect();
    edits.sort_unstable();
    let version_at = |at: i64| {
        edits
            .iter()
            .take_while(|(edit_at, _)| *edit_at <= at)
            .last()
            .map(|(_, version)| *version)
    };

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
