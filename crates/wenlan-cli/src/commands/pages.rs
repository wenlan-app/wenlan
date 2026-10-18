// SPDX-License-Identifier: Apache-2.0
//! `wenlan pages [query]` — browse distilled pages, or open one in your editor.
//!
//! Local by default: reads the page markdown files the daemon writes under
//! `knowledge_path_or_default()` (default `~/.wenlan/pages/`) and hands the
//! match to the OS default `.md` app via `open`/`xdg-open`/`start`. No daemon
//! round-trip and no agent turn, so it stays instant and works offline. This
//! reads the user's own local page files, not daemon DB state, so it does not
//! cross the "CLI talks HTTP only" boundary. Only `--print` talks to the
//! daemon: it finds the page file the same way, then fetches the page body and
//! its sources over HTTP.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

use crate::client::WenlanClient;
use crate::output::{print_json, ResolvedFormat};

/// `wenlan pages lock|unlock <page-id>`: the page review lock.
#[derive(Debug, clap::Subcommand)]
pub enum PagesAction {
    /// Require review for every machine write to a page: agent and pipeline
    /// updates are staged as revision cards. Your own edits still apply.
    Lock {
        /// The page's id (see `wenlan pages <query> --resolve-id`).
        page_id: String,
    },
    /// Clear a page's review lock.
    Unlock {
        /// The page's id (see `wenlan pages <query> --resolve-id`).
        page_id: String,
    },
}

/// Page ids are opaque ASCII tokens; refuse anything that could change the
/// request path before it reaches the daemon.
fn validate_page_id(page_id: &str) -> Result<()> {
    let ok = !page_id.is_empty()
        && page_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !ok {
        anyhow::bail!(
            "invalid page id {page_id:?}: use the id printed by `wenlan pages <query> --resolve-id`"
        );
    }
    Ok(())
}

pub async fn run_action(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    action: PagesAction,
) -> Result<()> {
    let (page_id, required) = match action {
        PagesAction::Lock { page_id } => (page_id, true),
        PagesAction::Unlock { page_id } => (page_id, false),
    };
    validate_page_id(&page_id)?;
    let receipt = client.set_page_review_required(&page_id, required).await?;
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => print_json(&receipt)?,
        ResolvedFormat::Table => {
            if receipt.review_required {
                println!(
                    "Locked page {}: agent and pipeline updates now wait for your review.",
                    receipt.page_id
                );
            } else {
                println!(
                    "Unlocked page {}: updates apply under the usual rules again.",
                    receipt.page_id
                );
            }
        }
    }
    Ok(())
}

/// One distilled page on disk.
#[derive(Debug, Clone)]
pub struct PageEntry {
    pub path: PathBuf,
    pub title: String,
    pub page_id: Option<String>,
    pub modified: SystemTime,
}

/// Outcome of matching a query against the page list.
pub enum QueryMatch<'a> {
    None,
    One(&'a PageEntry),
    Many(Vec<&'a PageEntry>),
}

/// Pull the frontmatter `title:` (quotes stripped); fall back to the filename
/// stem. Only the leading `--- ... ---` block is scanned, so a `title:` line in
/// the body can't be mistaken for the page title.
fn extract_frontmatter_value(content: &str, key: &str) -> Option<String> {
    let mut in_fm = false;
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if i == 0 {
            if trimmed == "---" {
                in_fm = true;
                continue;
            }
            break; // no frontmatter -> use stem
        }
        if in_fm && trimmed == "---" {
            break; // end of frontmatter
        }
        if let Some(rest) = trimmed.strip_prefix(key) {
            let value = rest.trim().trim_matches('"').trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn extract_title(content: &str, fallback_stem: &str) -> String {
    extract_frontmatter_value(content, "title:").unwrap_or_else(|| fallback_stem.to_string())
}

/// Newest-first by mtime.
fn sort_newest_first(pages: &mut [PageEntry]) {
    pages.sort_by_key(|p| std::cmp::Reverse(p.modified));
}

/// Case-insensitive substring match against title OR filename stem.
fn match_query<'a>(pages: &'a [PageEntry], query: &str) -> QueryMatch<'a> {
    let q = query.to_lowercase();
    let exact_filename_hits: Vec<&PageEntry> = pages
        .iter()
        .filter(|p| {
            p.path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.to_lowercase() == q)
        })
        .collect();
    match exact_filename_hits.len() {
        1 => return QueryMatch::One(exact_filename_hits[0]),
        2.. => return QueryMatch::Many(exact_filename_hits),
        _ => {}
    }

    let hits: Vec<&PageEntry> = pages
        .iter()
        .filter(|p| {
            p.title.to_lowercase().contains(&q)
                || p.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_lowercase().contains(&q))
                    .unwrap_or(false)
        })
        .collect();
    match hits.len() {
        0 => QueryMatch::None,
        1 => QueryMatch::One(hits[0]),
        _ => QueryMatch::Many(hits),
    }
}

/// Read every visible projected `*.md` under `dir` into a newest-first list. A missing
/// dir yields an empty list (no pages distilled yet). The `.wenlan/` state dir
/// holds no `*.md`, so it's skipped by the extension filter.
fn read_pages(dir: &Path) -> Vec<PageEntry> {
    let mut entries = Vec::new();
    let Ok(paths) = wenlan_core::export::knowledge::knowledge_markdown_paths(dir) else {
        return entries;
    };
    for relative in paths {
        let path = dir.join(&relative);
        // OKF reserves `index.md` (the bundle index) and `log.md` (a
        // directory's update history) at every level of the hierarchy, spec
        // 3.1. Neither is a page, so neither belongs in this listing.
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("index.md") || n.eq_ignore_ascii_case("log.md"))
        {
            continue;
        }
        let Ok((content, modified)) =
            wenlan_core::export::knowledge::read_knowledge_markdown(dir, &relative)
        else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let title = extract_title(&content, &stem);
        let page_id = extract_frontmatter_value(&content, "origin_id:");
        entries.push(PageEntry {
            path,
            title,
            page_id,
            modified,
        });
    }
    sort_newest_first(&mut entries);
    entries
}

/// Open a path in the OS default app for its type. Returns whether a launcher
/// actually ran (false -> caller prints the path so the user opens it).
fn open_in_editor(path: &Path) -> bool {
    use std::process::Command;
    let status = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .status()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(path).status()
    } else {
        Command::new("xdg-open").arg(path).status()
    };
    status.map(|s| s.success()).unwrap_or(false)
}

/// Collapse same-title pages into `(title, count)`, newest-first by first
/// occurrence — so a topic distilled into many revisions shows as one row.
fn collapse_by_title(pages: &[PageEntry]) -> Vec<(String, usize)> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for p in pages {
        if !counts.contains_key(p.title.as_str()) {
            order.push(p.title.clone());
        }
        *counts.entry(p.title.as_str()).or_insert(0) += 1;
    }
    order
        .iter()
        .map(|t| (t.clone(), counts[t.as_str()]))
        .collect()
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    query: Option<String>,
    limit: usize,
    resolve_id: bool,
    print: bool,
) -> Result<()> {
    // An empty/whitespace query (e.g. a skill passing "" for no-arg) means "list".
    let query = query.filter(|q| !q.trim().is_empty());
    let dir = wenlan_core::config::load_config().knowledge_path_or_default();
    let pages = read_pages(&dir);

    match query {
        // No arg -> list distinct topics (same-title revisions collapsed),
        // newest-first, capped at `limit` topics (0 = all).
        None => {
            if quiet {
                return Ok(());
            }
            let total_pages = pages.len();
            let groups = collapse_by_title(&pages);
            let total_titles = groups.len();
            let shown: &[(String, usize)] = if limit == 0 {
                &groups
            } else {
                &groups[..total_titles.min(limit)]
            };
            match format {
                ResolvedFormat::Json => {
                    let items: Vec<_> = shown
                        .iter()
                        .map(|(title, count)| serde_json::json!({ "title": title, "count": count }))
                        .collect();
                    print_json(&serde_json::json!({
                        "pages": items,
                        "total_pages": total_pages,
                        "total_titles": total_titles,
                    }))?;
                }
                ResolvedFormat::Table => print_list(shown, total_titles, total_pages, &dir),
            }
        }
        // Query -> open the match.
        Some(q) => match match_query(&pages, &q) {
            QueryMatch::None if print => {
                anyhow::bail!("no page matches: {q}. Run `wenlan pages` to list them.")
            }
            QueryMatch::None => {
                eprintln!("no page matches: {q}");
                eprintln!("run `wenlan pages` to list, or `/wenlan distill {q}` to synthesize one");
            }
            QueryMatch::One(p) => {
                if resolve_id || print {
                    let page_id = p.page_id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "page projection {} has no origin_id frontmatter",
                            p.path.display()
                        )
                    })?;
                    if print {
                        return print_page(client, format, quiet, page_id).await;
                    }
                    if !quiet {
                        println!("{page_id}");
                    }
                    return Ok(());
                }
                let launched = open_in_editor(&p.path);
                if !quiet {
                    if launched {
                        println!("Opened {}", p.path.display());
                    } else {
                        // Headless / no handler: print the path to open by hand.
                        println!("{}", p.path.display());
                    }
                }
            }
            QueryMatch::Many(hits) if print => anyhow::bail!(many_matches_message(&q, &hits)),
            QueryMatch::Many(hits) => {
                if !quiet {
                    // Titles can collide (many distilled revisions share a name),
                    // so show the filename too — that's the unique handle to refine on.
                    println!(
                        "{} matches for \"{q}\" — refine with a filename:",
                        hits.len()
                    );
                    for p in &hits {
                        let stem = p.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        println!("  {}  ·  {}", p.title, stem);
                    }
                }
            }
        },
    }
    Ok(())
}

/// Render a page's per-claim citations for terminal display: verified
/// citations show plain, unverified ones carry the exact spec-mandated badge
/// text (never "false" — see spec §5). Empty input renders empty (no
/// dangling "Citations:" header for uncited pages).
fn render_citations(citations: &[wenlan_types::pages::PageCitation]) -> String {
    if citations.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nCitations:\n");
    for c in citations {
        if c.status == "verified" {
            out.push_str(&format!("  [{}] {}\n", c.marker, c.locator));
        } else {
            out.push_str(&format!(
                "  [{}] {} — ⚠ not directly traceable to a source\n",
                c.marker, c.locator
            ));
        }
    }
    out
}

/// Error text for `--print` when the query names several pages: every
/// candidate's title and path, so the user can pick one and narrow the query.
fn many_matches_message(query: &str, hits: &[&PageEntry]) -> String {
    let mut out = format!(
        "{} pages match \"{query}\". Narrow the query to one of:",
        hits.len()
    );
    for p in hits {
        out.push_str(&format!("\n  {}  ({})", p.title, p.path.display()));
    }
    out
}

/// One "Sources" line: the short memory id, then the first line of the
/// memory's content (title when the content is blank), cut to 100 characters.
fn source_line(source: &wenlan_types::PageSourceWithMemory) -> String {
    let id: String = source.source.memory_source_id.chars().take(8).collect();
    let text = source
        .memory
        .as_ref()
        .map(|m| {
            let first = m.content.lines().find(|l| !l.trim().is_empty());
            first.unwrap_or(m.title.as_str()).trim().to_string()
        })
        .unwrap_or_else(|| "(memory no longer available)".to_string());
    let text = if text.chars().count() > 100 {
        let cut: String = text.chars().take(99).collect();
        format!("{cut}…")
    } else {
        text
    };
    format!("  {id}  {text}")
}

/// `--print`: fetch the page and its sources and write them to stdout.
async fn print_page(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    page_id: &str,
) -> Result<()> {
    let read = client.read_page(page_id).await?;
    let sources = client.get_page_sources(page_id).await?;
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => {
            print_json(&serde_json::json!({ "page": read.page, "sources": sources }))?
        }
        ResolvedFormat::Table => {
            let page = &read.page.page;
            println!("{}\n", page.title);
            println!("{}", page.content.trim_end());
            print!("{}", render_citations(&page.citations));
            if !sources.is_empty() {
                println!("\nSources:");
                for source in &sources {
                    println!("{}", source_line(source));
                }
            }
        }
    }
    Ok(())
}

fn print_list(shown: &[(String, usize)], total_titles: usize, total_pages: usize, dir: &Path) {
    if total_pages == 0 {
        println!("(no pages in {})", dir.display());
        println!(
            "  Pages come from the distill tool (/distill in the Claude Code or Codex plugin, or any MCP client) once a few related memories exist, or from the page editor in the Wenlan app."
        );
        return;
    }
    if shown.len() < total_titles {
        println!(
            "{total_pages} pages, {total_titles} topics (newest {} shown — `--limit 0` for all):",
            shown.len()
        );
    } else {
        println!(
            "{total_pages} pages, {total_titles} topic{}:",
            if total_titles == 1 { "" } else { "s" }
        );
    }
    for (title, count) in shown {
        if *count > 1 {
            println!("  {title}  (×{count})");
        } else {
            println!("  {title}");
        }
    }
    println!("open one: wenlan pages <title-or-filename>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn mk(title: &str, file: &str, secs: u64) -> PageEntry {
        PageEntry {
            path: PathBuf::from(file),
            title: title.into(),
            page_id: None,
            modified: UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn title_from_quoted_frontmatter() {
        let md = "---\ntitle: \"Rust Systems Language\"\norigin_id: x\n---\nbody\n";
        assert_eq!(extract_title(md, "stem"), "Rust Systems Language");
    }

    #[test]
    fn title_unquoted() {
        let md = "---\ntitle: Plain Title\n---\n";
        assert_eq!(extract_title(md, "stem"), "Plain Title");
    }

    #[test]
    fn title_falls_back_to_stem_when_no_frontmatter() {
        // a `title:` in the body must NOT be picked up
        let md = "no frontmatter\ntitle: body-title-should-not-match\n";
        assert_eq!(extract_title(md, "my-file"), "my-file");
    }

    #[test]
    fn title_falls_back_when_empty() {
        let md = "---\ntitle:\n---\n";
        assert_eq!(extract_title(md, "stem"), "stem");
    }

    #[test]
    fn query_unique_match_by_title() {
        let pages = vec![
            mk("Rust Systems", "a.md", 1),
            mk("Cooking Pasta", "b.md", 2),
        ];
        match match_query(&pages, "RUST") {
            QueryMatch::One(p) => assert_eq!(p.title, "Rust Systems"),
            _ => panic!("expected one"),
        }
    }

    #[test]
    fn query_multiple_matches() {
        let pages = vec![mk("Rust Systems", "a.md", 1), mk("Rust Async", "b.md", 2)];
        match match_query(&pages, "rust") {
            QueryMatch::Many(v) => assert_eq!(v.len(), 2),
            _ => panic!("expected many"),
        }
    }

    #[test]
    fn query_no_match() {
        let pages = vec![mk("Rust", "a.md", 1)];
        assert!(matches!(match_query(&pages, "zzz"), QueryMatch::None));
    }

    #[test]
    fn query_matches_filename_stem() {
        let pages = vec![mk("Some Title", "2026-03-18-notes.md", 1)];
        match match_query(&pages, "2026-03") {
            QueryMatch::One(p) => assert_eq!(p.title, "Some Title"),
            _ => panic!("expected one (stem match)"),
        }
    }

    #[test]
    fn exact_filename_stem_wins_over_broader_substring_matches() {
        let pages = vec![
            mk("Atlas", "atlas.md", 1),
            mk("Atlas Notes", "atlas-notes.md", 2),
        ];
        match match_query(&pages, "atlas") {
            QueryMatch::One(p) => assert_eq!(p.path, PathBuf::from("atlas.md")),
            _ => panic!("expected the exact filename stem"),
        }
    }

    #[test]
    fn collapse_groups_same_title_newest_first() {
        // newest-first input; "Rust" appears non-contiguously -> still one row, count 3
        let pages = vec![
            mk("Rust", "r3.md", 9),
            mk("Rust", "r2.md", 8),
            mk("Cooking", "c.md", 7),
            mk("Rust", "r1.md", 6),
        ];
        let g = collapse_by_title(&pages);
        assert_eq!(g, vec![("Rust".to_string(), 3), ("Cooking".to_string(), 1)]);
    }

    #[test]
    fn sorts_newest_first() {
        let mut v = vec![
            mk("Old", "a.md", 1),
            mk("New", "b.md", 9),
            mk("Mid", "c.md", 5),
        ];
        sort_newest_first(&mut v);
        let order: Vec<&str> = v.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(order, ["New", "Mid", "Old"]);
    }

    #[test]
    fn many_matches_message_lists_every_candidate_title_and_path() {
        let a = mk("Rust Systems", "/pages/rust-systems.md", 1);
        let b = mk("Rust Async", "/pages/rust-async.md", 2);
        let msg = many_matches_message("rust", &[&a, &b]);
        assert!(msg.starts_with("2 pages match \"rust\". Narrow the query"));
        assert!(msg.contains("Rust Systems  (/pages/rust-systems.md)"));
        assert!(msg.contains("Rust Async  (/pages/rust-async.md)"));
    }

    fn source_with(
        memory_source_id: &str,
        content: &str,
        title: &str,
    ) -> wenlan_types::PageSourceWithMemory {
        serde_json::from_value(serde_json::json!({
            "source": {
                "page_id": "page_1",
                "memory_source_id": memory_source_id,
                "linked_at": 1
            },
            "memory": {
                "source_id": memory_source_id,
                "title": title,
                "content": content,
                "summary": null,
                "memory_type": null,
                "source_agent": null,
                "confidence": null,
                "confirmed": true,
                "pinned": false,
                "supersedes": null,
                "last_modified": 1,
                "chunk_count": 1,
                "enrichment_status": "enriched",
        "supersede_mode": "hide",
        "access_count": 0
            }
        }))
        .expect("source fixture")
    }

    #[test]
    fn source_line_uses_short_id_and_first_content_line() {
        let s = source_with("mem_0123456789", "\nFirst line\nSecond line", "A title");
        assert_eq!(source_line(&s), "  mem_0123  First line");
        let blank = source_with("mem_abc", "  \n ", "Only a title");
        assert_eq!(source_line(&blank), "  mem_abc  Only a title");
        let long = source_with("mem_abc", &"x".repeat(150), "t");
        assert_eq!(source_line(&long).chars().count(), 2 + 7 + 2 + 100);
    }

    #[test]
    fn render_citations_verified_and_badged() {
        use wenlan_types::pages::PageCitation;
        let cites = vec![
            PageCitation {
                occurrence: 1,
                marker: 1,
                source_kind: "memory".into(),
                locator: "mem_a".into(),
                score: 0.9,
                status: "verified".into(),
                scope: "sentence".into(),
            },
            PageCitation {
                occurrence: 2,
                marker: 2,
                source_kind: "memory".into(),
                locator: "mem_b".into(),
                score: 0.2,
                status: "unverified".into(),
                scope: "paragraph".into(),
            },
        ];
        let out = render_citations(&cites);
        assert!(out.contains("[1] mem_a"));
        assert!(out.contains("⚠ not directly traceable"));
        assert!(!render_citations(&[]).contains("Citations"));
    }

    #[test]
    fn read_pages_reads_md_skips_non_md_and_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("one.md"), "---\ntitle: One\n---\n").unwrap();
        std::fs::write(tmp.path().join("two.md"), "---\ntitle: Two\n---\n").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "not a page").unwrap();
        // the reserved OKF bundle index is not a page
        std::fs::write(
            tmp.path().join("index.md"),
            "---\nokf_version: \"0.2\"\n---\n\n* [One](/one.md) - x\n",
        )
        .unwrap();
        // the reserved OKF bundle log is not a page either
        std::fs::write(
            tmp.path().join("log.md"),
            "---\nokf_version: \"0.2\"\n---\n\n- changed something\n",
        )
        .unwrap();
        // a .wenlan/state.json sibling must be ignored (non-recursive, non-md)
        std::fs::create_dir_all(tmp.path().join(".wenlan")).unwrap();
        std::fs::write(tmp.path().join(".wenlan/state.json"), "{}").unwrap();

        let pages = read_pages(tmp.path());
        assert_eq!(
            pages.len(),
            2,
            "only the two page .md files, not index.md or log.md"
        );
        let mut titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, ["One", "Two"]);

        // missing dir -> empty, no panic
        let empty = read_pages(&tmp.path().join("does-not-exist"));
        assert!(empty.is_empty());
    }

    #[test]
    fn read_pages_finds_nested_notes_and_omits_archived_or_control_notes() {
        let dir = tempfile::tempdir().unwrap();
        for folder in ["Research/Rust", "archive", "_sources", ".wenlan"] {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        }
        std::fs::write(
            dir.path().join("Research/Rust/ownership.md"),
            "---\ntitle: Ownership\norigin_id: page_nested\n---\nBody",
        )
        .unwrap();
        std::fs::write(dir.path().join("Research/log.md"), "not a Page").unwrap();
        for control in ["archive/old.md", "_sources/mem_1.md", ".wenlan/private.md"] {
            std::fs::write(dir.path().join(control), "private control").unwrap();
        }
        let pages = read_pages(dir.path());
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].title, "Ownership");
        assert_eq!(pages[0].page_id.as_deref(), Some("page_nested"));
        assert_eq!(pages[0].path, dir.path().join("Research/Rust/ownership.md"));
    }
}
