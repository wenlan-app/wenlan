// SPDX-License-Identifier: Apache-2.0
//! Knowledge compilation: pages are synthesized wiki entries distilled from memory clusters.

// Re-export the wire type from wenlan-types so existing consumers keep working.
pub use wenlan_types::pages::Page;

/// Result of replacing one complete Page draft snapshot.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum PageDraftUpdateOutcome {
    Updated(Page),
    VersionConflict { current_version: i64 },
}

/// Result of discarding a Page draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDraftDeleteOutcome {
    Deleted,
    VersionConflict { current_version: i64 },
}

/// Result of publishing a Page draft as an active Page.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum PageDraftPublishOutcome {
    Published(Page),
    VersionConflict {
        current_version: i64,
    },
    /// Another active Page in the same scope already carries this trimmed,
    /// case-insensitively equal title.
    TitleConflict {
        existing_page_id: String,
        existing_page_title: String,
        /// Stored (sentinel-mirrored) scope the conflict was found in. The
        /// disclosure recheck needs it: a page moved to another space after
        /// the publish transaction is no longer this conflict.
        scope: String,
    },
}

/// Generate a new unique page ID.
///
/// Replaces the former `Page::new_id()` associated function now that `Page`
/// is defined in `wenlan-types` and `impl` blocks on foreign types are
/// disallowed.
pub fn new_page_id() -> String {
    format!("page_{}", uuid::Uuid::new_v4())
}

/// Build the text embedded for page-level matching.
///
/// This is shared by page writes and PageWrite dedup so the stored page vector
/// and the candidate vector cannot drift apart.
pub(crate) fn page_embedding_text(title: &str, summary: Option<&str>, content: &str) -> String {
    const PAGE_EMBED_CONTENT_CAP: usize = 1500;
    let capped_body: String = content.chars().take(PAGE_EMBED_CONTENT_CAP).collect();
    match summary {
        Some(summary) => format!("{title} {summary} {capped_body}"),
        None => format!("{title} {capped_body}"),
    }
}

/// Decide a page's `kind` — migration 89's page-kind discriminator — from what
/// the page actually is.
///
/// One rule, shared by every insert path, so a page's stated kind cannot depend
/// on which writer happened to create it. `kind` is `NOT NULL DEFAULT 'concept'`,
/// which means an insert that stays silent does not fail; it asserts the row is
/// a concept page. Every page written between migration 89 and this rule did
/// exactly that, the reserved Overview singleton included.
///
/// The Overview is recognised by title, matching migration 89's own backfill:
/// its `creation_kind` is 'research', indistinguishable from any other research
/// page, and the title is reserved only among live pages. Everything else
/// follows `creation_kind`, with 'distilled' and 'research' both landing on
/// 'concept'.
///
/// One disagreement with the stored column survives on purpose, and it is the
/// reason no reader may route on `kind` yet. Migration 89 folded
/// `creation_kind='imported'` onto 'source' but never `'source'` itself, and it
/// wrote a fold-ledger row for every page it saw — so migration 107, which
/// skips any page the ledger already ruled on, leaves those rows where 89 put
/// them. A page imported before 89 therefore still stores 'concept' while this
/// rule says 'source'. Size the gap on a real vault with `SELECT COUNT(*) FROM
/// pages p JOIN page_kind_fold_ledger l ON l.page_id = p.id WHERE
/// p.creation_kind = 'source' AND p.kind = 'concept'`; closing it belongs with
/// M6's re-derivation of `kind` on the rename, archive, and replace paths,
/// which have the same staleness for the same reason.
///
/// Readers still resolve the Overview by title (`synthesis::overview`); this
/// makes the column honest without moving any reader onto it, and
/// `drift_guard::no_production_read_routes_on_a_non_entity_page_kind` is what
/// keeps it that way.
pub(crate) fn page_kind_for(title: &str, creation_kind: &str, status: &str) -> &'static str {
    if status == "active"
        && title.eq_ignore_ascii_case(crate::synthesis::overview::OVERVIEW_PAGE_TITLE)
    {
        return "overview";
    }
    match creation_kind {
        "authored" => "authored",
        "imported" | "source" => "source",
        "entity" => "entity",
        _ => "concept",
    }
}

/// Whether authoritative page fields describe an active Markdown-backed page.
///
/// The stored `kind` remains a trusted entity fence. Other classifications are
/// recomputed from title, creation kind, and status because historical rows can
/// retain a stale non-entity kind.
pub fn is_active_file_page(page: &Page) -> bool {
    page.status == "active"
        && page.kind != "entity"
        && matches!(
            page_kind_for(&page.title, &page.creation_kind, &page.status),
            "authored" | "concept" | "source"
        )
}

/// Maps a source memory's `memory_type` to the read-trust tier it sits behind.
pub fn trust_tier_for_memory_type(memory_type: Option<&str>) -> u8 {
    match memory_type {
        Some("identity") | Some("preference") => 1,
        Some("decision") | Some("correction") => 2,
        _ => 3,
    }
}

/// Filter pages by source overlap with search results.
///
/// A page is contextually relevant if the memories it was compiled from
/// overlap with the memories that search_memory returned for this query.
/// This is the strongest relevance signal: it answers "is this page about
/// the thing I'm searching for?" rather than relying on embedding similarity
/// (which we proved doesn't discriminate between good and garbage pages).
///
/// `min_overlap`: minimum number of search result source_ids that must appear
/// in the page's `source_memory_ids`. Recommended: 2 (filters noise while
/// keeping pages with genuine topical overlap).
pub fn filter_pages_by_source_overlap(
    pages: &[Page],
    search_result_source_ids: &std::collections::HashSet<String>,
    min_overlap: usize,
) -> Vec<Page> {
    pages
        .iter()
        .filter(|c| {
            let overlap = c
                .source_memory_ids
                .iter()
                .filter(|sid| search_result_source_ids.contains(sid.as_str()))
                .count();
            overlap >= min_overlap
        })
        .cloned()
        .collect()
}

/// How many pages a search reads before [`select_pages_for_context`] ranks
/// them down to its cap. It must be well above the cap, or the trusted-page
/// weight has nothing to choose between.
pub const PAGE_CANDIDATE_POOL: usize = 9;

/// Pages scoring below this normalized relevance are left out of search
/// results. Without a floor every search in a Space with a few pages would
/// carry full unrelated pages. A starting value, to be calibrated on data.
pub const PAGE_RELEVANCE_FLOOR: f32 = 0.3;

/// Fuse the two page search channels into one normalized 0..=1 relevance
/// per page id: distance-weighted reciprocal rank fusion, the keyword channel
/// weighted 0.2 against the vector channel. `vector` is in rank order with
/// cosine distances; `fts` is the keyword channel in rank order.
///
/// A keyword-only hit can reach at most 0.2 / 1.2, about 0.17, so on its own
/// it could never clear [`PAGE_RELEVANCE_FLOOR`]: recall dropped pages that
/// contain every word of the query whenever the vector channel missed them.
/// When `fts_matched_all_terms` says the keyword query required every word,
/// those pages are lifted to the floor, so they are kept but still rank below
/// pages both channels found.
pub fn fuse_page_channel_scores(
    vector: &[(String, f64)],
    fts: &[String],
    fts_matched_all_terms: bool,
) -> std::collections::HashMap<String, f32> {
    let rrf_k = 60.0f32;
    let fts_weight = 0.2f32;
    let mut scores: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (rank, (id, distance)) in vector.iter().enumerate() {
        let similarity = (1.0 - *distance as f32).max(0.01);
        *scores.entry(id.clone()).or_default() += similarity / (rrf_k + rank as f32);
    }
    for (rank, id) in fts.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += fts_weight / (rrf_k + rank as f32);
    }
    let theoretical_max = (1.0 + fts_weight) / rrf_k;
    for score in scores.values_mut() {
        *score = (*score / theoretical_max).min(1.0);
    }
    if fts_matched_all_terms {
        for id in fts {
            if let Some(score) = scores.get_mut(id) {
                *score = score.max(PAGE_RELEVANCE_FLOOR);
            }
        }
    }
    scores
}

/// How much a trusted page's relevance counts against an untrusted one.
/// Trust raises a page's rank; it is not a visibility gate. Pages are born
/// unconfirmed and a manual edit returns a page to unconfirmed, so a gate
/// would hide every page an AI client writes and every page the user edits
/// from the very searches that should find them.
const TRUSTED_PAGE_WEIGHT: f64 = 1.25;

/// A page is trusted when it passed the distillation faithfulness gate
/// (`review_status == "confirmed"`) or a person owns its prose. A manual edit
/// sets `review_status` back to unconfirmed, so ownership has to count too.
pub fn page_is_trusted(page: &Page) -> bool {
    page.review_status == "confirmed" || crate::post_write::page_is_human_owned(page)
}

/// Select pages for the assembled context block: drop pages under
/// [`PAGE_RELEVANCE_FLOOR`], then rank by relevance with trusted pages
/// weighted up by [`TRUSTED_PAGE_WEIGHT`].
pub fn select_pages_for_context(
    pages: &[Page],
    search_result_source_ids: &std::collections::HashSet<String>,
    cap: usize,
) -> Vec<Page> {
    // Decorate-sort-undecorate: bucket the score so ties are transitive
    // (float epsilon comparison is not a total order and can panic
    // `sort_by`), and compute overlap once per page instead of on every
    // comparison.
    let mut scored: Vec<(i64, usize, Page)> = pages
        .iter()
        .filter(|page| page.relevance_score >= PAGE_RELEVANCE_FLOOR)
        .cloned()
        .map(|page| {
            let overlap = page
                .source_memory_ids
                .iter()
                .filter(|sid| search_result_source_ids.contains(sid.as_str()))
                .count();
            let weight = if page_is_trusted(&page) {
                TRUSTED_PAGE_WEIGHT
            } else {
                1.0
            };
            let bucket = ((page.relevance_score as f64) * weight / 1e-6).round() as i64;
            (bucket, overlap, page)
        })
        .collect();

    scored.sort_by(|(lb, lo, _), (rb, ro, _)| rb.cmp(lb).then_with(|| ro.cmp(lo)));

    let mut selected: Vec<Page> = scored.into_iter().map(|(_, _, page)| page).collect();
    selected.truncate(cap);
    selected
}

/// Hard workspace scope for the additive Page path. Source-memory overlap is
/// a relevance signal only and can never override the Page's workspace.
pub fn scope_filter_pages(
    pages: Vec<Page>,
    caller_space: Option<&str>,
    _memory_source_ids: &std::collections::HashSet<String>,
) -> Vec<Page> {
    let Some(space) = caller_space else {
        return pages;
    };
    pages
        .into_iter()
        .filter(|page| page.workspace.as_deref() == Some(space))
        .collect()
}

/// The lines a person added to a page, by hand in the app or in the vault,
/// that the page still holds, in page order. `history` is the page's
/// `page_history`, newest first as `list_page_history` returns it; a line
/// counts when a human version added it to the version before and it is not
/// a touch-up of a line already there (a typo fix leaves the sentence to
/// whoever rewrites it from the sources). Lines are compared without
/// citation markers and with whitespace collapsed.
pub(crate) fn user_written_lines(
    history: &[crate::db::PageHistoryEntry],
    current: &str,
) -> Vec<String> {
    use std::collections::HashSet;
    let mut added: HashSet<String> = HashSet::new();
    let mut previous: HashSet<String> = HashSet::new();
    for entry in history.iter().rev() {
        let lines: HashSet<String> = entry.content.lines().filter_map(comparable_line).collect();
        if matches!(
            crate::post_write::Writer::classify(&entry.edited_by),
            crate::post_write::Writer::Human(_)
        ) {
            let before: Vec<HashSet<String>> = previous.iter().map(|l| line_words(l)).collect();
            for line in lines.difference(&previous) {
                let words = line_words(line);
                let touch_up = before.iter().any(|old| {
                    let shared = words.intersection(old).count();
                    let all = words.union(old).count();
                    all > 0 && shared * 10 >= all * 6
                });
                if !touch_up {
                    added.insert(line.clone());
                }
            }
        }
        previous = lines;
    }
    let mut seen = HashSet::new();
    current
        .lines()
        .filter_map(comparable_line)
        .filter(|line| added.contains(line) && seen.insert(line.clone()))
        .collect()
}

/// Put back into `rewrite` every line of `user_lines` it dropped, each as its
/// own paragraph: before the paragraph of the nearest line that followed it
/// on `current` and survives in the rewrite, else after the nearest one
/// that preceded it, else at the end. A rewrite from sources
/// cannot know what a person wrote that no source says; accepting the card
/// must never delete it. Returns the body and how many lines were restored.
pub(crate) fn keep_user_lines(
    rewrite: &str,
    current: &str,
    user_lines: &[String],
) -> (String, usize) {
    let flat = |text: &str| {
        text.lines()
            .filter_map(comparable_line)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let rewritten = flat(rewrite);
    let missing: Vec<&String> = user_lines
        .iter()
        .filter(|line| !rewritten.contains(line.as_str()))
        .collect();
    if missing.is_empty() {
        return (rewrite.to_string(), 0);
    }
    let current_lines: Vec<&str> = current.lines().collect();
    let mut out: Vec<String> = rewrite.lines().map(str::to_string).collect();
    for line in &missing {
        let Some(at) = current_lines
            .iter()
            .position(|raw| comparable_line(raw).as_ref() == Some(*line))
        else {
            continue;
        };
        let text = crate::citations::strip_markers(current_lines[at]);
        let surviving = |raw: &&str| {
            let raw = comparable_line(raw)?;
            out.iter()
                .position(|kept| comparable_line(kept).as_ref() == Some(&raw))
        };
        let after = current_lines[at + 1..].iter().find_map(surviving);
        let before = current_lines[..at].iter().rev().find_map(surviving);
        let insert = match (after, before) {
            // Before the next surviving line's paragraph, not inside it.
            (Some(index), _) => out[..index]
                .iter()
                .rposition(|kept| kept.trim().is_empty())
                .map_or(0, |blank| blank + 1),
            // After the last surviving line's paragraph.
            (None, Some(index)) => out[index..]
                .iter()
                .position(|kept| kept.trim().is_empty())
                .map_or(out.len(), |blank| index + blank),
            (None, None) => out.len(),
        };
        out.splice(insert..insert, [String::new(), text, String::new()]);
    }
    let mut body = String::new();
    let mut blank_run = 0;
    for line in out {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        body.push_str(&line);
        body.push('\n');
    }
    (body.trim().to_string(), missing.len())
}

/// Lowercased words of a line, each CJK character counting as one.
fn line_words(line: &str) -> std::collections::HashSet<String> {
    let mut words = std::collections::HashSet::new();
    let mut word = String::new();
    for c in line.chars() {
        if c.is_alphanumeric() && !is_cjk_char(c) {
            word.extend(c.to_lowercase());
            continue;
        }
        if !word.is_empty() {
            words.insert(std::mem::take(&mut word));
        }
        if is_cjk_char(c) {
            words.insert(c.to_string());
        }
    }
    if !word.is_empty() {
        words.insert(word);
    }
    words
}

fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// A line as `user_written_lines` compares it: markers out, whitespace
/// collapsed, `None` for a line with no words.
fn comparable_line(line: &str) -> Option<String> {
    let bare = crate::citations::strip_markers(line);
    let collapsed = bare.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .any(char::is_alphanumeric)
        .then_some(collapsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn history(versions: &[(&str, &str)]) -> Vec<crate::db::PageHistoryEntry> {
        versions
            .iter()
            .enumerate()
            .rev()
            .map(|(i, (edited_by, content))| crate::db::PageHistoryEntry {
                version: i as i64 + 1,
                title: None,
                content: content.to_string(),
                source_memory_ids: Vec::new(),
                edited_by: edited_by.to_string(),
                created_at: 0,
            })
            .collect()
    }

    #[test]
    fn user_lines_are_the_ones_a_human_version_added() {
        let v1 = "# Kestrel garden\n\nThe north bed gets drip irrigation [1].";
        let v2 = "# Kestrel garden\n\nThe north bed gets drip irrigation [1].\n\nMy own note: Maren wants to add a herb spiral next spring.";
        let v3 = "# Kestrel garden\n\nThe north bed gets drip irrigation and mulch [1].\n\nMy own note: Maren wants to add a herb spiral next spring.";
        let h = history(&[("create", v1), ("manual_edit", v2), ("agent_refresh", v3)]);
        assert_eq!(
            user_written_lines(&h, v3),
            vec!["My own note: Maren wants to add a herb spiral next spring.".to_string()]
        );
        // A line the agent rewrote is not the user's, even though the user's
        // version also held it.
        assert!(!user_written_lines(&h, v3)
            .iter()
            .any(|l| l.contains("drip")));
        // A typo fix of the machine's sentence leaves it to the rewrite.
        let v4 = "# Kestrel garden\n\nThe north bed gets drip irigation and mulch [1].";
        let v5 = "# Kestrel garden\n\nThe north bed gets drip irrigation and mulch [1].";
        let h = history(&[("create", v4), ("manual_edit", v5)]);
        assert!(user_written_lines(&h, v5).is_empty());
    }

    #[test]
    fn rewrite_that_drops_a_user_line_gets_it_back_in_place() {
        let current = "# Kestrel garden\n\nThe north bed gets drip irrigation [1].\n\nMy own note: Maren wants to add a herb spiral next spring.\n\nCompost goes on in March [2].";
        let user = vec!["My own note: Maren wants to add a herb spiral next spring.".to_string()];
        let rewrite = "# Kestrel garden\n\nThe north bed now gets drip irrigation and straw mulch [1].\n\nCompost goes on in March [2].";
        let (body, restored) = keep_user_lines(rewrite, current, &user);
        assert_eq!(restored, 1);
        assert_eq!(
            body,
            "# Kestrel garden\n\nThe north bed now gets drip irrigation and straw mulch [1].\n\nMy own note: Maren wants to add a herb spiral next spring.\n\nCompost goes on in March [2]."
        );
        // Kept already (even inside a paragraph, with a marker added): untouched.
        let kept = "# Kestrel garden\n\nDrip irrigation [1]. My own note: Maren wants to add a herb spiral next spring. [2]";
        assert_eq!(keep_user_lines(kept, current, &user), (kept.to_string(), 0));
    }

    #[test]
    fn user_line_with_no_surviving_anchor_goes_to_the_end() {
        let current = "Old opening line.\nShopping list: milk, eggs, bread.";
        let user = vec!["Shopping list: milk, eggs, bread.".to_string()];
        let (body, restored) = keep_user_lines("A new body [1].", current, &user);
        assert_eq!(restored, 1);
        assert_eq!(body, "A new body [1].\n\nShopping list: milk, eggs, bread.");
    }

    fn make_page(id: &str, source_ids: &[&str]) -> Page {
        make_page_with(id, source_ids, 0.5, "confirmed")
    }

    fn make_page_with(
        id: &str,
        source_ids: &[&str],
        relevance_score: f32,
        review_status: &str,
    ) -> Page {
        Page {
            id: id.to_string(),
            title: id.to_string(),
            summary: None,
            content: String::new(),
            entity_id: None,
            space: None,
            source_memory_ids: source_ids.iter().map(|s| s.to_string()).collect(),
            version: 1,
            status: "active".to_string(),
            created_at: String::new(),
            last_compiled: String::new(),
            last_modified: String::new(),
            sources_updated_count: 0,
            stale_reason: None,
            pending_rebuild: None,
            refresh_blocked_reason: None,
            review_required: false,
            user_edited: false,
            relevance_score,
            last_edited_by: None,
            last_edited_at: None,
            last_delta_summary: None,
            changelog: None,
            creation_kind: "distilled".to_string(),
            review_status: review_status.to_string(),
            workspace: None,
            citations: Vec::new(),
            kind: "concept".to_string(),
            truth: None,
        }
    }

    #[test]
    fn active_file_page_classification_matrix() {
        let cases = [
            ("ordinary", "distilled", "active", "overview", true),
            ("ordinary", "authored", "active", "authored", true),
            ("ordinary", "imported", "active", "concept", true),
            ("ordinary", "source", "active", "source", true),
            ("ordinary", "distilled", "active", "entity", false),
            ("ordinary", "entity", "active", "concept", false),
            ("ordinary", "authored", "draft", "authored", false),
            ("ordinary", "authored", "archived", "authored", false),
            ("overview", "authored", "active", "concept", false),
            ("OVERVIEW", "authored", "active", "authored", false),
        ];

        for (title, creation_kind, status, stored_kind, expected) in cases {
            let mut page = make_page("classification", &[]);
            page.title = title.to_string();
            page.creation_kind = creation_kind.to_string();
            page.status = status.to_string();
            page.kind = stored_kind.to_string();
            assert_eq!(
                is_active_file_page(&page),
                expected,
                "title={title}, creation_kind={creation_kind}, status={status}, stored_kind={stored_kind}"
            );
        }
    }

    #[test]
    fn tier_map_identity_and_preference_are_tier1() {
        assert_eq!(trust_tier_for_memory_type(Some("identity")), 1);
        assert_eq!(trust_tier_for_memory_type(Some("preference")), 1);
    }

    #[test]
    fn tier_map_decision_correction_tier2_else_tier3() {
        assert_eq!(trust_tier_for_memory_type(Some("decision")), 2);
        assert_eq!(trust_tier_for_memory_type(Some("correction")), 2);
        assert_eq!(trust_tier_for_memory_type(Some("fact")), 3);
        assert_eq!(trust_tier_for_memory_type(None), 3);
    }

    #[test]
    fn test_overlap_keeps_matching_concept() {
        let pages = vec![make_page("c1", &["m1", "m2", "m3"])];
        let search_ids: HashSet<String> = ["m1", "m2"].iter().map(|s| s.to_string()).collect();
        let kept = filter_pages_by_source_overlap(&pages, &search_ids, 2);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn test_overlap_filters_low_overlap() {
        let pages = vec![make_page("c1", &["m1", "m2", "m3"])];
        let search_ids: HashSet<String> = ["m1", "m99"].iter().map(|s| s.to_string()).collect();
        let kept = filter_pages_by_source_overlap(&pages, &search_ids, 2);
        assert_eq!(kept.len(), 0); // only 1 overlap, need 2
    }

    #[test]
    fn test_overlap_empty_concept_sources() {
        let pages = vec![make_page("c1", &[])];
        let search_ids: HashSet<String> = ["m1"].iter().map(|s| s.to_string()).collect();
        let kept = filter_pages_by_source_overlap(&pages, &search_ids, 1);
        assert_eq!(kept.len(), 0);
    }

    #[test]
    fn test_overlap_empty_search_results() {
        let pages = vec![make_page("c1", &["m1", "m2"])];
        let search_ids: HashSet<String> = HashSet::new();
        let kept = filter_pages_by_source_overlap(&pages, &search_ids, 1);
        assert_eq!(kept.len(), 0);
    }

    #[test]
    fn test_overlap_zero_threshold_keeps_all() {
        let pages = vec![make_page("c1", &["m1"]), make_page("c2", &["m99"])];
        let search_ids: HashSet<String> = ["m1"].iter().map(|s| s.to_string()).collect();
        let kept = filter_pages_by_source_overlap(&pages, &search_ids, 0);
        assert_eq!(kept.len(), 2); // min_overlap=0 keeps everything
    }

    #[test]
    fn page_embedding_text_concats_title_summary_and_capped_body() {
        // With a summary the slots join as "{title} {summary} {body}".
        assert_eq!(
            page_embedding_text("Title", Some("Summary"), "Body"),
            "Title Summary Body"
        );
        // Without a summary the summary slot is dropped entirely.
        assert_eq!(page_embedding_text("Title", None, "Body"), "Title Body");
        // The body is capped so a long page can't blow the embedder window;
        // the cap counts characters, not bytes.
        let long_body = "x".repeat(2000);
        let text = page_embedding_text("T", None, &long_body);
        let body_part = text.strip_prefix("T ").expect("title prefix present");
        assert_eq!(
            body_part.chars().count(),
            1500,
            "body must be capped at 1500 chars"
        );
    }

    #[test]
    fn page_embedding_text_is_the_single_source_of_truth() {
        // The write side (`insert_page_with_kind`) and the read/dedup side
        // (`create_page_impl`) must build the page embedding vector from the
        // SAME helper. If either re-inlines the formula or the cap, the stored
        // page vector and the dedup candidate vector silently drift and
        // near-duplicate pages stop matching — the exact bug this dedup path
        // exists to prevent. Guard it at compile-inlined source level.
        let db_source = include_str!("db.rs");
        let post_write_sources = [
            include_str!("post_write.rs"),
            include_str!("post_write/page_create.rs"),
        ]
        .join("\n");

        // Both call sites route through the shared helper.
        assert!(
            db_source.contains("page_embedding_text("),
            "insert_page_with_kind must call the shared page_embedding_text helper"
        );
        assert!(
            post_write_sources.contains("page_embedding_text("),
            "PageWrite dedup must call the shared page_embedding_text helper"
        );
        // Neither call site redefines the helper or its cap locally.
        assert!(
            !post_write_sources.contains("fn page_embedding_text"),
            "PageWrite must not carry its own page_embedding_text definition"
        );
        assert!(
            !db_source.contains("PAGE_EMBED_CONTENT_CAP"),
            "insert_page_with_kind must not carry a private page embedding cap"
        );
        assert!(
            !post_write_sources.contains("PAGE_EMBED_CONTENT_CAP"),
            "PageWrite dedup must not carry a private page embedding cap"
        );
    }

    #[test]
    fn test_overlap_mixed_keeps_and_filters() {
        let pages = vec![
            make_page("good", &["m1", "m2", "m3", "m4", "m5"]),
            make_page("noise", &["m90", "m91", "m92"]),
            make_page("edge", &["m1", "m90"]),
        ];
        let search_ids: HashSet<String> =
            ["m1", "m2", "m3"].iter().map(|s| s.to_string()).collect();
        let kept = filter_pages_by_source_overlap(&pages, &search_ids, 2);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "good"); // 3 overlap
                                        // "noise" has 0 overlap, "edge" has 1 overlap — both filtered at min_overlap=2
    }

    #[test]
    fn search_pages_scope_does_not_allow_source_overlap_to_override_workspace() {
        let mut personal = make_page("personal", &["work-memory"]);
        personal.workspace = Some("personal".to_string());
        let work_sources = HashSet::from(["work-memory".to_string()]);

        let kept = scope_filter_pages(vec![personal], Some("work"), &work_sources);

        assert!(
            kept.is_empty(),
            "a source-memory overlap must not expose a Page from another workspace"
        );
    }

    #[test]
    fn select_keeps_zero_overlap_page_when_score_high() {
        let pages = vec![
            make_page_with("low_overlap", &["m1"], 0.35, "confirmed"),
            make_page_with("high_zero_overlap", &["m90"], 0.95, "confirmed"),
        ];
        let search_ids: HashSet<String> = ["m1"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 2);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].id, "high_zero_overlap");
    }

    #[test]
    fn select_keeps_unconfirmed_pages() {
        let pages = vec![
            make_page_with("unconfirmed", &["m1"], 0.99, "unconfirmed"),
            make_page_with("confirmed", &["m2"], 0.40, "confirmed"),
        ];
        let search_ids: HashSet<String> = ["m1", "m2"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 3);

        let ids: Vec<&str> = selected.iter().map(|page| page.id.as_str()).collect();
        assert_eq!(ids, ["unconfirmed", "confirmed"]);
    }

    #[test]
    fn select_weights_human_owned_pages_like_confirmed() {
        let mut edited = make_page_with("edited", &["m1"], 0.45, "unconfirmed");
        edited.user_edited = true;
        let mut authored = make_page_with("authored", &["m2"], 0.44, "unconfirmed");
        authored.creation_kind = "authored".to_string();
        let pages = vec![
            make_page_with("machine", &["m3"], 0.50, "unconfirmed"),
            edited,
            authored,
        ];
        let search_ids: HashSet<String> = HashSet::new();

        let selected = select_pages_for_context(&pages, &search_ids, 3);

        let ids: Vec<&str> = selected.iter().map(|page| page.id.as_str()).collect();
        assert_eq!(ids, ["edited", "authored", "machine"]);
    }

    #[test]
    fn keyword_hit_on_every_word_clears_the_floor_but_ranks_below_both_channels() {
        let vector = vec![("both".to_string(), 0.4)];
        let fts = vec!["both".to_string(), "keyword_only".to_string()];

        let lifted = fuse_page_channel_scores(&vector, &fts, true);
        assert_eq!(lifted["keyword_only"], PAGE_RELEVANCE_FLOOR);
        assert!(lifted["both"] > lifted["keyword_only"]);

        let pages = vec![
            make_page_with(
                "keyword_only",
                &["m1"],
                lifted["keyword_only"],
                "unconfirmed",
            ),
            make_page_with("both", &["m2"], lifted["both"], "unconfirmed"),
        ];
        let search_ids: HashSet<String> = HashSet::new();
        let ids: Vec<String> = select_pages_for_context(&pages, &search_ids, 3)
            .into_iter()
            .map(|page| page.id)
            .collect();
        assert_eq!(ids, ["both", "keyword_only"]);
    }

    #[test]
    fn keyword_hit_on_any_word_stays_under_the_floor() {
        let fts = vec!["loose".to_string()];
        let scores = fuse_page_channel_scores(&[], &fts, false);
        assert!(scores["loose"] < PAGE_RELEVANCE_FLOOR, "{scores:?}");
        assert!((scores["loose"] - 0.2 / 1.2).abs() < 1e-6);
    }

    #[test]
    fn select_drops_pages_below_the_relevance_floor() {
        let pages = vec![
            make_page_with("related", &["m1"], 0.60, "unconfirmed"),
            make_page_with(
                "unrelated",
                &["m2"],
                PAGE_RELEVANCE_FLOOR - 0.01,
                "confirmed",
            ),
        ];
        let search_ids: HashSet<String> = ["m1", "m2"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 3);

        let ids: Vec<&str> = selected.iter().map(|page| page.id.as_str()).collect();
        assert_eq!(ids, ["related"]);
    }

    #[test]
    fn select_weights_confirmed_pages_up() {
        let pages = vec![
            make_page_with("unconfirmed", &["m1"], 0.50, "unconfirmed"),
            make_page_with("confirmed", &["m2"], 0.45, "confirmed"),
        ];
        let search_ids: HashSet<String> = ["m1", "m2"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 1);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "confirmed");
    }

    #[test]
    fn select_ranks_by_score_desc() {
        let pages = vec![
            make_page_with("low_with_overlap", &["m1"], 0.35, "confirmed"),
            make_page_with("high_without_overlap", &["m90"], 0.80, "confirmed"),
            make_page_with("mid_with_overlap", &["m2"], 0.50, "confirmed"),
        ];
        let search_ids: HashSet<String> = ["m1", "m2"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 3);

        let ids: Vec<_> = selected.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "high_without_overlap",
                "mid_with_overlap",
                "low_with_overlap"
            ]
        );
    }

    #[test]
    fn select_overlap_breaks_ties() {
        let pages = vec![
            make_page_with("zero_overlap", &["m90"], 0.75, "confirmed"),
            make_page_with("with_overlap", &["m1"], 0.75, "confirmed"),
        ];
        let search_ids: HashSet<String> = ["m1"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 2);

        assert_eq!(selected[0].id, "with_overlap");
        assert_eq!(selected[1].id, "zero_overlap");
    }

    #[test]
    fn select_respects_cap() {
        let pages = vec![
            make_page_with("one", &["m1"], 0.90, "confirmed"),
            make_page_with("two", &["m2"], 0.80, "confirmed"),
            make_page_with("three", &["m3"], 0.70, "confirmed"),
        ];
        let search_ids: HashSet<String> =
            ["m1", "m2", "m3"].iter().map(|s| s.to_string()).collect();

        let selected = select_pages_for_context(&pages, &search_ids, 2);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].id, "one");
        assert_eq!(selected[1].id, "two");
    }

    #[test]
    fn scope_requires_matching_workspace_even_with_source_overlap() {
        let mut p_other = make_page("p_other", &["x1"]); // workspace None, sources disjoint
        p_other.workspace = Some("personal".to_string());
        let mut p_match = make_page("p_match", &["x2"]);
        p_match.workspace = Some("work".to_string());
        let p_overlap = make_page("p_overlap", &["m1"]); // workspace None but source in result set
        let ids: HashSet<String> = ["m1"].iter().map(|s| s.to_string()).collect();
        let kept = scope_filter_pages(vec![p_other, p_match, p_overlap], Some("work"), &ids);
        let kept_ids: Vec<_> = kept.iter().map(|p| p.id.as_str()).collect();
        assert!(kept_ids.contains(&"p_match")); // workspace == caller space
        assert!(!kept_ids.contains(&"p_overlap")); // source overlap cannot override workspace
        assert!(!kept_ids.contains(&"p_other")); // cross-space, no overlap → dropped
    }

    #[test]
    fn scope_noop_when_no_space_filter() {
        let ids: HashSet<String> = HashSet::new();
        let pages = vec![make_page("a", &["z"])];
        assert_eq!(scope_filter_pages(pages, None, &ids).len(), 1); // unscoped recall keeps all
    }
}
