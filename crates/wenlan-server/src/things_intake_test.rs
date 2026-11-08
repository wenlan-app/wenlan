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

/// Answers once it is let go, so a test can act while a call is waiting.
struct Held {
    answer: String,
    release: tokio::sync::Notify,
}

impl Held {
    fn new(answer: &str) -> Arc<Self> {
        Arc::new(Self {
            answer: answer.into(),
            release: tokio::sync::Notify::new(),
        })
    }
}

#[async_trait]
impl LlmProvider for Held {
    async fn generate(&self, _request: LlmRequest) -> Result<String, LlmError> {
        self.release.notified().await;
        Ok(self.answer.clone())
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "held"
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
        touched: Vec::new(),
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
        r#"{"changes": []}"#,
        r#"{"conclusion": [
            {"text": "The tagline is settled; a pricing page is still open.", "points": ["P0", "P1"]},
            {"text": "Nothing backs this.", "points": []}
        ]}"#,
        r#"{"points": [
            {"text": "The site gets no pricing page for now.", "settled": true, "from": "C0", "replaces": "P1"}
        ]}"#,
        r#"{"changes": []}"#,
        "not json",
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
    let asked = group.prompts.lock().unwrap()[1].clone();
    assert!(asked.contains("Tagline: go with You do the thinking"));
    assert!(asked.contains("the tagline is in place"));
    assert!(asked.contains("\"Site redesign\""));

    // The points are the thing's map too: under a top point named after it,
    // each naming its memory (UX-CHAT-5).
    let map = db.get_thing_map(&thing.id).await.unwrap();
    assert_eq!(map.nodes.len(), 3, "{:?}", map.nodes);
    let root = map.nodes.iter().find(|n| n.parent_id.is_none()).unwrap();
    assert_eq!(root.label, "Site redesign");
    let pricing_node = map
        .nodes
        .iter()
        .find(|n| n.label.starts_with("Whether the site gets a pricing page"))
        .unwrap()
        .clone();
    assert_eq!(pricing_node.state, Some(wenlan_types::ThingMapState::Open));
    assert!(pricing_node.sources[0].memory_id.is_some());

    let detail = db
        .get_thing_detail(&thing.id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    let dream = detail.dream.unwrap();
    assert_eq!((dream.decided, dream.open), (1, 1));
    // UX-VIEW-6: then the page's first paragraph, from every standing point;
    // the sentence nothing backs is dropped.
    // UX-GRAPH-7: before it, the night asks whether alike memories an AI
    // wrote need tidying; this one needs nothing.
    let tidy = group.prompts.lock().unwrap()[2].clone();
    assert!(tidy.contains("Group 1:"), "{tidy}");
    let concluded = group.prompts.lock().unwrap()[3].clone();
    assert!(concluded.contains("P1 (open): Whether the site gets a pricing page is still open."));
    assert_eq!(dream.conclusion.len(), 1);
    assert_eq!(
        dream.conclusion[0].text,
        "The tagline is settled; a pricing page is still open."
    );
    assert_eq!(dream.conclusion[0].points.len(), 2);
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
    let asked = group.prompts.lock().unwrap()[4].clone();
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
    // The replacement is the same point on the map, now settled.
    let map = db.get_thing_map(&thing.id).await.unwrap();
    assert_eq!(map.nodes.len(), 3);
    let same = map.nodes.iter().find(|n| n.id == pricing_node.id).unwrap();
    assert_eq!(same.label, "The site gets no pricing page for now.");
    assert_eq!(same.state, Some(wenlan_types::ThingMapState::Decided));
    assert_ne!(same.sources[0].memory_id, pricing_node.sources[0].memory_id);
    // UX-VIEW-6: this night's conclusion failed. The night keeps its point,
    // and the page does not show last night's conclusion above newer points.
    let dream = db.thing_dream(&thing.id).await.unwrap().unwrap();
    assert_eq!(dream.decided, 1);
    assert!(dream.conclusion.is_empty());

    // Taking the night back puts the point back as it was.
    db.undo_thing_dream(&thing.id).await.unwrap();
    let map = db.get_thing_map(&thing.id).await.unwrap();
    let back = map.nodes.iter().find(|n| n.id == pricing_node.id).unwrap();
    assert_eq!(back, &pricing_node);
}

/// A thing with one conversation read for nothing yet, in the wenlan Space.
async fn thing_for_links(
    db: &MemoryDB,
    said: &[&str],
) -> (String, HashMap<String, FoundConversation>) {
    db.create_space("wenlan", None, false).await.unwrap();
    let found = vec![conv("a", said, 90)];
    let label = Scripted::new(&[
        r#"{"work": true, "title": "Site redesign", "space": "wenlan", "about": "a", "status": "Going"}"#,
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "Site redesign", "conversations": ["C0"], "now": "Going"}]}"#,
    ]);
    let models = IntakeModels { label, group };
    look_with(
        db,
        found.clone(),
        &models,
        "English",
        &mut Backoff::default(),
    )
    .await
    .unwrap();
    let thing = db.list_things(&ReadScope::Global, None).await.unwrap()[0]
        .id
        .clone();
    let by_session = found
        .into_iter()
        .map(|c| (found_session_id(&c).unwrap(), c))
        .collect();
    (thing, by_session)
}

async fn unlinked(db: &MemoryDB, thing: &str) -> Vec<String> {
    db.unlinked_night_points(thing)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect()
}

/// UX-CHAT-5: the same one call that writes a night's points names each one's
/// topic. It connects there and then; a point with no topic stays under its
/// thing, and an id that names nothing never loses the point. Taking the night
/// back removes the links.
#[tokio::test]
async fn ux_chat_5_a_night_connects_its_points_in_the_one_call() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (thing, found) = thing_for_links(
        &db,
        &["keep the GitHub repository private", "which plan first?"],
    )
    .await;
    db.store_entity("GitHub", "tool", Some("wenlan"), None, Some(1.0))
        .await
        .unwrap();
    let group = Scripted::new(&[
        r#"{"points": [
        {"text": "The repository stays private.", "settled": true, "from": "C0", "topic": "T0"},
        {"text": "Which plan comes first is open.", "settled": false, "from": "C0", "topic": null},
        {"text": "Named a topic that is not there.", "settled": true, "from": "C0", "topic": "T9"}
    ]}"#,
        r#"{"changes": []}"#,
    ]);
    let keep = Keep(&db, Default::default());
    let due = db
        .things_to_dream(chrono::Utc::now().timestamp(), 10)
        .await
        .unwrap();
    let done = dream_with(&db, &due, &found, group.as_ref(), "English", &keep)
        .await
        .unwrap();
    assert_eq!(done, (1, 3));
    // One call for points and topics; then the tidy, and the conclusion.
    assert_eq!(group.prompts.lock().unwrap().len(), 3, "one call");
    let asked = group.prompts.lock().unwrap()[0].clone();
    assert!(asked.contains("T0: GitHub"), "{asked}");

    assert_eq!(db.dream_points(&thing).await.unwrap().len(), 3);
    assert_eq!(unlinked(&db, &thing).await, ["mem_2", "mem_1"]);

    // Taking the night back removes its links and forgets its points in
    // the same step (F14): nothing of the night is left on the thing.
    let forget = db.undo_thing_dream(&thing).await.unwrap();
    assert_eq!(forget.len(), 3);
    assert!(db.dream_points(&thing).await.unwrap().is_empty());
    for id in &forget {
        assert!(db.get_memory_detail(id).await.unwrap().is_none(), "{id}");
    }
}

/// UX-GRAPH-7: a night merges two memories an AI wrote that say the same.
/// Both are hidden, not deleted; the merged one stands on the thing and the
/// morning line counts it; taking the night back takes all of it back.
#[tokio::test]
async fn ux_graph_7_a_night_merges_what_says_the_same_and_undo_takes_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (thing, found) = thing_for_links(
        &db,
        &["keep the GitHub repository private", "yes, private for now"],
    )
    .await;
    let group = Scripted::new(&[
        r#"{"points": [
        {"text": "The repository stays private.", "settled": true, "from": "C0"},
        {"text": "The repository is kept private.", "settled": true, "from": "C0"}
    ]}"#,
        r#"{"changes": [
        {"do": "merge", "memories": ["M0", "M1"], "text": "The repository stays private."}
    ]}"#,
    ]);
    let keep = Keep(&db, Default::default());
    let due = db
        .things_to_dream(chrono::Utc::now().timestamp(), 10)
        .await
        .unwrap();
    dream_with(&db, &due, &found, group.as_ref(), "English", &keep)
        .await
        .unwrap();
    let tidy = group.prompts.lock().unwrap()[1].clone();
    assert!(tidy.contains("The repository is kept private."), "{tidy}");

    // The two are hidden and kept; the merged one is the thing's point.
    let points: Vec<String> = db
        .dream_points(&thing)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(points, ["mem_2"]);
    for id in ["mem_0", "mem_1"] {
        let hidden = db.get_memory_detail(id).await.unwrap().unwrap();
        assert_eq!(hidden.supersede_mode, "evicted", "{id}");
    }
    let merged = db.get_memory_detail("mem_2").await.unwrap().unwrap();
    assert_eq!(merged.content, "The repository stays private.");
    assert_eq!(merged.memory_type.as_deref(), Some("decision"));
    assert!(
        merged.session_id.is_some(),
        "it keeps the conversation it came from"
    );
    let nights = db.thing_night_points(&ReadScope::Global).await.unwrap();
    let last = nights[0].last_night.clone().unwrap();
    assert!(
        last.written.is_empty(),
        "the merge is counted once, as a merge"
    );
    assert_eq!(
        last.tidied,
        [wenlan_types::NightTidied {
            kind: NightTidyKind::Merge,
            memory_id: "mem_2".into(),
            replaced: vec!["mem_0".into(), "mem_1".into()],
        }]
    );

    // Undo: this night wrote all three, so all three are forgotten.
    let forget = db.undo_thing_dream(&thing).await.unwrap();
    assert_eq!(forget, ["mem_0", "mem_1", "mem_2"]);
    assert!(db.get_memory_detail("mem_2").await.unwrap().is_none());
}

/// UX-CHAT-5: old points are connected through the same path, once. A model
/// failure leaves it able to run again; a connected point is not asked about
/// twice, and undo takes it all back.
#[tokio::test]
async fn ux_chat_5_old_points_are_connected_once_and_can_be_taken_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (thing, found) = thing_for_links(&db, &["x", "y"]).await;
    // A night with no topic to offer.
    let night = Scripted::new(&[r#"{"points": [
        {"text": "Keep the GitHub repository private.", "settled": true, "from": "C0"},
        {"text": "Which plan comes first?", "settled": false, "from": "C0"}
    ]}"#]);
    let keep = Keep(&db, Default::default());
    let due = db
        .things_to_dream(chrono::Utc::now().timestamp(), 10)
        .await
        .unwrap();
    dream_with(&db, &due, &found, night.as_ref(), "English", &keep)
        .await
        .unwrap();
    assert!(!night.prompts.lock().unwrap()[0].contains("big map"));
    db.store_entity("GitHub", "tool", Some("wenlan"), None, Some(1.0))
        .await
        .unwrap();

    // The model fails: nothing is linked and it can run again. While it
    // runs, a second start is refused instead of asking the model again.
    let failing = Held::new("not json");
    let second = Scripted::new(&[]);
    let (report, refused) = tokio::join!(connect_old_night_points(&db, failing.as_ref()), async {
        let refused = connect_old_night_points(&db, second.as_ref()).await;
        failing.release.notify_one();
        refused
    });
    assert!(refused.is_err());
    assert_eq!(second.prompts.lock().unwrap().len(), 0);
    let report = report.unwrap();
    assert_eq!((report.points, report.linked, report.failed), (2, 0, 1));
    assert!(db.night_links_backfilled().await.unwrap().is_none());

    let model = Scripted::new(&[r#"{"points": [
        {"point": "P0", "topic": "T0"},
        {"point": "P1", "topic": null}
    ]}"#]);
    let report = connect_old_night_points(&db, model.as_ref()).await.unwrap();
    assert_eq!(
        (report.things, report.points, report.linked, report.failed),
        (1, 2, 1, 0)
    );
    assert_eq!(unlinked(&db, &thing).await, ["mem_1"]);
    assert!(model.prompts.lock().unwrap()[0].contains("T0: GitHub"));

    // Once only: asks nobody.
    let again = Scripted::new(&[]);
    let report = connect_old_night_points(&db, again.as_ref()).await.unwrap();
    assert!(report.already_done);
    assert_eq!(again.prompts.lock().unwrap().len(), 0);

    // The thing's last night is still the night.
    let dream = db.thing_dream(&thing).await.unwrap().unwrap();
    assert_eq!((dream.decided, dream.open), (1, 1));

    assert_eq!(db.undo_night_links_backfill().await.unwrap(), 1);
    assert_eq!(unlinked(&db, &thing).await, ["mem_0", "mem_1"]);
}

/// Trust audit F6, F7: a night reads the conversations it can find and keeps
/// the line each point came from; one whose transcript is gone is not
/// marked read, and the thing's page can say so.
#[tokio::test]
async fn ux_chat_5_a_night_keeps_its_lines_and_says_what_it_could_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.create_space("wenlan", None, false).await.unwrap();
    let a = conv(
        "a",
        &["resume first pass", "keep the resume to one page"],
        90,
    );
    let gone = conv("gone", &["make the header centred", "yes"], 80);
    let label = Scripted::new(&[
        r#"{"work": true, "title": "Tailor resume", "space": "wenlan", "about": "a", "status": "s"}"#,
        r#"{"work": true, "title": "Tailor resume", "space": "wenlan", "about": "b", "status": "s"}"#,
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "Tailor resume", "conversations": ["C0", "C1"], "now": "Going"}]}"#,
        r#"{"points": [
            {"text": "The resume stays one page.", "settled": true, "from": "C0", "line": "L1"},
            {"text": "Merged in PR #43.", "settled": true, "from": "C0", "line": "L0"}
        ]}"#,
        r#"{"conclusion": []}"#,
    ]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };
    let mut backoff = Backoff::default();
    look_with(
        &db,
        vec![a.clone(), gone.clone()],
        &models,
        "English",
        &mut backoff,
    )
    .await
    .unwrap();
    let thing = db.list_things(&ReadScope::Global, None).await.unwrap()[0].clone();
    let now = chrono::Utc::now().timestamp();
    let due = db
        .things_to_dream(dream_quiet_since(now, 3), 10)
        .await
        .unwrap();
    assert_eq!(due[0].conversations.len(), 2);
    // Only a's transcript is still there.
    let by_session: HashMap<String, FoundConversation> =
        [(found_session_id(&a).unwrap(), a.clone())].into();
    let keep = Keep(&db, Default::default());
    let done = dream_with(&db, &due, &by_session, group.as_ref(), "English", &keep)
        .await
        .unwrap();
    assert_eq!(done, (1, 1), "the point with a number in it is dropped");
    let asked = group.prompts.lock().unwrap()[1].clone();
    assert!(asked.contains("L1: keep the resume to one page"), "{asked}");

    let anchor = db
        .memory_said_anchor("mem_0", &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(anchor.line.as_deref(), Some("keep the resume to one page"));
    // The gone conversation waits, and the page says it could not be read.
    assert!(db
        .things_to_dream(dream_quiet_since(now, 3), 10)
        .await
        .unwrap()
        .is_empty());
    let detail = db
        .get_thing_detail(&thing.id, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.unread, 1);
}
