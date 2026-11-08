// SPDX-License-Identifier: Apache-2.0
//! The nightly trust scan: what is already wrong in the pages and their
//! sources, found by reading only. It changes no page, memory, link or file;
//! its one write is the report itself ([`run_and_store`]), kept so the person
//! can read it and approve any repair.
//!
//! What it looks for:
//! - pages that are cut off, hold half a `[[` link, or hold U+FFFD;
//! - a latest version that lost most of the page's CJK text, or gained
//!   U+FFFD, against the version before it;
//! - a page's own source list disagreeing with its live source links, pages
//!   that should rest on sources and have none, and what the daily
//!   orphaned-source cleanup is about to remove;
//! - page files Wenlan would wrongly read as edited outside it;
//! - pages whose citation backfill gave up.
//!
//! Duplicate open revision cards are not scanned yet; see the hook comment
//! at the end of [`run_trust_scan`].

use crate::authorship::{is_protected, sentence_units};
use crate::db::trust_scan::ScanPage;
use crate::db::MemoryDB;
use crate::retrieval::integrity::{
    broken_char_count, cjk_count, drops_most_cjk, half_wiki_link, looks_cut_off,
};
use crate::WenlanError;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use wenlan_types::trust_scan::{
    BackfillGiveUp, CjkLoss, DamagedPage, DanglingSourcePage, DanglingSources, PageDamage, PageRef,
    SourceMismatch, TrustScanReport,
};

/// Kinds that should rest on sources. `authored` pages are the person's own
/// notes and `entity` pages mirror the entity records; neither cites.
const SOURCED_KINDS: &[&str] = &["concept", "source", "overview"];
const EXCERPT_CHARS: usize = 80;
const MISMATCH_IDS: usize = 5;
const DANGLING_PAGES: usize = 50;

/// Where in a body the damage sits.
enum Spot {
    /// These byte offsets.
    At(Vec<usize>),
    /// Anywhere: the damage is text that is gone, so a repair touches the
    /// page as a whole.
    Anywhere,
}

fn page_ref(page: &ScanPage) -> PageRef {
    PageRef {
        page_id: page.id.clone(),
        title: page.title.clone(),
        kind: page.kind.clone(),
    }
}

fn tail(body: &str, chars: usize) -> String {
    let body = body.trim_end();
    let count = body.chars().count();
    body.chars().skip(count.saturating_sub(chars)).collect()
}

fn around(body: &str, at: usize, chars: usize) -> String {
    let before: Vec<char> = body[..at].chars().rev().take(chars / 2).collect();
    let after = body[at..].chars().take(chars / 2);
    before.into_iter().rev().chain(after).collect()
}

fn broken_offsets(body: &str) -> Vec<usize> {
    body.match_indices('\u{FFFD}').map(|(at, _)| at).collect()
}

/// Whether any damaged spot sits in a sentence the AI may not change: the
/// person's, imported, or of unknown origin on a guarded page. When that
/// cannot be worked out, it says yes, so the repair stays the person's call.
async fn in_person_sentences(db: &MemoryDB, page_id: &str, spot: &Spot) -> bool {
    let Ok(Some(page)) = db.get_page_browse(page_id).await else {
        return true;
    };
    let Ok(authors) = crate::sentences::page_authorship(db, &page).await else {
        return true;
    };
    let guarded = crate::sentences::guarded(&page);
    let protected = |idx: usize| {
        authors
            .iter()
            .find(|sentence| sentence.idx as usize == idx)
            .is_none_or(|sentence| is_protected(sentence.author, guarded))
    };
    let units = sentence_units(&page.content);
    match spot {
        Spot::Anywhere => (0..units.len()).any(protected),
        Spot::At(offsets) => offsets.iter().any(|&at| {
            // The unit holding the spot, or the last one before it (the spot
            // can sit in the blank between units).
            units
                .iter()
                .rposition(|unit| unit.start <= at)
                .is_none_or(protected)
        }),
    }
}

/// Run the scan. Read only. A step that fails is named in `errors` and the
/// rest of the report still stands. `knowledge_path` is the pages folder;
/// without it the outside-edit check is skipped (`None` in the report).
pub async fn run_trust_scan(
    db: &MemoryDB,
    knowledge_path: Option<&Path>,
    now: i64,
) -> TrustScanReport {
    let mut report = TrustScanReport {
        ran_at: now,
        ..Default::default()
    };
    let pages = match db.trust_scan_pages().await {
        Ok(pages) => pages,
        Err(error) => {
            report.errors.push(format!("pages: {error}"));
            Vec::new()
        }
    };
    let by_id: HashMap<&str, &ScanPage> = pages.iter().map(|p| (p.id.as_str(), p)).collect();
    report.counts.pages_scanned = pages.len();

    // Item 4: damage already in the pages.
    for page in &pages {
        let body = page.content.as_str();
        let mut damage = Vec::new();
        let mut spots = Vec::new();
        let mut excerpt = None;
        if let Some(fragment) = half_wiki_link(body) {
            damage.push(PageDamage::HalfLink);
            spots.push(fragment.as_ptr() as usize - body.as_ptr() as usize);
            excerpt = Some(fragment.to_string());
        }
        if looks_cut_off(body) {
            damage.push(PageDamage::CutOff);
            spots.push(body.trim_end().len().saturating_sub(1));
        }
        let broken = broken_offsets(body);
        if !broken.is_empty() {
            damage.push(PageDamage::BrokenCharacters);
            if excerpt.is_none() && !damage.contains(&PageDamage::CutOff) {
                excerpt = Some(around(body, broken[0], EXCERPT_CHARS));
            }
            spots.extend(broken);
        }
        if damage.is_empty() {
            continue;
        }
        report.counts.cut_off += usize::from(damage.contains(&PageDamage::CutOff));
        report.counts.half_links += usize::from(damage.contains(&PageDamage::HalfLink));
        report.counts.broken_characters +=
            usize::from(damage.contains(&PageDamage::BrokenCharacters));
        report.damaged_pages.push(DamagedPage {
            page: page_ref(page),
            version: page.version,
            damage,
            excerpt: excerpt.unwrap_or_else(|| tail(body, EXCERPT_CHARS)),
            in_person_sentences: in_person_sentences(db, &page.id, &Spot::At(spots)).await,
        });
    }

    // Item 5: CJK text lost, or broken characters gained, by the latest write.
    match db.trust_scan_previous_versions().await {
        Ok(previous) => {
            for page in &pages {
                let Some(before) = previous.get(&page.id) else {
                    continue;
                };
                let (old, new) = (before.content.as_str(), page.content.as_str());
                let broken_before = broken_char_count(old);
                let broken_after = broken_char_count(new);
                let dropped = drops_most_cjk(old, new);
                if !dropped && broken_after <= broken_before {
                    continue;
                }
                let spot = if dropped {
                    Spot::Anywhere
                } else {
                    Spot::At(broken_offsets(new))
                };
                report.cjk_losses.push(CjkLoss {
                    page: page_ref(page),
                    previous_version: before.version,
                    version: page.version,
                    edited_by: before.current_edited_by.clone(),
                    cjk_before: cjk_count(old),
                    cjk_after: cjk_count(new),
                    chars_before: old.chars().count(),
                    chars_after: new.chars().count(),
                    broken_before,
                    broken_after,
                    in_person_sentences: in_person_sentences(db, &page.id, &spot).await,
                });
            }
        }
        Err(error) => report.errors.push(format!("previous versions: {error}")),
    }
    report.counts.cjk_losses = report.cjk_losses.len();

    // Item 17: sources. Live links are what the app shows as a page's sources.
    match db.trust_scan_live_cites().await {
        Ok(linked) => {
            let none = BTreeSet::new();
            for page in &pages {
                let listed: BTreeSet<String> = page.listed.iter().cloned().collect();
                let links = linked.get(&page.id).unwrap_or(&none);
                if listed.is_empty() && links.is_empty() {
                    if SOURCED_KINDS.contains(&page.kind.as_str()) {
                        report.sourceless_pages.push(page_ref(page));
                    }
                    continue;
                }
                if &listed == links {
                    continue;
                }
                report.source_mismatches.push(SourceMismatch {
                    page: page_ref(page),
                    listed: listed.len(),
                    linked: links.len(),
                    listed_only: listed
                        .difference(links)
                        .take(MISMATCH_IDS)
                        .cloned()
                        .collect(),
                    linked_only: links
                        .difference(&listed)
                        .take(MISMATCH_IDS)
                        .cloned()
                        .collect(),
                });
            }
        }
        Err(error) => report.errors.push(format!("source links: {error}")),
    }
    report.counts.source_mismatches = report.source_mismatches.len();
    report.counts.sourceless_pages = report.sourceless_pages.len();

    // What the daily cleanup will remove: the same inventory it runs on.
    match db.orphaned_page_sources_preview().await {
        Ok((links, listed, per_page)) => {
            report.dangling_sources = DanglingSources {
                links,
                listed,
                pages: per_page
                    .into_iter()
                    .take(DANGLING_PAGES)
                    .map(|(page_id, missing)| DanglingSourcePage {
                        page_id,
                        missing: missing.into_iter().collect(),
                    })
                    .collect(),
            };
            report.counts.dangling_links = links;
            report.counts.dangling_listed = listed;
        }
        Err(error) => report.errors.push(format!("dangling sources: {error}")),
    }
    match db.trust_scan_legacy_page_sources().await {
        Ok(legacy) => report.legacy_page_sources = legacy,
        Err(error) => report.errors.push(format!("legacy page_sources: {error}")),
    }

    // Item 20: files that would read as edited outside Wenlan.
    if let Some(path) = knowledge_path.filter(|path| path.exists()) {
        let found = match db.list_pages("active", 10_000, 0).await {
            Ok(live) => crate::export::knowledge::outside_edit_suspects(path, db, &live).await,
            Err(error) => Err(error),
        };
        match found {
            Ok(flags) => {
                report.counts.outside_edit_false_positives = Some(flags.len());
                report.outside_edit_false_positives = Some(flags);
            }
            Err(error) => report.errors.push(format!("outside edits: {error}")),
        }
    }

    // Item 20: citation backfill give-ups.
    match db.trust_scan_backfill_give_ups().await {
        Ok(give_ups) => {
            report.backfill_give_ups = give_ups
                .into_iter()
                .map(|(page_id, reason, gave_up_at)| BackfillGiveUp {
                    page: by_id.get(page_id.as_str()).map_or_else(
                        || PageRef {
                            page_id: page_id.clone(),
                            ..Default::default()
                        },
                        |page| page_ref(page),
                    ),
                    reason,
                    gave_up_at,
                })
                .collect();
        }
        Err(error) => report.errors.push(format!("backfill give-ups: {error}")),
    }
    report.counts.backfill_give_ups = report.backfill_give_ups.len();
    // Hook: duplicate open revision cards
    // (`MemoryDB::list_duplicate_open_revision_cards`, person-edits PR) go
    // here, as their own finding, once that reader is on main.

    report
}

/// Run the scan and keep its report as the latest. The report is the only
/// thing written.
pub async fn run_and_store(
    db: &MemoryDB,
    knowledge_path: Option<&Path>,
    now: i64,
) -> Result<TrustScanReport, WenlanError> {
    let report = run_trust_scan(db, knowledge_path, now).await;
    db.save_trust_scan_report(now, &serde_json::to_string(&report)?)
        .await?;
    Ok(report)
}

/// The latest stored report. A report stored by a build whose report shape
/// no longer parses reads as none; the next nightly run replaces it.
pub async fn latest(db: &MemoryDB) -> Result<Option<TrustScanReport>, WenlanError> {
    Ok(db
        .latest_trust_scan_report()
        .await?
        .and_then(|json| match serde_json::from_str(&json) {
            Ok(report) => Some(report),
            Err(error) => {
                log::warn!("[trust_scan] stored report no longer parses: {error}");
                None
            }
        }))
}
