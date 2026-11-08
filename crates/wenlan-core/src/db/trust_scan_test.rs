// SPDX-License-Identifier: Apache-2.0
//! The nightly trust scan (`crate::trust_scan`) and its DB half: migration
//! 159, the stored report, and the citation backfill re-queue.
//!
//! The damage and CJK findings carry no UX rule id: no rule in
//! docs/ux/spec.md covers damage already in a page (a new rule is proposed
//! in the report). The source and backfill findings are UX-WHO-5's: an
//! unverified source can't be hidden and can't look verified.

use crate::db::tests::test_db;
use crate::db::MemoryDB;
use crate::sources::RawDocument;
use crate::trust_scan::{latest, run_and_store, run_trust_scan};
use std::collections::HashMap;
use wenlan_types::trust_scan::PageDamage;

const NOW: i64 = 1_800_000_000;
const ZH: &str = "我們決定先在本地保存所有筆記，再慢慢同步到雲端，這樣離線時也能照常使用。";

async fn page(db: &MemoryDB, id: &str, kind: &str, content: &str, version: i64, listed: &[&str]) {
    let creation_kind = if kind == "authored" {
        "authored"
    } else {
        "distilled"
    };
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO pages (
             id, title, content, source_memory_ids, status, created_at,
             last_compiled, last_modified, creation_kind, kind, version
         ) VALUES (?1, ?2, ?3, ?4, 'active', 'now', 'now', 'now', ?5, ?6, ?7)",
        libsql::params![
            id,
            format!("Title {id}"),
            content,
            serde_json::to_string(listed).unwrap(),
            creation_kind,
            kind,
            version,
        ],
    )
    .await
    .unwrap();
}

async fn history(db: &MemoryDB, id: &str, version: i64, content: &str, edited_by: &str) {
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO page_history (page_id, version, content, source_memory_ids, edited_by, created_at)
         VALUES (?1, ?2, ?3, '[]', ?4, 0)",
        libsql::params![id, version, content, edited_by],
    )
    .await
    .unwrap();
}

async fn remember(db: &MemoryDB, id: &str) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: id.to_string(),
        content: format!("memory {id}"),
        last_modified: NOW,
        metadata: HashMap::new(),
        memory_type: Some("fact".to_string()),
        ..Default::default()
    }])
    .await
    .unwrap();
}

async fn exec(db: &MemoryDB, sql: &str) {
    let conn = db.conn.lock().await;
    conn.execute_batch(sql).await.unwrap();
}

async fn count(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

#[tokio::test]
async fn migration_159_adds_one_report_row_that_the_next_scan_replaces() {
    let (db, _dir) = test_db().await;
    assert_eq!(
        count(&db, "PRAGMA user_version").await,
        i64::from(super::SCHEMA_VERSION)
    );
    const { assert!(super::SCHEMA_VERSION >= 159) };
    assert!(latest(&db).await.unwrap().is_none());

    run_and_store(&db, None, NOW).await.unwrap();
    run_and_store(&db, None, NOW + 86_400).await.unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM trust_scan_reports").await,
        1
    );
    assert_eq!(latest(&db).await.unwrap().unwrap().ran_at, NOW + 86_400);

    // Replaying the migration (a crash before its version bump) converges.
    db.migrate_159_trust_scan_reports(158).await.unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM trust_scan_reports").await,
        1
    );
}

#[tokio::test]
async fn ux_who_8_trust_scan_reports_cut_off_half_link_and_broken_pages_with_an_excerpt() {
    let (db, _dir) = test_db().await;
    page(&db, "p_fine", "concept", "A finished sentence.", 1, &[]).await;
    page(
        &db,
        "p_cut",
        "concept",
        "The plan has three parts.\n\nThe first part covers the launch and then",
        1,
        &[],
    )
    .await;
    page(
        &db,
        "p_link",
        "concept",
        "See the decision.\n\nRelated: [[mem_e5c607eb6",
        1,
        &[],
    )
    .await;
    page(
        &db,
        "p_broken",
        "concept",
        "我們決定\u{FFFD}先在本地保存。",
        1,
        &[],
    )
    .await;

    let report = run_trust_scan(&db, None, NOW).await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let found: HashMap<&str, _> = report
        .damaged_pages
        .iter()
        .map(|d| (d.page.page_id.as_str(), d))
        .collect();
    assert!(!found.contains_key("p_fine"));
    assert_eq!(found["p_cut"].damage, vec![PageDamage::CutOff]);
    assert!(found["p_cut"].excerpt.ends_with("the launch and then"));
    assert_eq!(
        found["p_link"].damage,
        vec![PageDamage::HalfLink, PageDamage::CutOff]
    );
    assert_eq!(found["p_link"].excerpt, "[[mem_e5c607eb6");
    assert_eq!(found["p_broken"].damage, vec![PageDamage::BrokenCharacters]);
    assert!(found["p_broken"].excerpt.contains('\u{FFFD}'));
    assert_eq!(found["p_cut"].page.title, "Title p_cut");
    assert_eq!(found["p_cut"].page.kind, "concept");
    assert_eq!(report.counts.cut_off, 2);
    assert_eq!(report.counts.half_links, 1);
    assert_eq!(report.counts.broken_characters, 1);
}

#[tokio::test]
async fn ux_who_8_trust_scan_says_whether_the_damage_sits_in_the_persons_sentences() {
    let (db, _dir) = test_db().await;
    // An AI wrote it and nobody touched it: the AI may repair it.
    let ai = "Wenlan keeps notes local.\n\nIt syncs \u{FFFD} later.";
    page(&db, "p_ai", "concept", ai, 1, &[]).await;
    history(&db, "p_ai", 1, ai, "page_growth").await;
    // The broken sentence came from page growth, and the person then saved
    // it in their own edit: now it is theirs.
    let grown = "Wenlan keeps notes local.\n\nIt syncs \u{FFFD} later.";
    let kept = "Wenlan keeps notes local, always.\n\nIt syncs \u{FFFD} later, too.";
    page(&db, "p_kept", "concept", kept, 2, &[]).await;
    history(&db, "p_kept", 1, grown, "page_growth").await;
    history(&db, "p_kept", 2, kept, "manual_edit").await;

    let report = run_trust_scan(&db, None, NOW).await;
    let found: HashMap<&str, bool> = report
        .damaged_pages
        .iter()
        .map(|d| (d.page.page_id.as_str(), d.in_person_sentences))
        .collect();
    assert!(!found["p_ai"]);
    assert!(found["p_kept"]);
}

#[tokio::test]
async fn ux_who_8_trust_scan_reports_cjk_lost_or_broken_by_the_latest_version() {
    let (db, _dir) = test_db().await;
    let before = format!("{ZH}\n\n{ZH}");
    // Lost most of its Chinese text in an English rewrite.
    let lost = "We keep every note local first and sync to the cloud later.";
    page(&db, "p_lost", "concept", lost, 2, &[]).await;
    history(&db, "p_lost", 1, &before, "distill").await;
    history(&db, "p_lost", 2, lost, "page_growth").await;
    // Shortened, but still Chinese: an honest edit.
    page(&db, "p_short", "concept", ZH, 2, &[]).await;
    history(&db, "p_short", 1, &before, "distill").await;
    history(&db, "p_short", 2, ZH, "page_growth").await;
    // Gained a broken character.
    let broken = ZH.replacen('先', "\u{FFFD}", 1);
    page(&db, "p_broke", "concept", &broken, 2, &[]).await;
    history(&db, "p_broke", 1, ZH, "distill").await;
    history(&db, "p_broke", 2, &broken, "page_growth").await;

    let report = run_trust_scan(&db, None, NOW).await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let found: HashMap<&str, _> = report
        .cjk_losses
        .iter()
        .map(|l| (l.page.page_id.as_str(), l))
        .collect();
    assert_eq!(found.len(), 2, "{:?}", found.keys());
    let lost = found["p_lost"];
    assert_eq!((lost.previous_version, lost.version), (1, 2));
    assert_eq!(lost.edited_by, "page_growth");
    assert!(lost.cjk_before >= 60 && lost.cjk_after == 0);
    let broke = found["p_broke"];
    assert_eq!((broke.broken_before, broke.broken_after), (0, 1));
    assert_eq!(report.counts.cjk_losses, 2);
}

#[tokio::test]
async fn ux_who_5_trust_scan_reports_source_mismatches_and_sourceless_pages() {
    let (db, _dir) = test_db().await;
    for id in ["mem_a", "mem_b"] {
        remember(&db, id).await;
    }
    page(&db, "p_agree", "concept", "Agrees.", 1, &[]).await;
    db.link_page_source("p_agree", "mem_a", "test")
        .await
        .unwrap();
    exec(
        &db,
        "UPDATE pages SET source_memory_ids = '[\"mem_a\"]' WHERE id = 'p_agree'",
    )
    .await;
    page(&db, "p_differ", "concept", "Differs.", 1, &[]).await;
    db.link_page_source("p_differ", "mem_a", "test")
        .await
        .unwrap();
    exec(
        &db,
        "UPDATE pages SET source_memory_ids = '[\"mem_b\"]' WHERE id = 'p_differ'",
    )
    .await;
    page(&db, "p_bare", "concept", "Rests on nothing.", 1, &[]).await;
    page(&db, "p_note", "authored", "The person's own note.", 1, &[]).await;

    let report = run_trust_scan(&db, None, NOW).await;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.source_mismatches.len(), 1);
    let differ = &report.source_mismatches[0];
    assert_eq!(differ.page.page_id, "p_differ");
    assert_eq!((differ.listed, differ.linked), (1, 1));
    assert_eq!(differ.listed_only, vec!["mem_b".to_string()]);
    assert_eq!(differ.linked_only, vec!["mem_a".to_string()]);
    let sourceless: Vec<&str> = report
        .sourceless_pages
        .iter()
        .map(|p| p.page_id.as_str())
        .collect();
    assert_eq!(sourceless, vec!["p_bare"]);
}

#[tokio::test]
async fn ux_who_5_trust_scan_reports_what_the_orphan_cleanup_removes_and_changes_nothing() {
    let (db, _dir) = test_db().await;
    remember(&db, "mem_live").await;
    remember(&db, "mem_gone").await;
    page(&db, "p_src", "concept", "Rests on two notes.", 1, &[]).await;
    db.link_page_source("p_src", "mem_live", "test")
        .await
        .unwrap();
    db.link_page_source("p_src", "mem_gone", "test")
        .await
        .unwrap();
    exec(
        &db,
        "UPDATE pages SET source_memory_ids = '[\"mem_live\",\"mem_gone\",\"mem_never\"]'
          WHERE id = 'p_src';
         DELETE FROM memories WHERE source_id = 'mem_gone';",
    )
    .await;
    let state = "SELECT (SELECT group_concat(source_memory_ids) FROM pages)
                    || '|' || (SELECT COUNT(*) FROM edges WHERE valid_until IS NULL)";
    let before: String = {
        let conn = db.conn.lock().await;
        let mut rows = conn.query(state, ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    };

    let report = run_and_store(&db, None, NOW).await.unwrap();
    assert_eq!(report.dangling_sources.links, 1);
    assert_eq!(report.dangling_sources.listed, 2);
    assert_eq!(report.dangling_sources.pages.len(), 1);
    assert_eq!(
        report.dangling_sources.pages[0].missing,
        vec!["mem_gone".to_string(), "mem_never".to_string()]
    );
    let after: String = {
        let conn = db.conn.lock().await;
        let mut rows = conn.query(state, ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    };
    assert_eq!(before, after, "the scan must not change pages or links");

    // The cleanup removes exactly the pairs the report named.
    assert_eq!(db.cleanup_orphaned_page_sources().await.unwrap(), 2);
    let again = run_trust_scan(&db, None, NOW).await;
    assert_eq!(
        again.dangling_sources.links + again.dangling_sources.listed,
        0
    );
}

const GAVE_UP: &str = r#"[{"edited_by":"citation_backfill","citations_summary":"citation backfill gave up: zero markers after 3 attempts","at":1799999000,"version":1}]"#;

#[tokio::test]
async fn ux_who_5_trust_scan_lists_backfill_give_ups_and_requeue_puts_one_back() {
    let (db, _dir) = test_db().await;
    page(&db, "p_gave_up", "concept", "Uncited.", 1, &[]).await;
    page(&db, "p_cited", "concept", "Cited.", 1, &[]).await;
    exec(
        &db,
        &format!(
            "UPDATE pages SET citations = '[]', changelog = '{GAVE_UP}' WHERE id = 'p_gave_up';
             UPDATE pages SET citations = '[]', changelog = '[]' WHERE id = 'p_cited';
             INSERT INTO app_metadata (key, value) VALUES ('{}', '3');",
            crate::citations::attempt_key("p_gave_up")
        ),
    )
    .await;

    let report = run_trust_scan(&db, None, NOW).await;
    assert_eq!(report.backfill_give_ups.len(), 1);
    let give_up = &report.backfill_give_ups[0];
    assert_eq!(give_up.page.page_id, "p_gave_up");
    assert_eq!(give_up.page.title, "Title p_gave_up");
    assert_eq!(
        give_up.reason,
        "citation backfill gave up: zero markers after 3 attempts"
    );
    assert_eq!(give_up.gave_up_at, Some(1_799_999_000));
    // The scan does not re-queue anything by itself.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM pages WHERE citations IS NULL").await,
        0
    );

    assert!(!db.requeue_citation_backfill("p_cited").await.unwrap());
    assert!(db.requeue_citation_backfill("p_gave_up").await.unwrap());
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM pages WHERE id = 'p_gave_up' AND citations IS NULL"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM app_metadata WHERE key LIKE 'citation_backfill_attempts:%'"
        )
        .await,
        0
    );
    assert!(!db.requeue_citation_backfill("p_gave_up").await.unwrap());
    assert!(run_trust_scan(&db, None, NOW)
        .await
        .backfill_give_ups
        .is_empty());
}

/// Real counts on a copy of the person's database. Run by hand:
/// `TRUST_SCAN_DB=<folder holding a copied origin_memory.db> cargo test -p
/// wenlan-core --lib trust_scan_on_a_copy -- --ignored --nocapture`. The
/// copy is migrated in place, so never point it at the live data folder.
#[tokio::test]
#[ignore = "needs TRUST_SCAN_DB, a folder with a copy of a real database"]
async fn trust_scan_on_a_copy() {
    let path = std::env::var("TRUST_SCAN_DB").expect("TRUST_SCAN_DB");
    let knowledge = std::env::var("TRUST_SCAN_PAGES").ok();
    let db = MemoryDB::new_with_embedding_recovery(
        std::path::Path::new(&path),
        std::sync::Arc::new(crate::events::NoopEmitter),
        super::MemoryEmbeddingRecovery::Deferred,
    )
    .await
    .unwrap();
    let report = run_trust_scan(
        &db,
        knowledge.as_deref().map(std::path::Path::new),
        chrono::Utc::now().timestamp(),
    )
    .await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
