// SPDX-License-Identifier: Apache-2.0
//! Pages the search hits link to (UX-FIND-5).
//!
//! When recall surfaces a page, the pages it links to with `[[links]]` come
//! along, each cut to its first paragraph, so every AI tool gets them in the
//! same answer instead of having to think of calling `get_page_links`. Only
//! resolved links count, read in the caller's scope; a linked page passes the
//! same source-tier gate as a ranked one.
//!
//! Measured on a copy of one person's pages (2026-11-09, 157 pages,
//! keyword search, same number of results): where the hit page had links,
//! the share of related pages found rose from 0.207 to 0.267.

use super::MemoryDB;
use crate::read_scope::ReadScope;
use std::collections::{HashMap, HashSet};
use wenlan_types::responses::LinkedPage;

/// The longest point a linked page carries, in characters.
pub(crate) const LINKED_PAGE_POINT_CHARS: usize = 600;

impl MemoryDB {
    /// The pages `hits` (page id, title; best first) link to, at most `cap`,
    /// leaving out the hits themselves. A page more hits link to comes first;
    /// otherwise the order is the hits' order, then each hit's link order.
    pub async fn linked_pages_for_hits(
        &self,
        hits: &[(String, String)],
        scope: &ReadScope,
        caller_trust: &str,
        cap: usize,
    ) -> Vec<LinkedPage> {
        // Pages as a class need tier 2, the same floor as the ranked pages.
        if cap == 0 || hits.is_empty() || !crate::router::classify::tier_allowed(caller_trust, 2) {
            return Vec::new();
        }
        let hit_ids: HashSet<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        let mut order: Vec<String> = Vec::new();
        let mut linked_from: HashMap<String, Vec<String>> = HashMap::new();
        for (hit_id, hit_title) in hits {
            let Ok(links) = self.get_page_outbound_links_scoped(hit_id, scope).await else {
                continue;
            };
            for link in links {
                let Some(target) = link.target_page_id else {
                    continue;
                };
                if hit_ids.contains(target.as_str()) {
                    continue;
                }
                let from = linked_from.entry(target.clone()).or_default();
                if from.is_empty() {
                    order.push(target);
                }
                if !from.contains(hit_title) {
                    from.push(hit_title.clone());
                }
            }
        }
        // Stable: equal counts keep their first-seen order.
        order.sort_by_key(|id| std::cmp::Reverse(linked_from.get(id).map_or(0, Vec::len)));

        let mut pages = Vec::new();
        for id in order {
            if let Ok(Some(page)) = self.get_page(&id).await {
                if page.status == "active"
                    && page.kind != "entity"
                    && scope.matches(page.workspace.as_deref())
                {
                    pages.push(page);
                }
            }
        }
        self.pages_within_source_tier(pages, caller_trust)
            .await
            .into_iter()
            .take(cap)
            .map(|page| LinkedPage {
                linked_from: linked_from.remove(&page.id).unwrap_or_default(),
                point: page_point(&page.content),
                confirmed: crate::pages::page_is_trusted(&page),
                id: page.id,
                title: page.title,
                space: page.space,
            })
            .collect()
    }
}

/// A page's first paragraph, its point (UX-VIEW-5): the first run of text
/// lines after any headings, cut to [`LINKED_PAGE_POINT_CHARS`].
pub(crate) fn page_point(content: &str) -> String {
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        if lines.is_empty()
            && (trimmed.starts_with('#') || trimmed == "---" || trimmed.starts_with("<!--"))
        {
            continue;
        }
        lines.push(trimmed);
    }
    let point = lines.join(" ");
    if point.chars().count() <= LINKED_PAGE_POINT_CHARS {
        return point;
    }
    let cut: String = point.chars().take(LINKED_PAGE_POINT_CHARS).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::{page_point, LINKED_PAGE_POINT_CHARS};
    use crate::db::tests::test_db;
    use crate::read_scope::ReadScope;
    use crate::synthesis::wikilinks::Wikilink;

    async fn add_page(db: &crate::db::MemoryDB, id: &str, title: &str, body: &str, space: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        db.insert_page_with_kind(
            id,
            title,
            None,
            body,
            None,
            Some("decision"),
            &[],
            &now,
            "authored",
            "confirmed",
            Some(space),
            None,
        )
        .await
        .unwrap();
    }

    fn link(target: Option<&str>, label: &str) -> Wikilink {
        Wikilink {
            target_page_id: target.map(str::to_string),
            label: label.to_string(),
        }
    }

    #[tokio::test]
    async fn ux_find_5_recall_brings_the_pages_hits_link_to() {
        let (db, _tmp) = test_db().await;
        add_page(&db, "hit-a", "Trip plan", "We go in May.", "work").await;
        add_page(&db, "hit-d", "Budget", "Keep it under 2000.", "work").await;
        add_page(
            &db,
            "tickets",
            "Tickets",
            "# Tickets\n\nBuy them in March.\n\nMore.",
            "work",
        )
        .await;
        add_page(&db, "hotel", "Hotel", "Near the station.", "work").await;
        add_page(&db, "elsewhere", "Elsewhere", "Another Space.", "personal").await;
        db.replace_page_links(
            "hit-a",
            &[
                link(Some("hotel"), "Hotel"),
                link(Some("tickets"), "Tickets"),
                link(Some("hit-d"), "Budget"),
                link(None, "Not written yet"),
            ],
        )
        .await
        .unwrap();
        db.replace_page_links("hit-d", &[link(Some("tickets"), "Tickets")])
            .await
            .unwrap();
        let hits = vec![
            ("hit-a".to_string(), "Trip plan".to_string()),
            ("hit-d".to_string(), "Budget".to_string()),
        ];

        let linked = db
            .linked_pages_for_hits(&hits, &ReadScope::Global, "full", 5)
            .await;
        let ids: Vec<&str> = linked.iter().map(|page| page.id.as_str()).collect();
        // Two hits link to Tickets, so it comes first; hits never repeat and
        // an unresolved link brings nothing.
        assert_eq!(ids, vec!["tickets", "hotel"]);
        assert_eq!(linked[0].point, "Buy them in March.");
        assert_eq!(linked[0].linked_from, vec!["Trip plan", "Budget"]);

        let one = db
            .linked_pages_for_hits(&hits, &ReadScope::Global, "full", 1)
            .await;
        assert_eq!(one.len(), 1);
    }

    #[tokio::test]
    async fn ux_find_5_linked_pages_keep_scope_and_trust() {
        let (db, _tmp) = test_db().await;
        add_page(&db, "hit-a", "Trip plan", "We go in May.", "work").await;
        add_page(&db, "elsewhere", "Elsewhere", "Another Space.", "personal").await;
        db.replace_page_links("hit-a", &[link(Some("elsewhere"), "Elsewhere")])
            .await
            .unwrap();
        let hits = vec![("hit-a".to_string(), "Trip plan".to_string())];

        let scoped = db
            .linked_pages_for_hits(&hits, &ReadScope::Space("work".to_string()), "full", 5)
            .await;
        assert!(
            scoped.is_empty(),
            "a page in another Space must not come along"
        );

        let unknown = db
            .linked_pages_for_hits(&hits, &ReadScope::Global, "unknown", 5)
            .await;
        assert!(
            unknown.is_empty(),
            "a caller who may not read pages gets none by link"
        );
    }

    #[test]
    fn ux_find_5_point_is_the_first_paragraph_after_headings() {
        let content =
            "# Trip plan\n\n## Why\n\nWe go in May.\nTickets are cheap then.\n\nDetails follow.";
        assert_eq!(page_point(content), "We go in May. Tickets are cheap then.");
    }

    #[test]
    fn ux_find_5_point_is_cut_on_a_character_boundary() {
        let content = "文".repeat(LINKED_PAGE_POINT_CHARS + 10);
        let point = page_point(&content);
        assert_eq!(point.chars().count(), LINKED_PAGE_POINT_CHARS + 1);
        assert!(point.ends_with('…'));
    }

    #[test]
    fn ux_find_5_point_of_an_empty_page_is_empty() {
        assert_eq!(page_point("# Only a heading\n\n"), "");
    }
}
