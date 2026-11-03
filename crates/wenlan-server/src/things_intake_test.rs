// SPDX-License-Identifier: Apache-2.0

use super::*;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Mutex;
use wenlan_core::events::NoopEmitter;
use wenlan_core::llm_provider::{LlmBackend, LlmError};
use wenlan_core::read_scope::ReadScope;

/// Answers in turn from a list, and keeps the prompts it was given.
struct Scripted {
    answers: Mutex<Vec<String>>,
    prompts: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(answers: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.iter().rev().map(|a| a.to_string()).collect()),
            prompts: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl LlmProvider for Scripted {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.prompts.lock().unwrap().push(request.user_prompt);
        self.answers
            .lock()
            .unwrap()
            .pop()
            .ok_or(LlmError::NotAvailable)
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "scripted"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

fn conv(id: &str, said: &[&str], minutes_ago: i64) -> FoundConversation {
    let now = chrono::Utc::now().timestamp();
    FoundConversation {
        tool: "claude-code".into(),
        external_id: id.into(),
        path: PathBuf::from(format!("/home/me/.claude/projects/-w/{id}.jsonl")),
        len: 100,
        cwd: Some("/w/wenlan".into()),
        branch: None,
        said: said.iter().map(|s| s.to_string()).collect(),
        last_reply: "done".into(),
        first_at: now - minutes_ago * 60 - 600,
        last_at: now - minutes_ago * 60,
        automated: false,
        on_behalf: false,
    }
}

#[tokio::test]
async fn ux_thing_1_conversations_become_things_without_anyone_telling_wenlan() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.create_space("wenlan", None, false).await.unwrap();
    let found = vec![
        conv(
            "a",
            &["Make home show what I'm working on", "smaller cards"],
            30,
        ),
        conv("b", &["Home: add a hide button", "ok ship it"], 10),
        conv("c", &["what is the capital of Peru"], 5),
        conv("d", &["say hi", "thanks"], 3),
    ];
    let label = Scripted::new(&[
        // In the order found: a, b, d (c is left out by the rules: one message).
        r#"{"work": true, "title": "首頁改版", "space": "wenlan", "about": "首頁顯示正在做的事", "status": "卡片變小了"}"#,
        r#"{"work": true, "title": "首頁卡片", "space": "wenlan", "about": "卡片可以藏起來", "status": "藏起來的按鈕做好了"}"#,
        r#"{"work": false, "title": "", "space": "", "about": "", "status": ""}"#,
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "首頁改版", "conversations": ["C0", "C1"], "now": "卡片和藏起來的按鈕都好了，接著調間距"}]}"#,
    ]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };

    let mut backoff = Backoff::default();
    let report = look_with(
        &db,
        found.clone(),
        &models,
        "Traditional Chinese",
        &mut backoff,
    )
    .await
    .unwrap();
    assert_eq!(
        report,
        LookReport {
            found: 4,
            kept: 3,
            labelled: 3,
            work: 2,
            grouped: 2,
            things_made: 1,
            ..Default::default()
        }
    );
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 1);
    assert_eq!(things[0].title.as_deref(), Some("首頁改版"));
    // Where it stands is the grouping model's line for the whole thing.
    assert_eq!(
        things[0].status_line.as_deref(),
        Some("卡片和藏起來的按鈕都好了，接著調間距")
    );
    assert_eq!(things[0].spaces, vec!["wenlan".to_string()]);
    assert!(!things[0].quiet);

    // The grouping model saw only the labels, never what was said.
    let grouped = group.prompts.lock().unwrap().join("\n");
    assert!(grouped.contains("首頁卡片") && grouped.contains("in Traditional Chinese"));
    assert!(!grouped.contains("hide button") && !grouped.contains("smaller cards"));

    // Looking again with nothing new reads nothing and asks nobody.
    let report = look_with(&db, found, &models, "Traditional Chinese", &mut backoff)
        .await
        .unwrap();
    assert_eq!((report.labelled, report.grouped), (0, 0));
    assert_eq!(label.prompts.lock().unwrap().len(), 3);
    assert_eq!(group.prompts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn ux_thing_1_a_model_that_keeps_failing_is_not_asked_every_look() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let found = vec![
        conv(
            "a",
            &["Make home show what I'm working on", "smaller cards"],
            30,
        ),
        conv("b", &["Plan the Lisbon trip", "book flights"], 10),
    ];
    // The everyday model cannot read "a"; the grouping model answers nonsense.
    let label = Scripted::new(&[
        r#"{"work": true, "title": "Lisbon trip", "space": "", "about": "trip", "status": "Flights booked"}"#,
        "not json",
    ]);
    let group = Scripted::new(&["no idea"]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };
    let mut backoff = Backoff::default();
    assert!(
        look_with(&db, found.clone(), &models, "English", &mut backoff)
            .await
            .is_err()
    );
    assert_eq!(label.prompts.lock().unwrap().len(), 2);
    assert_eq!(group.prompts.lock().unwrap().len(), 1);

    // The next look asks neither model again.
    let report = look_with(&db, found, &models, "English", &mut backoff)
        .await
        .unwrap();
    assert_eq!((report.labelled, report.grouped), (0, 0));
    assert_eq!(label.prompts.lock().unwrap().len(), 2);
    assert_eq!(group.prompts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn ux_thing_2_a_conversation_moved_once_steers_the_next_grouping() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.create_space("wenlan", None, false).await.unwrap();
    let first = vec![conv("z1", &["sync Zotero into drafts", "ok"], 30)];
    let label = Scripted::new(&[
        r#"{"work": true, "title": "Zotero 同步", "space": "wenlan", "about": "把 Zotero 同步成草稿", "status": "同步好了"}"#,
        r#"{"work": true, "title": "Zotero 草稿", "space": "wenlan", "about": "Zotero 的草稿格式", "status": "格式改好了"}"#,
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "Wenlan 推廣", "conversations": ["C0"], "now": "同步好了"}]}"#,
        r#"{"join": [{"conversation": "C0", "piece": "T0"}], "now": [{"piece": "T0", "line": "草稿格式也好了"}]}"#,
    ]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };
    let mut backoff = Backoff::default();
    look_with(
        &db,
        first.clone(),
        &models,
        "Traditional Chinese",
        &mut backoff,
    )
    .await
    .unwrap();
    // The model got it wrong; the person moves it once.
    let z1 = found_session_id(&first[0]).unwrap();
    let wrong = db.session_things(&z1).await.unwrap()[0].id.clone();
    db.move_session_thing(
        &z1,
        &wrong,
        &wenlan_core::db::things::MoveTarget::NewThing("Zotero 工作流".into()),
    )
    .await
    .unwrap();

    // A new conversation like it: the grouping model is told how the person sorted that case.
    let mut second = first;
    second.push(conv("z2", &["fix the Zotero draft format", "ok"], 5));
    look_with(&db, second, &models, "Traditional Chinese", &mut backoff)
        .await
        .unwrap();
    let prompts = group.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 2);
    assert!(!prompts[0].contains("corrected"));
    assert!(prompts[1].contains(
        "- \"Zotero 同步\" (把 Zotero 同步成草稿): not \"Wenlan 推廣\" but \"Zotero 工作流\""
    ));
    // And the person's placement stays where they put it.
    let titles: Vec<_> = db
        .session_things(&z1)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.title)
        .collect();
    assert_eq!(titles, vec!["Zotero 工作流".to_string()]);
}

/// Stores points straight into the database, as the daemon's store would.
struct Keep<'a>(&'a MemoryDB, std::sync::atomic::AtomicUsize);

impl PointWriter for Keep<'_> {
    fn write(&self, request: StoreMemoryRequest) -> Written<'_> {
        Box::pin(async move {
            let n = self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let id = format!("mem_{n}");
            self.0
                .upsert_documents(vec![wenlan_core::sources::RawDocument {
                    source: "memory".into(),
                    source_id: id.clone(),
                    title: id.clone(),
                    content: request.content,
                    last_modified: chrono::Utc::now().timestamp(),
                    memory_type: request.memory_type,
                    supersedes: request.supersedes,
                    space: request.space.named().map(str::to_string),
                    ..Default::default()
                }])
                .await
                .map_err(|e| e.to_string())?;
            Ok(id)
        })
    }
}

#[test]
fn ux_chat_5_points_are_read_at_night_or_after_a_day() {
    let now = 1_000_000;
    assert_eq!(dream_quiet_since(now, 3), now - 30 * 60);
    assert_eq!(dream_quiet_since(now, 14), now - 86_400);
}

/// UX-CHAT-5: at night, what a thing's conversations settled becomes memories
/// on its page; the next night sees them and replaces the one that changed.
#[tokio::test]
async fn ux_chat_5_a_nights_reading_keeps_what_the_work_settled() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.create_space("wenlan", None, false).await.unwrap();
    let mut a = conv(
        "a",
        &[
            "Tagline: go with You do the thinking",
            "pricing page later?",
        ],
        90,
    );
    a.last_reply = "Done: the tagline is in place.".into();
    let found = vec![
        a.clone(),
        conv("b", &["make the hero image smaller", "ok ship it"], 60),
    ];
    let label = Scripted::new(&[
        r#"{"work": true, "title": "Site redesign", "space": "wenlan", "about": "a", "status": "s"}"#,
        r#"{"work": true, "title": "Site redesign", "space": "wenlan", "about": "b", "status": "s"}"#,
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "Site redesign", "conversations": ["C0", "C1"], "now": "Tagline done"}]}"#,
        r#"{"points": [
            {"text": "The site tagline is \"You do the thinking\".", "settled": true, "from": "C1", "replaces": null},
            {"text": "Whether the site gets a pricing page is still open.", "settled": false, "from": "C1", "replaces": "P7"},
            {"text": "", "settled": true, "from": "C0"},
            {"text": "A point from nowhere.", "settled": true, "from": "C9"}
        ]}"#,
        r#"{"points": [
            {"text": "The site gets no pricing page for now.", "settled": true, "from": "C0", "replaces": "P1"}
        ]}"#,
    ]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };
    let keep = Keep(&db, Default::default());
    let mut backoff = Backoff::default();
    look_with(&db, found.clone(), &models, "English", &mut backoff)
        .await
        .unwrap();
    let thing = db.list_things(&ReadScope::Global, None).await.unwrap()[0].clone();

    // A night: both conversations went quiet over half an hour ago.
    let now = chrono::Utc::now().timestamp();
    let due = db
        .things_to_dream(dream_quiet_since(now, 3), 10)
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
    let by_session: HashMap<String, FoundConversation> = found
        .iter()
        .map(|c| (found_session_id(c).unwrap(), c.clone()))
        .collect();
    let done = dream_with(&db, &due, &by_session, group.as_ref(), "English", &keep)
        .await
        .unwrap();
    assert_eq!(done, (1, 2));
    // The model read what the person typed and the end of the last reply.
    let asked = group.prompts.lock().unwrap().last().unwrap().clone();
    assert!(asked.contains("Tagline: go with You do the thinking"));
    assert!(asked.contains("the tagline is in place"));
    assert!(asked.contains("\"Site redesign\""));

    let detail = db
        .get_thing_detail(&thing.id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    let dream = detail.dream.unwrap();
    assert_eq!((dream.decided, dream.open), (1, 1));
    assert_eq!(
        detail.memories.len(),
        2,
        "the points are on the thing's page"
    );
    let after = db
        .list_things(&ReadScope::Global, Some(&thing.id))
        .await
        .unwrap()[0]
        .clone();
    assert_eq!(after.last_activity_at, thing.last_activity_at);

    // Nothing new: the next look reads nothing.
    let due = db.things_to_dream(now, 10).await.unwrap();
    assert!(due.is_empty());

    // Conversation a goes on; the next night sees the kept points and
    // replaces the open one.
    let mut a2 = a.clone();
    a2.last_at = now - 45 * 60;
    a2.said.push("no pricing page for now".into());
    let label2 = Scripted::new(&[
        r#"{"work": true, "title": "Site redesign", "space": "wenlan", "about": "a", "status": "s"}"#,
    ]);
    let models2 = IntakeModels {
        label: label2,
        group: Scripted::new(&[]),
    };
    look_with(&db, vec![a2.clone()], &models2, "English", &mut backoff)
        .await
        .ok();
    let due = db
        .things_to_dream(dream_quiet_since(now, 3), 10)
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].conversations.len(), 1);
    let by_session: HashMap<String, FoundConversation> =
        [(found_session_id(&a2).unwrap(), a2.clone())].into();
    let done = dream_with(&db, &due, &by_session, group.as_ref(), "English", &keep)
        .await
        .unwrap();
    assert_eq!(done, (1, 1));
    let asked = group.prompts.lock().unwrap().last().unwrap().clone();
    assert!(asked.contains("P1 (open): Whether the site gets a pricing page is still open."));
    let kept: Vec<String> = db
        .dream_points(&thing.id)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.text)
        .collect();
    assert_eq!(
        kept,
        [
            "The site tagline is \"You do the thinking\".",
            "The site gets no pricing page for now."
        ]
    );
}
