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
//! so only a correct card waits for review. Each card records the renamed
//! page and the digest of the body it staged; only a card that still holds
//! exactly that body is removed, never one the person edited.

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
        let titles = CardTitles {
            new_title: &new_title,
            previous_title,
            space_name,
        };
        if let Err(error) =
            dismiss_superseded_link_cards(db, page_id, &page, &source.labels, &titles).await
        {
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
        write_link_rewrite(
            db,
            &page,
            content,
            source_revision,
            knowledge_path,
            Some(page_id),
            &mut counts,
        )
        .await?;
    }
    Ok(counts)
}

/// The headings of `before` that `after` renamed, as `(old, new)` text.
///
/// A rename is read only where it is certain: both bodies have the same
/// headings in the same order and at the same levels, and at a place where
/// the words differ, the old words appear nowhere in `after` and the new
/// words nowhere in `before`. Headings added, removed or swapped are not
/// renames, so their links are left as written.
pub(crate) fn renamed_headings(before: &str, after: &str) -> Vec<(String, String)> {
    let old = atx_headings(before);
    let new = atx_headings(after);
    if old.len() != new.len() || old.iter().zip(&new).any(|(a, b)| a.0 != b.0) {
        return Vec::new();
    }
    let key = |text: &str| MemoryDB::page_title_key(text);
    let mut renames: Vec<(String, String)> = Vec::new();
    for ((_, was), (_, now)) in old.iter().zip(&new) {
        if key(was) == key(now) {
            continue;
        }
        if new.iter().any(|(_, text)| key(text) == key(was))
            || old.iter().any(|(_, text)| key(text) == key(now))
        {
            continue;
        }
        if !renames.iter().any(|(from, _)| key(from) == key(was)) {
            renames.push((was.clone(), now.clone()));
        }
    }
    renames
}

/// `(level, text)` of each ATX heading outside fenced code.
fn atx_headings(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent <= 3 {
            let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
            if let Some(marker) = marker {
                match fence {
                    Some(open) if open == marker => fence = None,
                    None => fence = Some(marker),
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() || indent > 3 {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) {
            continue;
        }
        let rest = &trimmed[level..];
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            continue;
        }
        let text = rest.trim().trim_end_matches('#').trim();
        if !text.is_empty() {
            out.push((level, text.to_string()));
        }
    }
    out
}

/// Rewrite `[[Page#old]]` to `[[Page#new]]` in the pages that link to
/// `page_id`, for each heading the page's last edit renamed. The same pages,
/// fences and review rule as a page rename: only pages in the same Space that
/// `scope` can see are written, and a page locked for review gets a card.
pub(crate) async fn update_links_to_renamed_headings(
    db: &MemoryDB,
    page_id: &str,
    renames: &[(String, String)],
    scope: &ReadScope,
    knowledge_path: Option<&Path>,
) -> Result<LinkRenameCounts, WenlanError> {
    let mut counts = LinkRenameCounts::default();
    if renames.is_empty() {
        return Ok(counts);
    }
    for source in db.rename_link_sources(page_id).await? {
        if !scope.matches(source.workspace.as_deref()) {
            continue;
        }
        let rewrite = |content: &str| {
            crate::synthesis::wikilinks::rewrite_wikilink_headings(content, |target, heading| {
                let target = MemoryDB::page_title_key(target);
                if !source
                    .labels
                    .iter()
                    .any(|label| MemoryDB::page_title_key(label) == target)
                {
                    return None;
                }
                let heading = MemoryDB::page_title_key(heading);
                renames
                    .iter()
                    .find(|(old, _)| MemoryDB::page_title_key(old) == heading)
                    .map(|(_, new)| new.clone())
            })
        };
        let source_revision = db.get_page_source_revision(&source.page_id).await?;
        let Some(page) = db.get_page(&source.page_id).await? else {
            continue;
        };
        let (content, changed) = rewrite(&page.content);
        if changed == 0 {
            continue;
        }
        if !source.same_space || !source.rewritable {
            counts.not_updated += 1;
            continue;
        }
        write_link_rewrite(
            db,
            &page,
            content,
            source_revision,
            knowledge_path,
            None,
            &mut counts,
        )
        .await?;
    }
    Ok(counts)
}

/// Write `content`, a body of `page` that differs only in link targets or
/// headings, under the version and source-revision fence it was read at,
/// and count what happened. `tag` names the renamed page on a review card,
/// so a later rename of it can find and replace the card.
async fn write_link_rewrite(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: String,
    source_revision: i64,
    knowledge_path: Option<&Path>,
    tag: Option<&str>,
    counts: &mut LinkRenameCounts,
) -> Result<(), WenlanError> {
    // The same automatic-reader permit the rename itself needs.
    if crate::truth_adapter::page_write_permit(db, &page.id)
        .await?
        .is_none()
    {
        counts.not_updated += 1;
        return Ok(());
    }
    let citations = carried_citations(page, &content);
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
            citations,
        },
    )
    .await;
    match written {
        Ok(result) if result.outcome == WriteOutcome::Wrote => counts.updated += 1,
        Ok(result) if result.outcome == WriteOutcome::Gated => {
            counts.pending_review += 1;
            // Untagged, the card is still found by its links (as a card
            // from 0.20.5 is), just less surely.
            if let (Some(card_id), Some(page_id)) = (result.revision_card_id.as_deref(), tag) {
                if let Err(error) = db.tag_link_rename_card(card_id, page_id).await {
                    log::warn!(
                        "[page_link_rename] link card {card_id} on {} left untagged: {error}",
                        page.id
                    );
                }
            }
        }
        Ok(result) => {
            log::info!(
                "[page_link_rename] {} kept its old links: {:?}",
                page.id,
                result.outcome
            );
            counts.not_updated += 1;
        }
        Err(error) => {
            log::warn!("[page_link_rename] {} kept its old links: {error}", page.id);
            counts.not_updated += 1;
        }
    }
    Ok(())
}

/// The page's citation records for a body that differs only in link targets.
/// Records are kept by marker position, not by text, and the sentences they
/// support did not change, so clearing them here would lose them for nothing.
/// `None`, which clears them, when the rewrite moved a marker (a page titled
/// `1` is linked as `[[1]]`, which reads as a marker) or the page has none.
fn carried_citations(page: &crate::pages::Page, rewritten: &str) -> Option<(String, String)> {
    if page.citations.is_empty() {
        return None;
    }
    let markers = |body: &'_ str| -> Vec<String> {
        crate::citations::marker_spans(body)
            .into_iter()
            .map(|(start, end)| body[start..end].to_string())
            .collect()
    };
    if markers(&page.content) != markers(rewritten) {
        return None;
    }
    let json = serde_json::to_string(&page.citations).ok()?;
    Some((json, "citations kept across a link update".to_string()))
}

/// The titles a link card from an earlier rename of the page can name.
struct CardTitles<'a> {
    new_title: &'a str,
    previous_title: Option<&'a str>,
    space_name: Option<&'a str>,
}

impl CardTitles<'_> {
    /// Whether `target` names the page by the title it had before this
    /// rename (what the previous rename's card proposed) or by its new one (a
    /// replay), bare or as `Space/Title`.
    fn names_page(&self, target: &str) -> bool {
        let names = |title: &str| {
            let key = MemoryDB::page_title_key(title);
            MemoryDB::page_title_key(target) == key
                || self.space_name.is_some_and(|space| {
                    target.rmatch_indices('/').any(|(at, _)| {
                        MemoryDB::page_title_key(&target[..at]) == MemoryDB::page_title_key(space)
                            && MemoryDB::page_title_key(&target[at + 1..]) == key
                    })
                })
        };
        names(self.new_title) || self.previous_title.is_some_and(names)
    }
}

/// Remove the pending link cards on `page` that an earlier rename of
/// `renamed_page_id` staged and nobody has touched since.
///
/// A card staged by this version records the renamed page and the digest of
/// the body it stores: it is removed only when the tag names this page and
/// the body still has that digest, so a card for another renamed page, or
/// one the person edited, stays. A card staged before cards were tagged
/// (0.20.5) is removed only when it is certainly this page's: staged against
/// the page's current version, and exactly its current body with only links
/// to this page retargeted to the title it had before this rename. When in
/// doubt a card stays; a stale card is the lesser harm.
///
/// The digest is re-checked in the same write transaction as the delete, so
/// an edit that lands between this read and the delete keeps the card.
async fn dismiss_superseded_link_cards(
    db: &MemoryDB,
    renamed_page_id: &str,
    page: &crate::pages::Page,
    labels: &[String],
    titles: &CardTitles<'_>,
) -> Result<(), WenlanError> {
    let cards = db
        .pending_page_cards_by_writer(&page.id, LINK_RENAME_WRITER)
        .await?;
    for (card_id, fields) in cards {
        let expected_digest = match fields.get("link_rename_page").and_then(|v| v.as_str()) {
            Some(tagged) if tagged == renamed_page_id => fields
                .get("link_rename_digest")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            Some(_) => None,
            None => {
                let staged_on_this_version =
                    fields.get("page_version").and_then(|v| v.as_i64()) == Some(page.version);
                match db.staged_card_body(&card_id).await? {
                    Some(body)
                        if staged_on_this_version
                            && untagged_card_is_for(&page.content, &body, labels, titles) =>
                    {
                        Some(crate::provenance::revision_content_digest(&body))
                    }
                    _ => None,
                }
            }
        };
        if let Some(digest) = expected_digest {
            db.delete_page_card_if_unchanged(&card_id, &digest).await?;
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

/// Whether an untagged link card proposes exactly `current` with only links
/// to the renamed page (those whose target is one of `labels`) retargeted,
/// each to a title `titles` says the page went by. Any other difference
/// (text the person edited, a link to another page changed, links
/// reordered, an unrelated title) means it is not provably this rename's
/// card, and it stays.
fn untagged_card_is_for(
    current: &str,
    card: &str,
    labels: &[String],
    titles: &CardTitles<'_>,
) -> bool {
    let now = card_link_targets(current);
    let proposed = card_link_targets(card);
    if now.len() != proposed.len() {
        return false;
    }
    let mut changed = false;
    for (now, proposed) in now.iter().zip(&proposed) {
        let key = MemoryDB::page_title_key(now);
        if key == MemoryDB::page_title_key(proposed) {
            continue;
        }
        if !labels
            .iter()
            .any(|label| MemoryDB::page_title_key(label) == key)
            || !titles.names_page(proposed)
        {
            return false;
        }
        changed = true;
    }
    let mut next = proposed.into_iter();
    let (rebuilt, _) =
        crate::synthesis::wikilinks::rewrite_wikilink_targets(current, |_| next.next());
    changed && rebuilt == card
}

#[cfg(test)]
mod tests {
    use super::{
        card_link_targets, renamed_headings, renamed_label, untagged_card_is_for, CardTitles,
    };

    #[test]
    fn a_heading_whose_words_changed_in_place_is_a_rename() {
        let before = "# A\n\ntext\n\n## Third chapter\n\n```\n# not a heading\n```\n";
        let after =
            "# A\n\ntext more\n\n## Third chapter, key points\n\n```\n# not a heading\n```\n";
        assert_eq!(
            renamed_headings(before, after),
            vec![(
                "Third chapter".to_string(),
                "Third chapter, key points".to_string()
            )]
        );
    }

    #[test]
    fn added_removed_or_swapped_headings_are_not_renames() {
        assert!(renamed_headings("# A\n# B", "# A\n# B\n# C").is_empty());
        assert!(renamed_headings("# A\n# B", "# A").is_empty());
        assert!(renamed_headings("# A\n# B", "# B\n# A").is_empty());
        assert!(renamed_headings("# A\n# B", "# A\n## C").is_empty());
        assert!(renamed_headings("# A", "# A").is_empty());
    }

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
    fn an_untagged_card_is_matched_only_when_certain() {
        let labels = vec!["Old".to_string()];
        let titles = CardTitles {
            new_title: "New",
            previous_title: Some("Mid"),
            space_name: Some("work"),
        };
        let is_for =
            |current: &str, card: &str| untagged_card_is_for(current, card, &labels, &titles);
        let current = "See [[Old]] and [[Other|x]]. `[[Old]]`";
        assert_eq!(card_link_targets(current), vec!["Old", "Other"]);
        assert!(is_for(current, "See [[Mid]] and [[Other|x]]. `[[Old]]`"));
        assert!(is_for(
            current,
            "See [[work/Mid]] and [[Other|x]]. `[[Old]]`"
        ));
        // A replay's card already names the new title.
        assert!(is_for(current, "See [[New]] and [[Other|x]]. `[[Old]]`"));
        // A card that retargets this page's links to a title the page did not
        // have just before this rename is not provably this rename's card.
        assert!(!is_for(
            current,
            "See [[Elsewhere]] and [[Other|x]]. `[[Old]]`"
        ));
        // A card for another renamed page changes only that page's links.
        assert!(!is_for(current, "See [[Old]] and [[Renamed|x]]. `[[Old]]`"));
        // A card the person edited is not this rename's card.
        assert!(!is_for(
            current,
            "See [[Mid]] and [[Other|x]], edited. `[[Old]]`"
        ));
        // Links reordered since the card: another page's card stays.
        assert!(!is_for("[[Old]] [[Other]]", "[[Other two]] [[Old]]"));
        // A card from before the page changed shape is not matched.
        assert!(!is_for(current, "See [[Mid]]."));
        // A card that changes nothing is not a link card for this page.
        assert!(!is_for(current, current));
    }

    #[test]
    fn a_label_from_an_earlier_title_names_the_page_bare() {
        assert_eq!(
            renamed_label("Older", "New", Some("Old"), Some("work")),
            Some("New".to_string())
        );
    }
}
