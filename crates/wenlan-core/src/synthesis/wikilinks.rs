// SPDX-License-Identifier: Apache-2.0
//! Wikilink graph for distilled pages.
//!
//! Pages are written with `[[Label]]` link syntax. The distill prompt emits
//! them, users write them by hand. The daemon's job is to:
//!   1. Pull `[[Label]]` occurrences out of a page body.
//!   2. Resolve each label against current page titles (case-insensitive,
//!      trimmed). No fuzzy matching in v1 — the LLM's job is to use exact
//!      titles, and the orphan-by-label feed surfaces drift. A label first
//!      names a note in the linking page's own Space; only when none carries
//!      that title does `[[Space/Title]]` name a note in another Space. When
//!      neither reading resolves, a bare title reaches the one note with that
//!      title across all other Spaces plus the no-Space scope; two owners
//!      elsewhere leave it unresolved.
//!   3. Persist resolved/unresolved pairs into the `page_links` table so the
//!      refinery can re-resolve later (when a target page is created) and
//!      emergence can mine repeated orphan labels as topic candidates.
//!
//! Parsing delegates to `sources::obsidian::extract_wikilinks` — that module
//! already handles code-block exclusion, aliased links, and heading anchors
//! correctly. Here we strip the structural details (heading / display /
//! embed flag) down to the target label, since the link's identity for
//! graph purposes is the target, not the display text.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::sources::obsidian;
use std::collections::{HashMap, HashSet};

/// A wikilink reference stored in or read from the `page_links` table.
/// `target_page_id` is `None` when the resolver couldn't find a matching
/// page title — those rows persist with NULL target so the refinery's
/// orphan-resolve phase can pick them up later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    pub label: String,
    pub target_page_id: Option<String>,
}

/// Pull every `[[Label]]` reference out of `content`, returning labels in
/// document order, deduplicated case-insensitively. Skips embeds (`![[...]]`,
/// reserved for transclusion) and links inside fenced code blocks. The
/// obsidian regex naturally rejects targets containing `]`, `|`, or `#`
/// so malformed input like `[[[[foo]]]]` or `[[foo]bar]]` produces nothing.
pub fn extract_wikilinks(content: &str) -> Vec<String> {
    let parsed = obsidian::extract_wikilinks(content);
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for link in parsed {
        if link.is_embed {
            continue;
        }
        let label = link.target.trim().to_string();
        if label.is_empty() {
            continue;
        }
        // Obsidian's regex accepts `[` inside the target since `[^\]|#]+`
        // doesn't exclude it — so `[[[[foo]]]]` captures `[[foo`. Reject
        // labels that carry stray brackets; they'd poison page_links and
        // the orphan-by-count feed.
        if label.contains('[') || label.contains(']') {
            continue;
        }
        let key = label.to_lowercase();
        if seen.insert(key) {
            out.push(label);
        }
    }
    out
}

/// Folded title to its unique owner page (`None`: ambiguous), as built by the
/// database's active non-entity owner scan.
pub(crate) type TitleOwners = HashMap<String, Option<String>>;

/// Title owners keyed by Space scope (`None`: the pages in no Space).
pub(crate) type OwnersBySpace = HashMap<Option<String>, TitleOwners>;

/// The `(Space name, title)` readings of a `[[Space/Title]]` label, longest
/// Space prefix first. A prefix counts only when it names exactly one Space,
/// so a title such as "CI/CD" has no reading unless a Space is called "CI".
pub(crate) fn space_qualified_readings<'a>(
    label: &'a str,
    space_names: &TitleOwners,
) -> Vec<(String, &'a str)> {
    label
        .rmatch_indices('/')
        .filter_map(|(at, _)| {
            let title = label[at + 1..].trim();
            if title.is_empty() {
                return None;
            }
            let space = space_names
                .get(&MemoryDB::page_title_key(&label[..at]))?
                .clone()?;
            Some((space, title))
        })
        .collect()
}

/// True when `label` spells `[[space_name/title]]`, so a link that reached a
/// note in another Space did so by naming it there.
pub(crate) fn label_names_space_note(label: &str, space_name: &str, title: &str) -> bool {
    let space_key = MemoryDB::page_title_key(space_name);
    let title_key = MemoryDB::page_title_key(title);
    label.rmatch_indices('/').any(|(at, _)| {
        MemoryDB::page_title_key(&label[..at]) == space_key
            && MemoryDB::page_title_key(&label[at + 1..]) == title_key
    })
}

/// Resolve `label` written in a page whose Space is `scope`. A title in the
/// page's own Space wins, even an ambiguous one; otherwise the first
/// `[[Space/Title]]` reading that names a unique note in that Space. Last, the
/// literal label resolves when exactly one note across every other scope owns
/// it. `owners` must contain every scope for that fallback; a missing map for
/// a named Space reading reads as no match.
pub(crate) fn resolve_label(
    label: &str,
    scope: Option<&str>,
    owners: &OwnersBySpace,
    space_names: &TitleOwners,
) -> Option<String> {
    let own_scope = scope
        .filter(|space| *space != crate::db::UNFILED_SPACE_ID)
        .map(str::to_string);
    let label_key = MemoryDB::page_title_key(label);
    if let Some(owner) = owners.get(&own_scope).and_then(|own| own.get(&label_key)) {
        return owner.clone();
    }
    if let Some(owner) = space_qualified_readings(label, space_names)
        .into_iter()
        .find_map(|(space, title)| {
            owners
                .get(&Some(space))?
                .get(&MemoryDB::page_title_key(title))?
                .clone()
        })
    {
        return Some(owner);
    }

    let mut elsewhere = None;
    for (candidate_scope, titles) in owners {
        if candidate_scope == &own_scope {
            continue;
        }
        match titles.get(&label_key) {
            Some(Some(owner)) if elsewhere.is_none() => elsewhere = Some(owner.clone()),
            Some(_) => return None,
            None => {}
        }
    }
    elsewhere
}

/// Resolve each extracted label against the current `pages` table.
/// Case-insensitive exact match on title, restricted to `status='active'`.
/// Unresolved labels keep `target_page_id = None` so the caller can persist
/// them as orphan rows.
pub async fn resolve_against_pages(
    db: &MemoryDB,
    labels: &[String],
    scope: Option<&str>,
) -> Result<Vec<Wikilink>, WenlanError> {
    if labels.is_empty() {
        return Ok(Vec::new());
    }
    let qualified = labels.iter().any(|label| label.contains('/'));
    // The common single-link page takes the keep-nothing scan instead of
    // paying for a full owner map it would use once.
    if let ([label], false) = (labels, qualified) {
        return Ok(vec![Wikilink {
            label: label.clone(),
            target_page_id: db
                .find_unique_active_page_id_by_title_scoped(label, scope)
                .await?,
        }]);
    }
    // One all-scope scan for the whole label set — the fold and both the
    // same-scope and elsewhere ambiguity contracts live in the owner maps.
    let owners = db.folded_title_owners_by_scope().await?;
    let space_names = if qualified {
        db.folded_space_names().await?
    } else {
        TitleOwners::new()
    };
    Ok(labels
        .iter()
        .map(|label| Wikilink {
            label: label.clone(),
            target_page_id: resolve_label(label, scope, &owners, &space_names),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_link() {
        let v = extract_wikilinks("hello [[Rust Ownership]] world");
        assert_eq!(v, vec!["Rust Ownership"]);
    }

    #[test]
    fn extract_multiple_links_preserve_order() {
        let v = extract_wikilinks("[[Alpha]] then [[Beta]] then [[Gamma]]");
        assert_eq!(v, vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn extract_dedups_case_insensitively() {
        let v = extract_wikilinks("[[Wenlan]] and [[wenlan]] and [[WENLAN]]");
        // Keep first-seen casing.
        assert_eq!(v, vec!["Wenlan"]);
    }

    #[test]
    fn extract_handles_aliased_links() {
        // Obsidian regex captures `target` as group 2; display text is
        // stripped here.
        let v = extract_wikilinks("see [[Rust Ownership|borrowing rules]] for more");
        assert_eq!(v, vec!["Rust Ownership"]);
    }

    #[test]
    fn extract_strips_heading_anchor() {
        // `[[Page#Section]]` reduces to the target page.
        let v = extract_wikilinks("see [[Rust Ownership#borrowing]] below");
        assert_eq!(v, vec!["Rust Ownership"]);
    }

    #[test]
    fn extract_skips_embeds() {
        // Embeds (`![[...]]`) are for transclusion — out of scope for the
        // graph. Plain `[[...]]` is the citation link.
        let v = extract_wikilinks("![[Diagram]] then [[Real Link]]");
        assert_eq!(v, vec!["Real Link"]);
    }

    #[test]
    fn extract_skips_code_block_wikilinks() {
        let v = extract_wikilinks("real [[A]]\n\n```\n[[NotALink]]\n```\n\nmore [[B]]");
        assert_eq!(v, vec!["A", "B"]);
    }

    #[test]
    fn extract_handles_utf8_payload() {
        let v = extract_wikilinks("café [[Naïve Bayes]] résumé");
        assert_eq!(v, vec!["Naïve Bayes"]);
    }

    #[test]
    fn extract_returns_empty_for_no_links() {
        let v = extract_wikilinks("plain prose with [no brackets] only");
        assert!(v.is_empty());
    }

    #[test]
    fn extract_rejects_nested_brackets() {
        // `[[[[foo]]]]` — obsidian regex captures target `[[foo` (it allows
        // `[` inside the target group). The bracket post-filter drops it.
        let v = extract_wikilinks("noise [[[[foo]]]] more");
        assert!(v.is_empty());
    }

    #[test]
    fn extract_rejects_stray_closing_bracket() {
        // `[[foo]bar]]` — obsidian regex fails to find a complete
        // `[[...]]` since target stops at the inner `]` but `\]\]` isn't
        // there immediately after. Nothing captured.
        let v = extract_wikilinks("noise [[foo]bar]] more");
        assert!(v.is_empty());
    }

    #[test]
    fn extract_keeps_well_formed_after_garbage() {
        // Bracket post-filter drops the captured `[[foo`; real link survives.
        let v = extract_wikilinks("[[[[foo]]]] and [[Real Link]] keep going");
        assert_eq!(v, vec!["Real Link"]);
    }
}
