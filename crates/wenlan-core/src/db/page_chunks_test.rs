// SPDX-License-Identifier: Apache-2.0

use super::{heading_before, merge_page_chunk_hits};
use crate::db::tests::test_db;
use crate::db::MemoryDB;
use crate::pages::Page;
use crate::read_scope::ReadScope;
use wenlan_types::PageSectionHit;

async fn insert(db: &MemoryDB, id: &str, content: &str) {
    db.insert_page_with_kind(
        id,
        "Weekly operations routine",
        None,
        content,
        None,
        None,
        &[],
        &chrono::Utc::now().to_rfc3339(),
        "distilled",
        "confirmed",
        Some("work"),
        None,
    )
    .await
    .unwrap();
}

async fn chunk_versions(db: &MemoryDB, id: &str) -> Vec<i64> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT page_version FROM page_chunks WHERE page_id = ?1 ORDER BY chunk_index",
            libsql::params![id],
        )
        .await
        .unwrap();
    let mut versions = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        versions.push(row.get::<i64>(0).unwrap());
    }
    versions
}

fn hit_page(id: &str, section: Option<PageSectionHit>) -> Page {
    let mut page: Page = serde_json::from_value(serde_json::json!({
        "id": id, "title": id, "summary": null, "content": "", "entity_id": null,
        "source_memory_ids": [], "version": 1, "status": "active", "created_at": "",
        "last_compiled": "", "last_modified": "", "sources_updated_count": 0,
        "stale_reason": null, "user_edited": false
    }))
    .unwrap();
    page.matched_section = section;
    page
}

fn section(index: u32) -> PageSectionHit {
    PageSectionHit {
        index,
        heading: Some(format!("Section {index}")),
        byte_start: 0,
        byte_end: 10,
        page_version: 1,
    }
}

#[test]
fn merge_keeps_the_closer_distance_and_adds_chunk_only_pages() {
    let mut vector = vec![
        ("a".to_string(), 0.30, hit_page("a", None)),
        ("b".to_string(), 0.40, hit_page("b", None)),
        ("d".to_string(), 0.45, hit_page("d", None)),
    ];
    let chunks = vec![
        ("b".to_string(), 0.10, hit_page("b", Some(section(1)))),
        ("a".to_string(), 0.50, hit_page("a", Some(section(2)))),
        ("c".to_string(), 0.20, hit_page("c", Some(section(3)))),
    ];
    merge_page_chunk_hits(&mut vector, chunks, 3);
    let ranked: Vec<(&str, f64, Option<u32>)> = vector
        .iter()
        .map(|(id, d, p)| (id.as_str(), *d, p.matched_section.as_ref().map(|s| s.index)))
        .collect();
    // b: chunk closer, keeps its section. c: found only by a chunk, keeps it.
    // a: the page vector is closer, so no section.
    assert_eq!(
        ranked,
        vec![
            ("b", 0.10, Some(1)),
            ("c", 0.20, Some(3)),
            ("a", 0.30, None)
        ]
    );
}

#[test]
fn heading_is_the_chunks_own_or_the_last_one_before_it() {
    let content = "# Top\n\nintro\n\n## Cooling\n\nThe chiller.\n\nmore text";
    let start = content.find("more text").unwrap();
    assert_eq!(
        heading_before(content, start, "more text").as_deref(),
        Some("Cooling")
    );
    assert_eq!(
        heading_before(content, 0, "# Top\n\nintro").as_deref(),
        Some("Top")
    );
    assert_eq!(heading_before("plain", 0, "plain"), None);
    assert_eq!(heading_before(content, 0, "#hashtag text"), None);
}

/// The page vector sees only the start of a page. A question about a later
/// section must still find the page once its sections are chunked, on the
/// global and the Space-scoped path, and only the current version's chunks
/// count.
#[tokio::test]
async fn page_search_finds_a_page_by_a_late_section() {
    let (db, _tmp) = test_db().await;
    let opening = "Release notes and the routine checklist we follow every week. ".repeat(80);
    insert(
        &db,
        "a-unrelated",
        &format!("# Routine\n\n{opening}\n\n## Parking\n\nParking permits are renewed each June at the front desk."),
    )
    .await;
    insert(
        &db,
        "b-chiller",
        &format!("# Routine\n\n{opening}\n\n## Plant room\n\nThe chiller in building four gets serviced every March by the HVAC contractor."),
    )
    .await;
    // Same page vector for both: only their sections can tell them apart.
    let same = MemoryDB::vec_to_sql(&db.get_or_compute_embedding("weekly release").unwrap());
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET embedding = vector32(?1)",
            libsql::params![same],
        )
        .await
        .unwrap();
    }
    // No word of the question is on either page.
    let question = "when is cooling equipment maintenance";
    let scopes = [ReadScope::Global, ReadScope::Space("work".to_string())];
    for scope in &scopes {
        let results = db
            .search_pages_scoped(question, 10, None, scope)
            .await
            .unwrap();
        assert_eq!(
            results[0].id, "a-unrelated",
            "no chunks yet, id order: {scope:?}"
        );
    }

    assert_eq!(db.count_pages_needing_chunks().await.unwrap(), 2);
    let batch = db.refresh_page_chunks_batch(16).await.unwrap();
    assert_eq!(batch.pages, 2);
    assert!(batch.chunks >= 4, "{batch:?}");
    assert_eq!(db.count_pages_needing_chunks().await.unwrap(), 0);
    for scope in &scopes {
        let results = db
            .search_pages_scoped(question, 10, None, scope)
            .await
            .unwrap();
        assert_eq!(results[0].id, "b-chiller", "{scope:?}: {results:?}");
        let matched = results[0]
            .matched_section
            .as_ref()
            .expect("section pointer");
        assert_eq!(matched.heading.as_deref(), Some("Plant room"), "{scope:?}");
        assert_eq!(matched.page_version, 1);
        let body = &results[0].content[matched.byte_start as usize..matched.byte_end as usize];
        assert!(body.contains("chiller"), "{body}");
    }

    // A new version hides the old chunks until they are rebuilt.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET version = version + 1 WHERE id = 'b-chiller'",
            (),
        )
        .await
        .unwrap();
    }
    assert_eq!(db.count_pages_needing_chunks().await.unwrap(), 1);
    let results = db
        .search_pages_scoped(question, 10, None, &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!(results[0].id, "a-unrelated", "stale chunks must not count");
    assert!(
        results
            .iter()
            .filter(|p| p.id == "b-chiller")
            .all(|p| p.matched_section.is_none()),
        "a stale chunk must not point into the new version"
    );
    db.refresh_page_chunks_batch(16).await.unwrap();
    assert!(chunk_versions(&db, "b-chiller")
        .await
        .iter()
        .all(|v| *v == 2));
    let results = db
        .search_pages_scoped(question, 10, None, &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!(results[0].id, "b-chiller");
    assert_eq!(results[0].matched_section.as_ref().unwrap().page_version, 2);
}

/// A page with no text gets one marker row, so it is not picked every
/// batch; rows of a page that is gone are dropped.
#[tokio::test]
async fn empty_pages_are_marked_and_deleted_pages_pruned() {
    let (db, _tmp) = test_db().await;
    insert(&db, "empty", "").await;
    insert(&db, "full", "# Notes\n\nSomething worth finding.").await;
    let batch = db.refresh_page_chunks_batch(16).await.unwrap();
    assert_eq!(batch.pages, 2);
    assert_eq!(chunk_versions(&db, "empty").await, vec![1]);
    assert_eq!(db.count_pages_needing_chunks().await.unwrap(), 0);
    assert_eq!(db.refresh_page_chunks_batch(16).await.unwrap().pages, 0);

    {
        let conn = db.conn.lock().await;
        conn.execute("DELETE FROM pages WHERE id = 'full'", ())
            .await
            .unwrap();
    }
    db.refresh_page_chunks_batch(16).await.unwrap();
    assert!(chunk_versions(&db, "full").await.is_empty());
}

/// Measurement, not a gate: real pages and paraphrased head/tail questions
/// from `WENLAN_PAGE_CHUNK_MEASURE_DIR` (`pages/` of Markdown files with
/// optional `title`/`description` front matter, `sample.json` with `id` and
/// `path`, `questions.json` with `id`, `q_head`, `q_tail`). Prints top-5 and
/// top-10 hits with page chunk vectors off and on.
#[tokio::test]
#[ignore]
async fn measure_page_chunks_on_real_pages() {
    let Ok(root) = std::env::var("WENLAN_PAGE_CHUNK_MEASURE_DIR") else {
        eprintln!("WENLAN_PAGE_CHUNK_MEASURE_DIR unset; nothing to measure");
        return;
    };
    let root = std::path::PathBuf::from(root);
    let (db, _tmp) = test_db().await;
    let pages_dir = root.join("pages");
    let mut stack = vec![pages_dir.clone()];
    let mut count = 0usize;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let rel = path
                .strip_prefix(&pages_dir)
                .unwrap()
                .to_string_lossy()
                .to_string();
            let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string();
            let (mut title, mut summary, mut body) =
                (rel.trim_end_matches(".md").to_string(), None, text.clone());
            if let Some(rest) = text.strip_prefix("---\n") {
                if let Some(end) = rest.find("\n---") {
                    for line in rest[..end].lines() {
                        let value = |key: &str| {
                            line.strip_prefix(key)
                                .map(|v| v.trim().trim_matches('"').to_string())
                        };
                        if let Some(v) = value("title:") {
                            title = v;
                        } else if let Some(v) = value("description:") {
                            summary = Some(v);
                        }
                    }
                    body = rest[end + 4..].trim_start_matches('\n').to_string();
                }
            }
            // The stored page text, without the exported Sources block.
            let body = crate::export::provenance::canonicalize_page_body(&body);
            db.insert_page_with_kind(
                &rel,
                &title,
                summary.as_deref(),
                &body,
                None,
                None,
                &[],
                &chrono::Utc::now().to_rfc3339(),
                "distilled",
                "confirmed",
                None,
                None,
            )
            .await
            .unwrap();
            count += 1;
        }
    }
    let mut chunks = 0;
    loop {
        let batch = db.refresh_page_chunks_batch(64).await.unwrap();
        if batch.pages == 0 {
            break;
        }
        chunks += batch.chunks;
    }
    let read = |name: &str| -> Vec<serde_json::Value> {
        serde_json::from_str(&std::fs::read_to_string(root.join(name)).unwrap()).unwrap()
    };
    let sample = read("sample.json");
    let questions = read("questions.json");
    let mut cases = Vec::new();
    for item in &sample {
        let q = questions.iter().find(|q| q["id"] == item["id"]).unwrap();
        for part in ["head", "tail"] {
            if let Some(text) = q[format!("q_{part}")].as_str().filter(|t| !t.is_empty()) {
                cases.push((
                    part,
                    text.to_string(),
                    item["path"].as_str().unwrap().to_string(),
                ));
            }
        }
    }
    eprintln!("pages {count} chunks {chunks} questions {}", cases.len());
    for setting in ["0", "1"] {
        std::env::set_var("WENLAN_ENABLE_PAGE_CHUNK_VECTORS", setting);
        for limit in [5usize, 10] {
            for part in ["head", "tail"] {
                let (mut hits, mut total) = (0, 0);
                for (p, question, target) in &cases {
                    if p != &part {
                        continue;
                    }
                    total += 1;
                    let results = db
                        .search_pages_scoped(question, limit, None, &ReadScope::Global)
                        .await
                        .unwrap();
                    hits += usize::from(results.iter().any(|page| &page.id == target));
                }
                eprintln!("chunks={setting} top{limit} {part}: {hits}/{total}");
            }
        }
    }
    std::env::remove_var("WENLAN_ENABLE_PAGE_CHUNK_VECTORS");
}
