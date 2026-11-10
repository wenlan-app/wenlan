// SPDX-License-Identifier: Apache-2.0
//! Automatic mode writes new pages with its own writer through steep's
//! new-page claim and commit. These run on the no-embedder scratch database.
//!
//! Holds are process-wide, so every test uses its own source ids.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{release_set_aside, set_aside_groups, write_one_new_page, NewPageOutcome};
use crate::db::code_basis_test::test_db;
use crate::db::MemoryDB;
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::read_scope::ReadScope;
use wenlan_types::SteepSetAsideReason;

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

fn item_ids(group: &wenlan_types::SteepSetAsideGroup) -> Vec<&str> {
    group.items.iter().map(|i| i.item_id.as_str()).collect()
}

#[tokio::test]
async fn a_refused_note_is_listed_by_its_topic_and_a_release_lets_automatic_try_again() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    row(&db, "sa_1", BREAD_1, "claude-code", now).await;
    row(&db, "sa_2", BREAD_2, "claude-code", now).await;
    let model = Scripted::new(
        "TOPIC: Sourdough\nITEMS: sa_1, sa_2",
        "Bread is a food people like.",
    );
    assert_eq!(
        turn(&db, &model).await.unwrap().outcome,
        NewPageOutcome::Failed
    );

    let groups = set_aside_groups(&db, &ReadScope::Global).await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "Sourdough");
    assert_eq!(groups[0].reason, SteepSetAsideReason::SourcesDidNotMatch);
    assert_eq!(item_ids(&groups[0]), vec!["sa_1", "sa_2"]);
    assert_eq!(groups[0].items[0].title, "sa_1");
    assert!(groups[0].set_aside_at > 0);
    // The rows live in Space "test": another Space sees no group and cannot
    // give it back.
    let other = ReadScope::Space("elsewhere".into());
    assert!(set_aside_groups(&db, &other).await.unwrap().is_empty());
    let ids = vec!["sa_1".to_string(), "sa_2".to_string()];
    assert_eq!(release_set_aside(&db, &other, &ids).await.unwrap(), 0);
    assert!(turn(&db, &model).await.is_none(), "still set aside");

    let test_space = ReadScope::Space("test".into());
    assert_eq!(set_aside_groups(&db, &test_space).await.unwrap().len(), 1);
    assert_eq!(release_set_aside(&db, &test_space, &ids).await.unwrap(), 2);
    assert!(set_aside_groups(&db, &ReadScope::Global)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(release_set_aside(&db, &test_space, &ids).await.unwrap(), 0);
    // Given back, Automatic tries the group again (and sets it aside again).
    let again = turn(&db, &model).await.expect("the group waits again");
    assert_eq!(again.outcome, NewPageOutcome::Failed);
    assert_eq!(model.calls(), 4);
    assert_eq!(
        set_aside_groups(&db, &ReadScope::Global)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn short_items_are_listed_as_too_short_under_the_item_title_or_the_named_topic() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    row(&db, "ts_new", "A short line.", "claude-code", now).await;
    row(
        &db,
        "ts_old",
        "Another short line.",
        "claude-code",
        now - 60,
    )
    .await;
    // No usable group: the newest short item goes alone, under its title.
    let unsure = Scripted::new("I am not sure.", BODY);
    turn(&db, &unsure).await.expect("material waits");
    // A named one-item group under the floor: the claim refuses it.
    let named = Scripted::new("TOPIC: Lines\nITEMS: ts_old", BODY);
    turn(&db, &named).await.expect("one item waits");
    assert_eq!(named.calls(), 1, "a refused claim costs no writing call");

    let groups = set_aside_groups(&db, &ReadScope::Global).await.unwrap();
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"ts_new") && names.contains(&"Lines"));
    assert!(groups
        .iter()
        .all(|g| g.reason == SteepSetAsideReason::TooShort));
}

#[tokio::test]
async fn an_item_that_changes_leaves_the_set_aside_list_and_is_tried_again() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    row(&db, "ch_1", BREAD_1, "claude-code", now - 120).await;
    row(&db, "ch_2", BREAD_2, "claude-code", now - 120).await;
    let model = Scripted::new("TOPIC: Bread\nITEMS: ch_1, ch_2", "Bread is good.");
    turn(&db, &model).await.expect("material waits");
    assert_eq!(
        item_ids(&set_aside_groups(&db, &ReadScope::Global).await.unwrap()[0]),
        vec!["ch_1", "ch_2"]
    );
    // A second chunk of ch_1, later: the item changed.
    row(
        &db,
        "ch_1::1",
        "The crumb sets as it cools.",
        "claude-code",
        now,
    )
    .await;
    let groups = set_aside_groups(&db, &ReadScope::Global).await.unwrap();
    assert_eq!(
        item_ids(&groups[0]),
        vec!["ch_2"],
        "only the unchanged item"
    );
    let again = turn(&db, &model).await.expect("the changed item waits");
    assert_eq!(again.outcome, NewPageOutcome::Failed);
}

#[tokio::test]
async fn the_first_set_aside_record_is_read_and_migrated_without_names() {
    let (db, _dir) = test_db().await;
    let now = chrono::Utc::now().timestamp();
    row(&db, "lg_1", BREAD_1, "claude-code", now).await;
    row(&db, "lg_2", BREAD_2, "claude-code", now).await;
    row(&db, "lg_3", "A third item.", "claude-code", now - 60).await;
    let legacy = serde_json::json!([
        format!("lg_1@{now}"),
        format!("lg_2@{now}"),
        "lg_3@not-a-time",
        "no-at-sign",
        format!("gone@{now}")
    ]);
    db.set_app_metadata(super::LEGACY_FAILED_ITEMS_KEY, &legacy.to_string())
        .await
        .unwrap();
    let groups = set_aside_groups(&db, &ReadScope::Global).await.unwrap();
    let mut names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["lg_1", "lg_2"], "named by their items");
    assert!(groups
        .iter()
        .all(|g| g.reason == SteepSetAsideReason::Other));

    // A turn skips them and takes the one item the old record did not hold.
    let model = Scripted::new("TOPIC: Third\nITEMS: lg_3", BODY);
    turn(&db, &model).await.expect("lg_3 waits");
    let groups = set_aside_groups(&db, &ReadScope::Global).await.unwrap();
    assert_eq!(groups.len(), 3, "the migrated groups are kept on write");
    assert_eq!(groups[0].name, "Third", "newest first");
    assert!(turn(&db, &model).await.is_none());

    // An unreadable record is not fatal.
    db.set_app_metadata(super::SET_ASIDE_KEY, "{not json")
        .await
        .unwrap();
    assert!(set_aside_groups(&db, &ReadScope::Global)
        .await
        .unwrap()
        .is_empty());
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
