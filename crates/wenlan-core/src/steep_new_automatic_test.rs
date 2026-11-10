// SPDX-License-Identifier: Apache-2.0
//! Automatic mode writes new pages with its own writer through steep's
//! new-page claim and commit. These run on the no-embedder scratch database.
//!
//! Holds are process-wide, so every test uses its own source ids.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{write_one_new_page, NewPageOutcome};
use crate::db::code_basis_test::test_db;
use crate::db::MemoryDB;
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::read_scope::ReadScope;

/// Answers the grouping call with `group` and the writing call with `body`,
/// and counts the calls.
struct Scripted {
    group: String,
    body: String,
    calls: AtomicUsize,
    labels: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(group: &str, body: &str) -> Arc<Self> {
        Arc::new(Self {
            group: group.to_string(),
            body: body.to_string(),
            calls: AtomicUsize::new(0),
            labels: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for Scripted {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let label = request.label.unwrap_or_default();
        self.labels.lock().unwrap().push(label.clone());
        Ok(if label == "steep_new_group" {
            self.group.clone()
        } else {
            self.body.clone()
        })
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "scripted"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::OnDevice
    }
}

const BREAD_1: &str = "The sourdough starter peaks four to six hours after feeding.";
const BREAD_2: &str = "Below 22 degrees the bulk ferment needs about eight hours.";
const BODY: &str = "The sourdough starter peaks four to six hours after feeding [1]. \
                    Below 22 degrees the bulk ferment needs about eight hours [2].";

async fn row(db: &MemoryDB, id: &str, content: &str, agent: &str, at: i64) {
    let conn = db.test_primary_session().await;
    conn.execute(
        "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
         VALUES (?1, ?1, ?1, ?2, 0, 'text', 'fact', 'test', ?3, ?4, ?4, 1, 'confirmed', 'memory')",
        libsql::params![id, content, agent, at],
    )
    .await
    .unwrap();
}

async fn waiting(db: &MemoryDB, prefix: &str) -> Vec<String> {
    crate::steep_new::new_material(db, &ReadScope::Global)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.item.item_id)
        .filter(|id| id.starts_with(prefix))
        .collect()
}

async fn turn(db: &MemoryDB, model: &Arc<Scripted>) -> Option<super::NewPageTurn> {
    let llm: Arc<dyn LlmProvider> = model.clone();
    write_one_new_page(db, &llm, &PromptRegistry::default(), None)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_folder_import_becomes_a_cited_note_in_one_turn_of_two_calls() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    // Folder documents only: the clustering seed floor never lets these seed
    // a page; the new-page claim takes them.
    row(&db, "fa_1", BREAD_1, "folder", now).await;
    row(&db, "fa_2", BREAD_2, "folder", now).await;
    row(
        &db,
        "fa_other",
        "A note about bicycles.",
        "folder",
        now - 60,
    )
    .await;
    let model = Scripted::new("TOPIC: Sourdough timing\nITEMS: fa_1, fa_2", BODY);

    let done = turn(&db, &model).await.expect("material waits");
    let NewPageOutcome::Written { page_id } = done.outcome else {
        panic!("expected a written note: {done:?}");
    };
    assert!(done.more, "the bicycle note still waits");
    assert_eq!(model.calls(), 2, "one call to group, one to write");
    let page = db.get_page(&page_id).await.unwrap().unwrap();
    assert_eq!(page.title, "Sourdough timing");
    assert_eq!(page.citations.len(), 2);
    assert_eq!(waiting(&db, "fa_").await, vec!["fa_other".to_string()]);
}

#[tokio::test]
async fn a_refused_note_is_set_aside_and_the_next_turn_moves_on() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    row(&db, "rf_1", BREAD_1, "claude-code", now).await;
    row(&db, "rf_2", BREAD_2, "claude-code", now).await;
    // No citation the check can verify: the commit is refused.
    let model = Scripted::new(
        "TOPIC: Sourdough\nITEMS: rf_1, rf_2",
        "Bread is a food people like.",
    );

    let done = turn(&db, &model).await.expect("material waits");
    assert_eq!(done.outcome, NewPageOutcome::Failed);
    assert_eq!(model.calls(), 2);
    // The items are given back (not held) but not tried again as they are.
    let listed = crate::steep_new::new_material(&db, &ReadScope::Global)
        .await
        .unwrap();
    assert!(listed
        .iter()
        .filter(|m| m.item.item_id.starts_with("rf_"))
        .all(|m| !m.item.held));
    assert!(turn(&db, &model).await.is_none());
    assert_eq!(model.calls(), 2, "a set-aside group costs no further call");
}

#[tokio::test]
async fn a_reply_naming_no_group_sets_the_newest_short_item_aside() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    row(&db, "ng_new", "A short line.", "claude-code", now).await;
    row(
        &db,
        "ng_old",
        "Another short line.",
        "claude-code",
        now - 60,
    )
    .await;
    let model = Scripted::new("I am not sure.", BODY);

    let done = turn(&db, &model).await.expect("material waits");
    assert_eq!(done.outcome, NewPageOutcome::Failed);
    assert!(done.more);
    assert_eq!(model.calls(), 1, "nothing to write, so no writing call");
    // The next turn offers only the other item.
    let again = turn(&db, &model).await.expect("one item still waits");
    assert_eq!(again.outcome, NewPageOutcome::Failed);
    assert!(!again.more);
    assert!(turn(&db, &model).await.is_none());
}

#[test]
fn the_group_reply_is_read_by_short_or_full_id() {
    let item = |id: &str| crate::steep_new::NewMaterial {
        item: wenlan_types::SteepNewItem {
            item_id: id.to_string(),
            kind: "capture".to_string(),
            title: id.to_string(),
            snippet: String::new(),
            space: None,
            chars: 10,
            last_modified: 0,
            held: false,
        },
        source_ids: vec![id.to_string()],
    };
    let a = item("pg_a");
    let b = item("pg_b");
    let c = item("pg_c");
    let offered = vec![&a, &b, &c];
    let reply = format!(
        "**TOPIC:** \"Bread\"\nITEMS: {}, pg_c, unknown",
        wenlan_types::steep_short_id("pg_a")
    );
    let (topic, group) = super::parse_group(&reply, &offered).unwrap();
    assert_eq!(topic, "Bread");
    let ids: Vec<&str> = group.iter().map(|m| m.item.item_id.as_str()).collect();
    assert_eq!(ids, vec!["pg_a", "pg_c"]);
    assert!(super::parse_group("ITEMS: pg_a", &offered).is_none());
    assert!(super::parse_group("TOPIC: Bread\nITEMS: nothing", &offered).is_none());
}

#[tokio::test]
async fn the_paid_page_count_persists_for_the_day_and_starts_again_the_next_day() {
    use super::{note_paid_new_pages, paid_new_pages_on, PAID_NEW_PAGES_PER_DAY};
    let (db, _dir) = test_db().await;
    assert_eq!(paid_new_pages_on(&db, "2026-11-10").await.unwrap(), 0);
    assert_eq!(note_paid_new_pages(&db, "2026-11-10", 5).await.unwrap(), 5);
    // Nothing is held in memory: a restarted daemon reads the same count
    // back from the database.
    assert_eq!(note_paid_new_pages(&db, "2026-11-10", 3).await.unwrap(), 8);
    assert_eq!(
        paid_new_pages_on(&db, "2026-11-10").await.unwrap(),
        PAID_NEW_PAGES_PER_DAY
    );
    // The next local day starts from zero.
    assert_eq!(paid_new_pages_on(&db, "2026-11-11").await.unwrap(), 0);
    assert_eq!(note_paid_new_pages(&db, "2026-11-11", 1).await.unwrap(), 1);
    assert_eq!(paid_new_pages_on(&db, "2026-11-10").await.unwrap(), 0);
    assert_eq!(PAID_NEW_PAGES_PER_DAY, 8);
}
