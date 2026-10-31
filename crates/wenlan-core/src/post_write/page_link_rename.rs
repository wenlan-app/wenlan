// SPDX-License-Identifier: Apache-2.0
//! Renaming a page updates the `[[links]]` other pages make to it.
//!
//! The title change commits first, in its own transaction
//! (`MemoryDB::commit_page_title_rename`). The link rewrite runs right after it, one
//! ordinary page write per linking page (`page_write`, writer `link_rename`),
//! each under the version and source-revision fence of the body it was
//! computed from. So every page ends either at its old body or at its old body
//! with only link targets changed, never part-way, and each rewrite gets the
//! normal version bump, history row, changelog entry, link-graph refresh and
//! `.md` projection.
//!
//! Which pages, and which links: the active `links` edges into the renamed
//! page. They hold the link identity by page id, as resolved when the linking
//! page was last saved, so this rewrites exactly the links the backlinks panel
//! shows, and nothing that resolved to another page (an ambiguous title, or the
//! same title in another Space, never has an edge here). The edges keep
//! pointing at the page after the title changes, which makes the step
//! convergent: running it again only rewrites what still names an old title,
//! and a repeated rename request with the same title runs it again.
//!
//! A page locked for review gets a revision card instead of the rewrite. The
//! card names the title the page had when it was staged, so a later rename
//! of the same page (or a rename back) first removes the cards an earlier
//! rename left for its links, then stages the one that is right now, if any,
//! so only a correct card waits for review.

use super::{page_dispatch::page_write, PageWrite, WriteOutcome};
use crate::{db::MemoryDB, error::WenlanError, read_scope::ReadScope};
use std::path::Path;
use wenlan_types::requests::UpdatePageRequest;

/// The `edited_by` a link rewrite records in history and the changelog.
pub(crate) const LINK_RENAME_WRITER: &str = "link_rename";

/// What happened to the pages that link to a renamed page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LinkRenameCounts {
    pub(crate) updated: u32,
    pub(crate) pending_review: u32,
    pub(crate) not_updated: u32,
}

/// The target a link label should name now that the page is called
/// `new_title`, or `None` when the label already names it.
///
/// A label written as `[[Space/Title]]` keeps its Space prefix as written. A
/// label is read as qualified only when it is not itself the old title, so a
/// title such as "CI/CD" in a Space called "CI" stays a bare title.
fn renamed_label(
    label: &str,
    new_title: &str,
    previous_title: Option<&str>,
    space_name: Option<&str>,
) -> Option<String> {
    let key = MemoryDB::page_title_key(label);
    if key == MemoryDB::page_title_key(new_title) {
        return None;
    }
    let names_previous = |text: &str| {
        previous_title
            .is_none_or(|old| MemoryDB::page_title_key(text) == MemoryDB::page_title_key(old))
    };
    if previous_title.is_some_and(|old| MemoryDB::page_title_key(old) == key) {
        return Some(new_title.to_string());
    }
    if let Some(space) = space_name {
        let space_key = MemoryDB::page_title_key(space);
        for (at, _) in label.rmatch_indices('/') {
            let (prefix, rest) = (&label[..at], &label[at + 1..]);
            if MemoryDB::page_title_key(prefix) == space_key
                && !rest.trim().is_empty()
                && names_previous(rest)
            {
                if MemoryDB::page_title_key(rest) == MemoryDB::page_title_key(new_title) {
                    return None;
                }
                return Some(format!("{}/{new_title}", prefix.trim_end()));
            }
        }
    }
    // A label the edge still ties to this page from an earlier title (a
    // rename made before links were updated) names the page bare.
    Some(new_title.to_string())
}

/// Rewrite the links other pages make to `page_id`, now titled as stored.
/// Only pages in the same Space that `scope` can see are written; pages
/// elsewhere that still link by an old title are counted, never touched.
pub(crate) async fn update_links_to_renamed_page(
    db: &MemoryDB,
    page_id: &str,
    previous_title: Option<&str>,
    scope: &ReadScope,
    knowledge_path: Option<&Path>,
) -> Result<LinkRenameCounts, WenlanError> {
    let mut counts = LinkRenameCounts::default();
    let Some(target) = db.get_page(page_id).await? else {
        return Ok(counts);
    };
    let new_title = target.title.clone();
    let space_name = target
        .space
        .as_deref()
        .filter(|space| *space != crate::db::UNFILED_SPACE_ID);
    // A title that cannot be written inside `[[...]]` cannot be linked to;
    // leave every link as it is and report them.
    let linkable = !new_title.contains(crate::synthesis::wikilinks::WIKILINK_TARGET_BREAKERS);

    for source in db.rename_link_sources(page_id).await? {
        // A page the caller cannot see is neither written nor counted.
        if !scope.matches(source.workspace.as_deref()) {
            continue;
        }
        let rewrite = |content: &str| {
            crate::synthesis::wikilinks::rewrite_wikilink_targets(content, |target| {
                let key = MemoryDB::page_title_key(target);
                source
                    .labels
                    .iter()
                    .any(|label| MemoryDB::page_title_key(label) == key)
                    .then(|| renamed_label(target, &new_title, previous_title, space_name))
                    .flatten()
            })
        };
        // Fence before reading: a source attached after this read moves the
        // revision, so the write below is refused rather than dropping it.
        let source_revision = db.get_page_source_revision(&source.page_id).await?;
        let Some(page) = db.get_page(&source.page_id).await? else {
            continue;
        };
        // A card an earlier rename staged here names a title this page no
        // longer has. Remove it before deciding what this page needs now.
        if let Err(error) = dismiss_superseded_link_cards(db, &page, &source.labels).await {
            log::warn!(
                "[page_link_rename] {} kept an earlier link card for {page_id}: {error}",
                page.id
            );
        }
        let (content, changed) = rewrite(&page.content);
        if changed == 0 {
            continue;
        }
        if !source.same_space || !source.rewritable || !linkable {
            counts.not_updated += 1;
            continue;
        }
        // The same automatic-reader permit the rename itself needs.
        if crate::truth_adapter::page_write_permit(db, &page.id)
            .await?
            .is_none()
        {
            counts.not_updated += 1;
            continue;
        }
        let written = page_write(
            db,
            PageWrite::Update {
                page_id: &page.id,
                req: UpdatePageRequest {
                    content,
                    source_memory_ids: page.source_memory_ids.clone(),
                    expected_version: Some(page.version),
                    caller_id: None,
                    operation_id: None,
                },
                edited_by: LINK_RENAME_WRITER,
                require_stale: false,
                expected_source_revision: Some(source_revision),
                knowledge_path,
                citations: None,
            },
        )
        .await;
        match written {
            Ok(result) if result.outcome == WriteOutcome::Wrote => counts.updated += 1,
            Ok(result) if result.outcome == WriteOutcome::Gated => counts.pending_review += 1,
            Ok(result) => {
                log::info!(
                    "[page_link_rename] {} kept its links to {page_id}: {:?}",
                    page.id,
                    result.outcome
                );
                counts.not_updated += 1;
            }
            Err(error) => {
                log::warn!(
                    "[page_link_rename] {} kept its links to {page_id}: {error}",
                    page.id
                );
                counts.not_updated += 1;
            }
        }
    }
    Ok(counts)
}

/// Remove the pending link cards on `page` that a rename of the page its
/// `labels` link to staged. Such a card proposes `page` as it is with only
/// those links' targets changed, so it is found by comparing link targets in
/// order: a card that changes a link whose target is one of `labels` is for
/// this renamed page. A card for another renamed page changes other links and
/// stays. A card staged before `page` changed shape is left for its own
/// version check to refuse.
async fn dismiss_superseded_link_cards(
    db: &MemoryDB,
    page: &crate::pages::Page,
    labels: &[String],
) -> Result<(), WenlanError> {
    let cards = db
        .pending_page_cards_by_writer(&page.id, LINK_RENAME_WRITER)
        .await?;
    if cards.is_empty() {
        return Ok(());
    }
    let current = card_link_targets(&page.content);
    for (card_id, proposed) in cards {
        if card_changes_links_to(&current, &card_link_targets(&proposed), labels) {
            db.delete_by_source_id("memory", &card_id).await?;
        }
    }
    Ok(())
}

/// The targets of the links `rewrite_wikilink_targets` would consider, in
/// document order.
fn card_link_targets(content: &str) -> Vec<String> {
    let mut targets = Vec::new();
    crate::synthesis::wikilinks::rewrite_wikilink_targets(content, |target| {
        targets.push(target.to_string());
        None
    });
    targets
}

fn card_changes_links_to(current: &[String], proposed: &[String], labels: &[String]) -> bool {
    current.len() == proposed.len()
        && current.iter().zip(proposed).any(|(now, card)| {
            let key = MemoryDB::page_title_key(now);
            key != MemoryDB::page_title_key(card)
                && labels
                    .iter()
                    .any(|label| MemoryDB::page_title_key(label) == key)
        })
}

#[cfg(test)]
mod tests {
    use super::{card_changes_links_to, card_link_targets, renamed_label};

    #[test]
    fn bare_old_title_becomes_new_title() {
        assert_eq!(
            renamed_label("old  ", "New", Some("Old"), Some("work")),
            Some("New".to_string())
        );
    }

    #[test]
    fn space_prefix_is_kept_as_written() {
        assert_eq!(
            renamed_label("Work/Old", "New", Some("Old"), Some("work")),
            Some("Work/New".to_string())
        );
    }

    #[test]
    fn a_title_with_a_slash_is_not_read_as_a_space_prefix() {
        assert_eq!(
            renamed_label("CI/CD", "Pipelines", Some("CI/CD"), Some("CI")),
            Some("Pipelines".to_string())
        );
    }

    #[test]
    fn a_label_already_naming_the_page_is_left_alone() {
        assert_eq!(renamed_label("new", "New", Some("Old"), None), None);
        assert_eq!(renamed_label("work/new", "New", None, Some("work")), None);
    }

    #[test]
    fn a_card_is_matched_to_the_links_it_changes() {
        let labels = vec!["Old".to_string()];
        let current = card_link_targets("See [[Old]] and [[Other|x]]. `[[Old]]`");
        assert_eq!(current, vec!["Old", "Other"]);
        assert!(card_changes_links_to(
            &current,
            &card_link_targets("See [[Mid]] and [[Other|x]]. `[[Old]]`"),
            &labels
        ));
        // A card for another renamed page changes only that page's links.
        assert!(!card_changes_links_to(
            &current,
            &card_link_targets("See [[Old]] and [[Renamed|x]]. `[[Old]]`"),
            &labels
        ));
        // A card from before the page changed shape is not matched.
        assert!(!card_changes_links_to(
            &current,
            &card_link_targets("See [[Mid]]."),
            &labels
        ));
    }

    #[test]
    fn a_label_from_an_earlier_title_names_the_page_bare() {
        assert_eq!(
            renamed_label("Older", "New", Some("Old"), Some("work")),
            Some("New".to_string())
        );
    }
}
