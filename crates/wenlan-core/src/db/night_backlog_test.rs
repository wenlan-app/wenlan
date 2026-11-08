// SPDX-License-Identifier: Apache-2.0
//! The one-time pass's report reads only (UX-GRAPH-7).

use super::*;
use crate::db::tests::test_db;
use crate::sources::RawDocument;
use std::collections::HashMap as Map;

async fn remember(
    db: &MemoryDB,
    id: &str,
    content: &str,
    agent: Option<&str>,
    space: &str,
    at: i64,
) {
    db.upsert_documents(vec![RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: id.to_string(),
        content: content.to_string(),
        last_modified: at,
        metadata: Map::new(),
        memory_type: Some("fact".to_string()),
        space: Some(space.to_string()),
        source_agent: agent.map(str::to_string),
        ..Default::default()
    }])
    .await
    .unwrap();
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE memories SET created_at = ?2 WHERE source_id = ?1",
        libsql::params![id, at],
    )
    .await
    .unwrap();
}

#[test]
fn ux_graph_7_exact_copies_are_told_from_paraphrases() {
    assert_eq!(
        normalized("The site stays FREE, for now."),
        normalized("the site stays free for now")
    );
    assert_ne!(
        normalized("The site stays free."),
        normalized("The site is free.")
    );
    assert!(may_hide(SentenceAuthor::Ai));
    assert!(may_hide(SentenceAuthor::Original));
    assert!(!may_hide(SentenceAuthor::Person));
    assert!(!may_hide(SentenceAuthor::Unknown));
}

/// UX-GRAPH-7: the backlog report pairs copies within one Space, keeps the
/// earlier, never offers to hide a person's or unknown writer's memory,
/// lists passed dates, stale contradiction checks and old night points, and
/// changes nothing.
#[tokio::test]
async fn ux_graph_7_the_backlog_report_counts_without_changing_anything() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    remember(
        &db,
        "mem_a",
        "The site stays free for now.",
        Some("claude-code"),
        "wenlan",
        now - 9_000,
    )
    .await;
    remember(
        &db,
        "mem_b",
        "The site stays free, for now",
        Some("obsidian"),
        "wenlan",
        now - 5_000,
    )
    .await;
    remember(
        &db,
        "mem_c",
        "The site stays free for now.",
        None,
        "wenlan",
        now - 1_000,
    )
    .await;
    remember(
        &db,
        "mem_d",
        "The site stays free for now.",
        Some("claude-code"),
        "personal",
        now - 500,
    )
    .await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET event_date = ?1 WHERE source_id = 'mem_a'",
            libsql::params![now - 86_400],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO refinement_queue (id, action, source_ids, status)
             VALUES ('r1', 'detect_contradiction', '[]', 'pending'),
                    ('r2', 'detect_contradiction', '[]', 'pending'),
                    ('r3', 'detect_contradiction', '[]', 'awaiting_review')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO thing_map_nodes (thing_id, id, parent_id, rank, kind, label, by, sources, created_at, updated_at)
             VALUES ('thg_1', 'n1', NULL, 1, 'point', 'x', 'ai', '[{\"session_id\":\"s\",\"memory_id\":\"mem_a\"}]', 0, 0)",
            (),
        )
        .await
        .unwrap();
    }

    let report = db.backlog_report(now).await.unwrap();
    assert_eq!(report.memories, 4);
    // Never across Spaces: mem_d is in another Space.
    assert!(
        report
            .pairs
            .iter()
            .all(|p| p.keep.id != "mem_d" && p.hide.id != "mem_d"),
        "{:?}",
        report.pairs
    );
    let ab = report
        .pairs
        .iter()
        .find(|p| p.keep.id == "mem_a" && p.hide.id == "mem_b")
        .expect("the later copy pairs with the earlier");
    assert!(ab.exact);
    // mem_c has no writer: it may be paired, never offered to hide.
    assert!(report.hideable().all(|p| p.hide.id != "mem_c"));
    assert_eq!(
        report
            .passed
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["mem_a"]
    );
    assert_eq!(report.passed[0].author, SentenceAuthor::Ai);
    assert_eq!(
        report.contradictions,
        [
            ("pending".to_string(), 2),
            ("awaiting_review".to_string(), 1)
        ]
    );
    assert_eq!(report.night_points.total, 1);
    assert_eq!(report.night_points.unconfirmed, 1);

    // Nothing changed.
    let again = db.backlog_report(now).await.unwrap();
    assert_eq!(again, report);
}

fn short(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 140 {
        format!("{}…", line.chars().take(140).collect::<String>())
    } else {
        line
    }
}

fn day(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// The backlog report on a copy of a real store, as markdown. Reads only.
/// `WENLAN_BACKLOG_DRY_DIR` holds the copy's `origin_memory.db`;
/// `WENLAN_BACKLOG_DRY_REPORT` is where the report goes.
#[tokio::test]
#[ignore = "runs on a copy of a real store"]
async fn backlog_dry_run() {
    let dir = std::env::var("WENLAN_BACKLOG_DRY_DIR").expect("WENLAN_BACKLOG_DRY_DIR");
    let path = std::env::var("WENLAN_BACKLOG_DRY_REPORT").expect("WENLAN_BACKLOG_DRY_REPORT");
    let db = MemoryDB::new(
        std::path::Path::new(&dir),
        std::sync::Arc::new(crate::events::NoopEmitter),
    )
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp();
    let started = std::time::Instant::now();
    let report = db.backlog_report(now).await.unwrap();
    let author = |a: SentenceAuthor| format!("{a:?}").to_lowercase();
    let mut out = format!(
        "# Old-memory pass: dry run\n\nRead {} live memories in {}s. Nothing was changed.\n\n",
        report.memories,
        started.elapsed().as_secs()
    );
    let hideable: Vec<_> = report.hideable().collect();
    let exact = hideable.iter().filter(|p| p.exact).count();
    let mut by_author: Map<String, usize> = Map::new();
    for pair in &report.pairs {
        *by_author.entry(author(pair.hide.author)).or_default() += 1;
    }
    out.push_str(&format!(
        "## Copies (similarity ≥ {BACKLOG_MIN_SIMILARITY}, same Space)\n\n\
         {} pairs. The later copy was written by: {:?}.\n\
         May be hidden (the later copy is an AI's or an import's): {} — {} exact, {} paraphrase (the model would decide).\n\
         Never hidden (a person's or unknown writer's): {}.\n\n",
        report.pairs.len(),
        by_author,
        hideable.len(),
        exact,
        hideable.len() - exact,
        report.pairs.len() - hideable.len()
    ));
    for (title, exact) in [("Exact", true), ("Paraphrase", false)] {
        out.push_str(&format!("### {title} samples\n\n"));
        for pair in hideable.iter().filter(|p| p.exact == exact).take(10) {
            out.push_str(&format!(
                "- {:.3} · keep {} ({}, {}): {}\n  hide {} ({}, {}): {}\n",
                pair.similarity,
                pair.keep.id,
                author(pair.keep.author),
                day(pair.keep.created_at),
                short(&pair.keep.text),
                pair.hide.id,
                author(pair.hide.author),
                day(pair.hide.created_at),
                short(&pair.hide.text)
            ));
        }
        out.push('\n');
    }
    let mut passed_by: Map<String, usize> = Map::new();
    for memory in &report.passed {
        *passed_by.entry(author(memory.author)).or_default() += 1;
    }
    out.push_str(&format!(
        "## Passed dates\n\n{} memories about a date that has passed, by writer: {:?}. Only the AI's would be rewritten in the past tense.\n\n",
        report.passed.len(),
        passed_by
    ));
    for memory in report
        .passed
        .iter()
        .filter(|m| m.author == SentenceAuthor::Ai)
        .take(15)
    {
        out.push_str(&format!(
            "- {} (about {}, written {}): {}\n",
            memory.id,
            day(memory.event_date.unwrap_or(0)),
            day(memory.created_at),
            short(&memory.text)
        ));
    }
    out.push_str(&format!(
        "\n## Contradiction checks\n\nBy status: {:?}. The pass would mark the `pending` ones expired (kept, not deleted).\n\n",
        report.contradictions
    ));
    let points = &report.night_points;
    out.push_str(&format!(
        "## Old night points\n\n{} points: {} still on the review list, {} unfiled, {} with no writer recorded.\n",
        points.total, points.unconfirmed, points.unfiled, points.no_writer
    ));
    std::fs::write(&path, out).unwrap();
}

async fn mode(db: &MemoryDB, id: &str) -> Option<String> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT supersede_mode FROM memories WHERE source_id = ?1 AND chunk_index = 0",
            libsql::params![id],
        )
        .await
        .unwrap();
    rows.next()
        .await
        .unwrap()
        .and_then(|row| row.get::<Option<String>>(0).unwrap())
}

async fn one(db: &MemoryDB, sql: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, ()).await.unwrap();
    rows.next()
        .await
        .unwrap()
        .map(|row| row.get::<i64>(0).unwrap())
        .unwrap_or(-1)
}

/// UX-GRAPH-7: the one-time pass hides later copies behind the earliest,
/// never a person's or unknown writer's or a thing's point, stands an
/// update's new memory for the old, expires stale checks, takes old night
/// points off the review list, runs once, and Undo takes all of it back.
#[tokio::test]
async fn ux_graph_7_the_old_memory_pass_runs_once_and_undo_takes_it_all_back() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    for (id, agent, at) in [
        ("mem_a", Some("claude-code"), now - 9_000),
        ("mem_b", Some("obsidian"), now - 5_000),
        ("mem_b2", Some("claude-code"), now - 4_000),
        ("mem_c", None, now - 1_000),
        ("mem_old", Some("claude-code"), now - 8_000),
        ("mem_new", Some("wenlan"), now),
    ] {
        remember(
            &db,
            id,
            &format!("{id} says something."),
            agent,
            "wenlan",
            at,
        )
        .await;
    }
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET confirmed = 0 WHERE source_id = 'mem_a'",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO refinement_queue (id, action, source_ids, status)
             VALUES ('r1', 'detect_contradiction', '[]', 'pending'),
                    ('r3', 'detect_contradiction', '[]', 'awaiting_review')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO thing_map_nodes (thing_id, id, parent_id, rank, kind, label, by, sources, created_at, updated_at)
             VALUES ('thg_1', 'n1', NULL, 1, 'point', 'x', 'ai', '[{\"session_id\":\"s\",\"memory_id\":\"mem_a\"}]', 0, 0)",
            (),
        )
        .await
        .unwrap();
    }
    let plan = BacklogPlan {
        // mem_b2 stands behind mem_b, which stands behind mem_a; mem_c has
        // no writer; mem_a is a thing's point and is only ever kept.
        hide: vec![
            ("mem_a".into(), "mem_b".into()),
            ("mem_b".into(), "mem_b2".into()),
            ("mem_a".into(), "mem_c".into()),
            ("mem_b".into(), "mem_a".into()),
        ],
        updated: vec![("mem_old".into(), "mem_new".into())],
    };
    let pass = db.apply_backlog(&plan).await.unwrap();
    assert_eq!((pass.merged(), pass.updated()), (2, 1), "{pass:?}");
    assert_eq!(mode(&db, "mem_b").await.as_deref(), Some("evicted"));
    assert_eq!(mode(&db, "mem_b2").await.as_deref(), Some("evicted"));
    assert_eq!(mode(&db, "mem_old").await.as_deref(), Some("evicted"));
    assert_ne!(mode(&db, "mem_c").await.as_deref(), Some("evicted"));
    assert_ne!(mode(&db, "mem_a").await.as_deref(), Some("evicted"));
    assert_eq!(pass.expired, ["r1"]);
    assert_eq!(
        one(
            &db,
            "SELECT COUNT(*) FROM refinement_queue WHERE status = 'awaiting_review'"
        )
        .await,
        1
    );
    assert_eq!(pass.confirmed, ["mem_a"]);
    assert_eq!(db.backlog_pass().await.unwrap(), Some(pass.clone()));
    assert!(db.apply_backlog(&plan).await.is_err(), "it runs once");

    let undone = db.undo_backlog().await.unwrap().unwrap();
    assert_eq!(undone, pass);
    for id in ["mem_b", "mem_b2", "mem_old"] {
        assert_ne!(mode(&db, id).await.as_deref(), Some("evicted"), "{id}");
    }
    assert_eq!(
        one(
            &db,
            "SELECT COUNT(*) FROM memories WHERE source_id = 'mem_new'"
        )
        .await,
        0,
        "what the pass wrote is forgotten"
    );
    assert_eq!(
        one(
            &db,
            "SELECT COUNT(*) FROM refinement_queue WHERE status = 'pending'"
        )
        .await,
        1
    );
    assert_eq!(
        one(
            &db,
            "SELECT confirmed FROM memories WHERE source_id = 'mem_a' AND chunk_index = 0"
        )
        .await,
        0
    );
    assert_eq!(db.backlog_pass().await.unwrap(), None);
    assert!(db.undo_backlog().await.unwrap().is_none());
    // It can run again after Undo.
    assert!(db.apply_backlog(&plan).await.is_ok());
}
