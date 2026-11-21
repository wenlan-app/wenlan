// SPDX-License-Identifier: Apache-2.0

use super::*;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Mutex;
use wenlan_core::events::NoopEmitter;
use wenlan_core::llm_provider::{LlmBackend, LlmError};
use wenlan_core::read_scope::ReadScope;
use wenlan_types::authorship::SentenceAuthor;

/// Answers in turn from a list, and keeps the prompts it was given.
/// Answers in order. A night's tidy call (UX-GRAPH-7) depends on how alike
/// the embedder finds memories, which differs between a machine with the
/// model and one without, so tidy calls are kept apart: they take the first
/// scripted `{"changes": ...}` answer, or nothing to tidy, and their prompts
/// go to `tidy_prompts`.
struct Scripted {
    answers: Mutex<Vec<String>>,
    prompts: Mutex<Vec<String>>,
    tidy_prompts: Mutex<Vec<String>>,
    /// The night's check of a note it only read (`things_read_page`): one
    /// scripted answer per note, in order, kept apart from the points' answers.
    read_page_answers: Mutex<Vec<String>>,
    read_page_prompts: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(answers: &[&str]) -> Arc<Self> {
        Self::with_read_page_answers(answers, &[])
    }

    fn with_read_page_answers(answers: &[&str], read_page_answers: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.iter().map(|a| a.to_string()).collect()),
            prompts: Mutex::new(Vec::new()),
            tidy_prompts: Mutex::new(Vec::new()),
            read_page_answers: Mutex::new(
                read_page_answers.iter().map(|a| a.to_string()).collect(),
            ),
            read_page_prompts: Mutex::new(Vec::new()),
        })
    }
}

fn is_tidy_answer(answer: &str) -> bool {
    answer.trim_start().starts_with(r#"{"changes""#)
}

#[async_trait]
impl LlmProvider for Scripted {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        if request.label.as_deref() == Some("things_read_page") {
            self.read_page_prompts
                .lock()
                .unwrap()
                .push(request.user_prompt);
            let mut answers = self.read_page_answers.lock().unwrap();
            return Ok(if answers.is_empty() {
                r#"{"contradicted": []}"#.to_string()
            } else {
                answers.remove(0)
            });
        }
        let tidy = matches!(
            request.label.as_deref(),
            Some("things_tidy" | "things_old_pass")
        );
        let mut answers = self.answers.lock().unwrap();
        let at = answers.iter().position(|a| is_tidy_answer(a) == tidy);
        if tidy {
            self.tidy_prompts.lock().unwrap().push(request.user_prompt);
            return Ok(at
                .map(|i| answers.remove(i))
                .unwrap_or_else(|| r#"{"changes": []}"#.to_string()));
        }
        self.prompts.lock().unwrap().push(request.user_prompt);
        at.map(|i| answers.remove(i)).ok_or(LlmError::NotAvailable)
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
        last_content: "done".into(),
        first_at: now - minutes_ago * 60 - 600,
        last_content_at: now - minutes_ago * 60,
        last_at: now - minutes_ago * 60,
        automated: false,
        on_behalf: false,
        touched: Vec::new(),
    }
}

#[test]
fn idle_fill_shared_space_list_includes_folder_shared_spaces() {
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("data/shared-spaces.json");
    let folder = temp.path().join("shared-folder");
    let scope = wenlan_core::shared_space::Scope::Space("folder-shared".into());
    wenlan_core::shared_space::join_at(&settings, &scope, &folder, "Tester").unwrap();

    let shared = super::shared_space_names(&settings).unwrap();
    assert!(shared.contains("folder-shared"));
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

/// A conversation whose AI kept a diagram is work, even when the labelling
/// model calls it a test, so the diagram lands under a thing (UX-ART-4).
#[tokio::test]
async fn ux_art_4_a_conversation_that_kept_a_diagram_is_work() {
    use wenlan_core::chat_import::originals::{
        keep_session_original, SessionOriginal, SessionOriginalFormat,
    };
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let session = wenlan_types::sessions::SessionRef::new("claude-code", "kept").unwrap();
    keep_session_original(
        &db,
        dir.path(),
        None,
        SessionOriginal {
            session: Some(&session),
            title: "Release flow",
            content: "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>keep</text></svg>",
            format: SessionOriginalFormat::Svg,
            space: wenlan_types::WriteSpaceTarget::Uncategorized,
            workspace: None,
            source_memory_ids: Vec::new(),
            replaces: None,
        },
    )
    .await
    .unwrap();
    let found = vec![
        conv(
            "kept",
            &["Test keeping a diagram", "now the new version"],
            20,
        ),
        conv("plain", &["Test the tool", "thanks"], 10),
    ];
    // The model calls both a test of the tool.
    let label = Scripted::new(&[
        r#"{"work": false, "title": "Release flow diagram", "space": "", "about": "a diagram of the release", "status": "Kept two versions"}"#,
        r#"{"work": false, "title": "Tool test", "space": "", "about": "a test", "status": "Done"}"#,
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "Release flow", "conversations": ["C0"], "now": "Two versions of the diagram kept"}]}"#,
    ]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };
    let mut backoff = Backoff::default();
    let report = look_with(&db, found, &models, "English", &mut backoff)
        .await
        .unwrap();
    assert_eq!((report.labelled, report.work, report.grouped), (2, 1, 1));
    let grouped = group.prompts.lock().unwrap().join("\n");
    assert!(grouped.contains("Release flow diagram"));
    assert!(!grouped.contains("Tool test"));
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 1);
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
        r#"{"conclusion": [
            {"text": "The tagline is settled; a pricing page is still open.", "points": ["P0", "P1"]},
            {"text": "Nothing backs this.", "points": []}
        ]}"#,
        r#"{"points": [
            {"text": "The site gets no pricing page for now.", "settled": true, "from": "C0", "replaces": "P1"}
        ]}"#,
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
    let done = dream_with(
        &db,
        &due,
        &by_session,
        group.as_ref(),
        "English",
        &keep,
        None,
    )
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
    let concluded = group.prompts.lock().unwrap()[2].clone();
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
    let done = dream_with(
        &db,
        &due,
        &by_session,
        group.as_ref(),
        "English",
        &keep,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done, (1, 1));
    let asked = group.prompts.lock().unwrap()[3].clone();
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
    let group = Scripted::new(&[r#"{"points": [
        {"text": "The repository stays private.", "settled": true, "from": "C0", "topic": "T0"},
        {"text": "Which plan comes first is open.", "settled": false, "from": "C0", "topic": null},
        {"text": "Named a topic that is not there.", "settled": true, "from": "C0", "topic": "T9"}
    ]}"#]);
    let keep = Keep(&db, Default::default());
    let due = db
        .things_to_dream(chrono::Utc::now().timestamp(), 10)
        .await
        .unwrap();
    let done = dream_with(&db, &due, &found, group.as_ref(), "English", &keep, None)
        .await
        .unwrap();
    assert_eq!(done, (1, 3));
    // One call for points and topics; the second writes the conclusion.
    assert_eq!(group.prompts.lock().unwrap().len(), 2, "one call");
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
/// morning line counts it; the page can take back just the merge, and the
/// night's Undo still takes back the rest.
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
        {"text": "The repository stays private for now.", "settled": true, "from": "C0"}
    ]}"#,
        r#"{"changes": [
        {"do": "merge", "memories": ["M1", "M0"], "text": "The repository stays private."}
    ]}"#,
    ]);
    // The model names them in reverse: the page and the night's line both
    // still list them in one order.
    let keep = Keep(&db, Default::default());
    let due = db
        .things_to_dream(chrono::Utc::now().timestamp(), 10)
        .await
        .unwrap();
    dream_with(&db, &due, &found, group.as_ref(), "English", &keep, None)
        .await
        .unwrap();
    let tidy = group.tidy_prompts.lock().unwrap()[0].clone();
    assert!(
        tidy.contains("The repository stays private for now."),
        "{tidy}"
    );

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
            // Replay shows the words that went (UX-GRAPH-7).
            said: vec![
                "The repository stays private.".into(),
                "The repository stays private for now.".into(),
            ],
        }]
    );

    // The thing's own map holds one point for the two.
    let points_on_map = |map: &wenlan_types::ThingMapResponse| -> Vec<String> {
        map.nodes
            .iter()
            .filter(|n| n.parent_id.is_some())
            .map(|n| n.label.clone())
            .collect()
    };
    let map = db.get_thing_map(&thing).await.unwrap();
    assert_eq!(points_on_map(&map), ["The repository stays private."]);
    // The page can say what it took the place of, and take back just that.
    let detail = db
        .get_thing_detail(&thing, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.tidied.len(), 1);
    assert_eq!(detail.tidied[0].memory_id, "mem_2");
    let texts: Vec<&str> = detail.tidied[0]
        .replaced
        .iter()
        .map(|r| r.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "The repository stays private.",
            "The repository stays private for now."
        ]
    );
    let listed: Vec<&str> = detail
        .memories
        .iter()
        .map(|m| m.source_id.as_str())
        .collect();
    assert_eq!(listed, ["mem_2"], "the hidden two are not listed");

    assert_eq!(
        db.undo_tidy(&thing, "mem_2").await.unwrap().unwrap(),
        ["mem_2"]
    );
    assert!(db.get_memory_detail("mem_2").await.unwrap().is_none());
    for id in ["mem_0", "mem_1"] {
        let back = db.get_memory_detail(id).await.unwrap().unwrap();
        assert_ne!(back.supersede_mode, "evicted", "{id}");
    }
    let mut back = points_on_map(&db.get_thing_map(&thing).await.unwrap());
    back.sort();
    assert_eq!(
        back,
        [
            "The repository stays private for now.",
            "The repository stays private."
        ]
    );
    assert!(db
        .get_thing_detail(&thing, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap()
        .tidied
        .is_empty());
    assert!(db.undo_tidy(&thing, "mem_2").await.unwrap().is_none());

    // Undo of the night then forgets the two points it wrote.
    let forget = db.undo_thing_dream(&thing).await.unwrap();
    assert_eq!(forget, ["mem_0", "mem_1"]);
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
    dream_with(&db, &due, &found, night.as_ref(), "English", &keep, None)
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
    let done = dream_with(
        &db,
        &due,
        &by_session,
        group.as_ref(),
        "English",
        &keep,
        None,
    )
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
    assert_eq!(detail.unread_tools, ["claude-code"]);
    assert_eq!(detail.waiting, 0, "a gone transcript is not waiting");
}

async fn waiting_unread(db: &MemoryDB, thing: &str) -> (usize, usize) {
    let d = db
        .get_thing_detail(thing, &ReadScope::Global)
        .await
        .unwrap()
        .unwrap();
    (d.waiting, d.unread)
}

/// Design v5 F7: a night with no model leaves its due conversations waiting
/// and the thing's page says so; a later night that has a model reads them
/// and the line goes. A gone transcript is "didn't get", never "waiting".
#[tokio::test]
async fn ux_chat_5_a_night_with_no_model_leaves_conversations_waiting_for_a_later_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "wenlan", "wenlan").await;
    let thing = db.list_things(&ReadScope::Global, None).await.unwrap()[0].clone();
    let now = chrono::Utc::now().timestamp();
    assert_eq!(waiting_unread(&db, &thing.id).await, (0, 0));

    note_waiting_for_model(&db, now, 3).await;
    assert_eq!(
        waiting_unread(&db, &thing.id).await,
        (2, 0),
        "no model ran: both are waiting"
    );

    let group = Scripted::new(&[
        r#"{"points": [
            {"text": "The release ships on Tuesday.", "settled": true, "from": "C0", "line": "L0"}
        ]}"#,
        r#"{"conclusion": []}"#,
    ]);
    // Only one transcript is still there on the later night.
    let a = found.values().next().unwrap().clone();
    let only: HashMap<String, FoundConversation> = [(found_session_id(&a).unwrap(), a)].into();
    let keep = Keep(&db, Default::default());
    dream_with(&db, &due, &only, group.as_ref(), "English", &keep, None)
        .await
        .unwrap();
    assert_eq!(
        waiting_unread(&db, &thing.id).await,
        (0, 1),
        "one was read, one is gone: nothing waits any more"
    );
}

/// The dry run of UX-GRAPH-7 on a copy of a real database: what a week of
/// nights would have tidied, asked of the same model a night uses, written to
/// a report. Nothing is applied. Run on a copy only:
/// `WENLAN_TIDY_DRY_DIR=<copy's data dir> WENLAN_TIDY_DRY_REPORT=<file>
/// cargo test -p wenlan-server --lib -- --ignored tidy_dry_run`
#[tokio::test]
#[ignore = "needs a database copy and the person's AI tool"]
async fn tidy_dry_run() {
    let dir = std::env::var("WENLAN_TIDY_DRY_DIR").expect("WENLAN_TIDY_DRY_DIR");
    let report = std::env::var("WENLAN_TIDY_DRY_REPORT").expect("WENLAN_TIDY_DRY_REPORT");
    let days: i64 = std::env::var("WENLAN_TIDY_DRY_DAYS")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(7);
    let db = MemoryDB::new(std::path::Path::new(&dir), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let model = wenlan_core::agent_cli_llm::AgentCliProvider::find().expect("an AI tool CLI");
    let now = chrono::Utc::now().timestamp();
    let since = now - days * 86_400;
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    let mut out = format!(
        "# Night tidy dry run\n\nLooked back {days} days over {} things. Nothing was applied.\n\n",
        things.len()
    );
    let (mut offered, mut asked, mut counts) = (0usize, 0usize, [0usize; 3]);
    let mut samples = String::new();
    for thing in &things {
        let title = thing.title.clone().unwrap_or_else(|| "(untitled)".into());
        let offer = db
            .tidy_offer_since(&thing.id, Some(since), now)
            .await
            .unwrap();
        offered += offer.items.len();
        if offer.groups.is_empty() {
            continue;
        }
        asked += 1;
        let started = std::time::Instant::now();
        let answer = model
            .generate(LlmRequest {
                system_prompt: Some(
                    "You tidy the memories an AI keeps for a person, changing only what you are sure of. Answer with JSON only."
                        .into(),
                ),
                user_prompt: night_tidy::tidy_prompt(&offer.items, &offer.groups, now),
                max_tokens: 1_500,
                temperature: 0.0,
                label: Some("things_tidy_dry_run".into()),
                timeout_secs: Some(300),
            })
            .await;
        let secs = started.elapsed().as_secs();
        let changes = match answer {
            Ok(answer) => night_tidy::parse_tidy(&answer, &offer.items, now).unwrap_or_default(),
            Err(error) => {
                samples.push_str(&format!("- {}: the call failed ({error})\n", title));
                continue;
            }
        };
        samples.push_str(&format!(
            "\n## {} ({} groups, {} memories, {secs}s)\n",
            title,
            offer.groups.len(),
            offer.items.len()
        ));
        if changes.is_empty() {
            samples.push_str("Nothing to tidy.\n");
        }
        for change in changes {
            let text = |i: usize| offer.items[i].text.replace('\n', " ");
            match change {
                night_tidy::TidyChange::Merge {
                    items: [a, b],
                    text: merged,
                } => {
                    counts[0] += 1;
                    samples.push_str(&format!(
                        "- Merge\n  - was: {}\n  - was: {}\n  - now: {merged}\n",
                        text(a),
                        text(b)
                    ));
                }
                night_tidy::TidyChange::Replace { keep, drop } => {
                    counts[1] += 1;
                    samples.push_str(&format!(
                        "- Replace\n  - hidden: {}\n  - kept: {}\n",
                        text(drop),
                        text(keep)
                    ));
                }
                night_tidy::TidyChange::Update {
                    item,
                    text: updated,
                } => {
                    counts[2] += 1;
                    samples.push_str(&format!(
                        "- Update\n  - was: {}\n  - now: {updated}\n",
                        text(item)
                    ));
                }
            }
        }
    }
    out.push_str(&format!(
        "AI-written memories looked at: {offered}. Things asked: {asked}.\n\n\
Would merge {}, replace {}, update {}.\n",
        counts[0], counts[1], counts[2]
    ));
    out.push_str(&samples);
    std::fs::write(&report, out).unwrap();
}

/// UX-GRAPH-7: the old-memory pass hides an exact copy behind the earlier
/// without asking, says an AI's memory about a passed date again in the
/// past tense through the same path as a point, runs once, and Undo takes
/// all of it back.
#[tokio::test]
async fn ux_graph_7_the_old_memory_pass_hides_copies_and_says_passed_dates_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let day = now - now.rem_euclid(86_400) - 3 * 86_400;
    for (id, text, space, at) in [
        (
            "mem_x",
            "The site stays free for now.",
            "wenlan",
            now - 9_000,
        ),
        (
            "mem_y",
            "The site stays free, for now",
            "wenlan",
            now - 5_000,
        ),
        (
            "mem_z",
            "The demo is planned for that day.",
            "personal",
            // Written well over a day before the date it is about at any hour:
            // `day` is midnight three days back, so it can be as late as
            // now - 3 days.
            day - 2 * 86_400,
        ),
    ] {
        db.upsert_documents(vec![wenlan_core::sources::RawDocument {
            source: "memory".into(),
            source_id: id.into(),
            title: id.into(),
            content: text.into(),
            last_modified: at,
            memory_type: Some("fact".into()),
            space: Some(space.into()),
            source_agent: Some("claude-code".into()),
            ..Default::default()
        }])
        .await
        .unwrap();
    }
    db.set_event_dates_by_source_id(&[("mem_z".into(), day)])
        .await
        .unwrap();
    let date = chrono::DateTime::from_timestamp(day, 0)
        .unwrap()
        .format("%b %-d, %Y")
        .to_string();
    let answer = format!(
        r#"{{"changes": [{{"do": "update", "memory": "M0", "text": "The demo was planned for {date}."}}]}}"#
    );
    let model = Scripted::new(&[answer.as_str()]);
    let keep = Keep(&db, Default::default());
    let pass = run_old_memory_pass(&db, model.as_ref(), &keep)
        .await
        .unwrap();
    assert_eq!((pass.merged(), pass.updated()), (1, 1), "{pass:?}");
    let shown = |id: &'static str| {
        let db = &db;
        async move { db.get_memory_detail(id).await.unwrap().is_some() }
    };
    let written = db.get_memory_detail("mem_0").await.unwrap().unwrap();
    assert_eq!(written.content, format!("The demo was planned for {date}."));
    assert!(
        run_old_memory_pass(&db, model.as_ref(), &keep)
            .await
            .is_err(),
        "it runs once"
    );
    assert!(shown("mem_x").await);
    db.undo_backlog().await.unwrap().unwrap();
    assert!(db.get_memory_detail("mem_0").await.unwrap().is_none());
    assert!(db.backlog_pass().await.unwrap().is_none());
}

/// One thing with two conversations, `a` labelled into `space_a` and `b` into
/// `space_b`, both quiet for over half an hour: what the night tests below
/// read. Returns the transcripts by session and what is due tonight.
async fn one_thing_two_conversations(
    db: &MemoryDB,
    space_a: &str,
    space_b: &str,
) -> (HashMap<String, FoundConversation>, Vec<DreamDue>) {
    for space in [space_a, space_b] {
        if db.get_space(space).await.unwrap().is_none() {
            db.create_space(space, None, false).await.unwrap();
        }
    }
    let a = conv("a", &["ship the release on Tuesday", "yes"], 90);
    let b = conv("b", &["my dentist is on Friday", "ok"], 80);
    let label = Scripted::new(&[
        &format!(
            r#"{{"work": true, "title": "Release", "space": "{space_a}", "about": "a", "status": "s"}}"#
        ),
        &format!(
            r#"{{"work": true, "title": "Release", "space": "{space_b}", "about": "b", "status": "s"}}"#
        ),
    ]);
    let group = Scripted::new(&[
        r#"{"new": [{"title": "Release", "conversations": ["C0", "C1"], "now": "Going"}]}"#,
    ]);
    let models = IntakeModels { label, group };
    let mut backoff = Backoff::default();
    look_with(
        db,
        vec![a.clone(), b.clone()],
        &models,
        "English",
        &mut backoff,
    )
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp();
    let due = db
        .things_to_dream(dream_quiet_since(now, 3), 10)
        .await
        .unwrap();
    let found = [a, b]
        .into_iter()
        .map(|c| (found_session_id(&c).unwrap(), c))
        .collect();
    (found, due)
}

/// Design v5 D14: a night never puts one Space's conversation in the prompt
/// that writes another Space's points, so a personal conversation cannot leak
/// into a shared Space. A thing spanning two Spaces is read once per Space.
#[tokio::test]
async fn a_night_reads_each_spaces_conversations_apart() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "home").await;
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].conversations.len(), 2);
    let night = Scripted::new(&[
        r#"{"points": [{"text": "The dentist visit is on Friday.", "settled": true, "from": "C0"}]}"#,
        r#"{"conclusion": []}"#,
        r#"{"points": [{"text": "The release ships on Tuesday.", "settled": true, "from": "C0"}]}"#,
        r#"{"conclusion": []}"#,
    ]);
    let keep = Keep(&db, Default::default());
    dream_with(&db, &due, &found, night.as_ref(), "English", &keep, None)
        .await
        .unwrap();

    let prompts = night.prompts.lock().unwrap().clone();
    let reads: Vec<&String> = prompts.iter().filter(|p| p.contains("C0")).collect();
    assert!(reads.len() >= 2, "{prompts:?}");
    let home = reads.iter().find(|p| p.contains("dentist")).unwrap();
    assert!(!home.contains("release on Tuesday"), "{home}");
    let team = reads
        .iter()
        .find(|p| p.contains("release on Tuesday"))
        .unwrap();
    assert!(!team.contains("dentist"), "{team}");
    let space_of = |id: &str| {
        let db = &db;
        let id = id.to_string();
        async move { db.get_memory_detail(&id).await.unwrap().unwrap().space }
    };
    assert_eq!(space_of("mem_0").await.as_deref(), Some("home"));
    assert_eq!(space_of("mem_1").await.as_deref(), Some("team"));
}

/// Writes nothing the first time it is asked, then keeps points as `Keep`.
struct FailFirst<'a>(Keep<'a>, std::sync::atomic::AtomicBool);

impl PointWriter for FailFirst<'_> {
    fn write(&self, request: StoreMemoryRequest) -> Written<'_> {
        if !self.1.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async { Err("the store refused it".to_string()) });
        }
        self.0.write(request)
    }
}

/// A point the night could not keep does not mark its conversation read: the
/// next night reads it again instead of losing what it settled.
#[tokio::test]
async fn a_point_not_kept_leaves_its_conversation_for_the_next_night() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let night = Scripted::new(&[
        r#"{"points": [
            {"text": "The release ships on Tuesday.", "settled": true, "from": "C0"},
            {"text": "The dentist visit is on Friday.", "settled": true, "from": "C1"}
        ]}"#,
        r#"{"conclusion": []}"#,
    ]);
    let keep = FailFirst(Keep(&db, Default::default()), Default::default());
    dream_with(&db, &due, &found, night.as_ref(), "English", &keep, None)
        .await
        .unwrap();

    let now = chrono::Utc::now().timestamp();
    let again = db
        .things_to_dream(dream_quiet_since(now, 3), 10)
        .await
        .unwrap();
    assert_eq!(again.len(), 1, "the thing is due again");
    assert_eq!(again[0].conversations.len(), 1, "{again:?}");
    assert_eq!(
        again[0].conversations[0].session_id, due[0].conversations[0].session_id,
        "only the conversation (C0) whose point was not kept"
    );
}

// ---------------------------------------------------------------------------
// Design v5 F6 / D15: a note a conversation only read is a candidate; the
// night changes one of its sentences only when a new point clearly
// contradicts it.
// ---------------------------------------------------------------------------

/// A note in `space` whose sentences the AI wrote, cited by one old memory.
async fn ai_note(db: &MemoryDB, space: &str, title: &str, body: &str) -> String {
    let source = format!("src_{}", title.replace(' ', "_"));
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: source.clone(),
        title: source.clone(),
        content: body.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".into()),
        source_agent: Some("test".into()),
        confidence: Some(0.9),
        confirmed: Some(true),
        space: Some(space.to_string()),
        ..Default::default()
    }])
    .await
    .unwrap();
    // A match threshold nothing reaches: each note is its own page.
    let made = wenlan_core::post_write::create_page_with_tuning(
        db,
        wenlan_types::requests::CreateConceptRequest {
            title: title.to_string(),
            content: body.to_string(),
            summary: None,
            entity_id: None,
            space: Some(space.to_string()).into(),
            source_memory_ids: vec![source],
            creation_kind: Some("distilled".into()),
            workspace: Some(space.to_string()),
        },
        "test",
        None,
        1,
        2.0,
    )
    .await
    .unwrap();
    assert!(
        made.attached_to.is_none(),
        "the note was merged into another"
    );
    db.set_page_review_status(&made.id, "confirmed")
        .await
        .unwrap();
    made.id
}

/// The person rewrites the note's body in their own editor.
async fn person_edits(db: &MemoryDB, page_id: &str, body: &str) {
    let page = db.get_page(page_id).await.unwrap().unwrap();
    let result = wenlan_core::post_write::update_page(
        db,
        page_id,
        wenlan_types::requests::UpdatePageRequest {
            content: body.to_string(),
            source_memory_ids: page.source_memory_ids.clone(),
            expected_version: None,
            caller_id: None,
            operation_id: None,
        },
        "manual_edit",
        false,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(result.wrote);
}

/// Record that conversation `conversation` of `due` (C0 is 0) read `page_id` as `kind`.
async fn conversation_read(
    db: &MemoryDB,
    due: &[DreamDue],
    conversation: usize,
    kind: wenlan_types::SessionReadKind,
    page_id: &str,
) {
    let conversation = &due[0].conversations[conversation];
    let session = wenlan_types::SessionRef::parse(&format!(
        "{}:{}",
        conversation.tool, conversation.external_id
    ))
    .unwrap();
    assert!(db
        .record_session_read(&session, kind, page_id, None)
        .await
        .unwrap());
}

/// A night in which the conversation settles "The release ships on
/// Wednesday." (kept as `mem_0`) and the model answers each read note's check
/// from `read_answers`, in order. Returns the model, to see what it was asked.
async fn night_over_read_notes(
    db: &MemoryDB,
    found: &HashMap<String, FoundConversation>,
    due: &[DreamDue],
    read_answers: &[&str],
) -> Arc<Scripted> {
    let keep = Keep(db, Default::default());
    night_over_read_notes_in(db, found, due, read_answers, None, &keep).await
}

/// [`night_over_read_notes`] projecting pages into `pages_folder`.
async fn night_over_read_notes_in(
    db: &MemoryDB,
    found: &HashMap<String, FoundConversation>,
    due: &[DreamDue],
    read_answers: &[&str],
    pages_folder: Option<&std::path::Path>,
    writer: &dyn PointWriter,
) -> Arc<Scripted> {
    let night = Scripted::with_read_page_answers(
        &[
            r#"{"points": [{"text": "The release ships on Wednesday.", "settled": true, "from": "C0"}]}"#,
            r#"{"conclusion": []}"#,
        ],
        read_answers,
    );
    dream_with(
        db,
        due,
        found,
        night.as_ref(),
        "English",
        writer,
        pages_folder,
    )
    .await
    .unwrap();
    night
}

const SHIPS_ON_TUESDAY: &str =
    r#"{"contradicted": [{"sentence": 0, "point": 0, "text": "The release ships on Wednesday."}]}"#;

#[tokio::test]
async fn a_night_changes_the_ai_sentence_a_new_point_contradicts_on_a_note_a_recall_returned() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(
        &db,
        "team",
        "Release",
        "The release ships on Tuesday. The team reviews on Monday.",
    )
    .await;
    conversation_read(
        &db,
        &due,
        0,
        wenlan_types::SessionReadKind::RecallPage,
        &note,
    )
    .await;
    let before = db.get_page(&note).await.unwrap().unwrap();

    // The pages folder is a scratch one, never the person's own.
    let pages = tempfile::tempdir().unwrap();
    let keep = Keep(&db, Default::default());
    let night = night_over_read_notes_in(
        &db,
        &found,
        &due,
        &[SHIPS_ON_TUESDAY],
        Some(pages.path()),
        &keep,
    )
    .await;

    let asked = night.read_page_prompts.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].contains("The release ships on Wednesday."),
        "{}",
        asked[0]
    );
    let after = db.get_page(&note).await.unwrap().unwrap();
    // Only the contradicted sentence changed; the other is byte for byte.
    assert_eq!(
        after.content,
        "The release ships on Wednesday. The team reviews on Monday."
    );
    assert_eq!(after.version, before.version + 1);
    assert!(after.source_memory_ids.contains(&"mem_0".to_string()));
    // The page's file in the pages folder carries the new sentence.
    let projected = std::fs::read_dir(pages.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .any(|entry| {
            std::fs::read_to_string(entry.path())
                .is_ok_and(|text| text.contains("The release ships on Wednesday."))
        });
    assert!(projected, "no .md in the pages folder has the new sentence");
    let sentences = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert_eq!(sentences.sentences[0].author, SentenceAuthor::Ai);
    assert!(sentences.proposals.is_empty());
}

#[tokio::test]
async fn a_night_only_proposes_when_the_contradicted_sentence_is_the_persons() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    person_edits(
        &db,
        &note,
        "The release ships on Tuesday. I check the release on Tuesday morning.",
    )
    .await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let before = db.get_page(&note).await.unwrap().unwrap();
    let mine = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert_eq!(mine.sentences[1].author, SentenceAuthor::Person);

    let night = night_over_read_notes(
        &db,
        &found,
        &due,
        &[r#"{"contradicted": [{"sentence": 1, "point": 0, "text": "I check the release on Wednesday morning."}]}"#],
    )
    .await;

    assert_eq!(night.read_page_prompts.lock().unwrap().len(), 1);
    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.version, before.version);
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert_eq!(view.proposals.len(), 1, "{:?}", view.proposals);
    let proposal = &view.proposals[0];
    assert_eq!(proposal.after, "I check the release on Wednesday morning.");
    assert!(
        proposal.why.as_deref().unwrap_or("").contains("mem_0"),
        "{proposal:?}"
    );
}

#[tokio::test]
async fn a_night_only_proposes_on_a_locked_note_even_for_the_ais_sentence() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    db.set_page_review_required(&note, true).await.unwrap();
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let before = db.get_page(&note).await.unwrap().unwrap();

    night_over_read_notes(&db, &found, &due, &[SHIPS_ON_TUESDAY]).await;

    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(
        (after.content, after.version),
        (before.content, before.version)
    );
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert_eq!(view.proposals.len(), 1);
}

#[tokio::test]
async fn a_read_note_with_no_contradicted_sentence_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let before = db.get_page(&note).await.unwrap().unwrap();

    // The model finds nothing; and an answer naming a sentence that does not
    // exist, a point that does not exist, or the sentence as it already reads
    // changes nothing either.
    for answer in [
        r#"{"contradicted": []}"#,
        r#"{"contradicted": [{"sentence": 9, "point": 0, "text": "x."}]}"#,
        r#"{"contradicted": [{"sentence": 0, "point": 5, "text": "x."}]}"#,
        r#"{"contradicted": [{"sentence": 0, "point": 0, "text": "The release ships on Tuesday."}]}"#,
    ] {
        let night = night_over_read_notes(&db, &found, &due, &[answer]).await;
        assert_eq!(night.read_page_prompts.lock().unwrap().len(), 1, "{answer}");
        let after = db.get_page(&note).await.unwrap().unwrap();
        assert_eq!(after.version, before.version, "{answer}");
        assert_eq!(after.content, before.content, "{answer}");
        let view = wenlan_core::sentences::page_sentences(&db, &note)
            .await
            .unwrap();
        assert!(view.proposals.is_empty(), "{answer}");
    }
}

#[tokio::test]
async fn a_note_read_in_another_space_is_never_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    db.create_space("home", None, false).await.unwrap();
    let team = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    let home = ai_note(&db, "home", "Chores", "The chores rota changes on Tuesday.").await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &home).await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &team).await;
    let home_before = db.get_page(&home).await.unwrap().unwrap();

    // One answer: it can only go to the one note that is checked.
    let night = night_over_read_notes(&db, &found, &due, &[SHIPS_ON_TUESDAY]).await;

    let asked = night.read_page_prompts.lock().unwrap().clone();
    assert_eq!(
        asked.len(),
        1,
        "only the note in the points' Space is asked about"
    );
    let home_after = db.get_page(&home).await.unwrap().unwrap();
    assert_eq!(home_after.content, home_before.content);
    assert_eq!(home_after.version, home_before.version);
    let team_after = db.get_page(&team).await.unwrap().unwrap();
    assert_eq!(team_after.content, "The release ships on Wednesday.");
}

#[tokio::test]
async fn a_note_is_checked_only_against_points_of_conversations_that_read_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    // C1 read the note; the contradicting point comes only from C0.
    conversation_read(&db, &due, 1, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let before = db.get_page(&note).await.unwrap().unwrap();

    let night = night_over_read_notes(&db, &found, &due, &[SHIPS_ON_TUESDAY]).await;

    assert!(night.read_page_prompts.lock().unwrap().is_empty());
    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(
        (after.content, after.version),
        (before.content, before.version)
    );
}

#[tokio::test]
async fn an_ai_sentence_with_citations_is_replaced_and_the_others_keep_theirs() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(
        &db,
        "team",
        "Release",
        "The release ships on Tuesday. The team reviews on Monday.",
    )
    .await;
    let page = db.get_page(&note).await.unwrap().unwrap();
    let records = serde_json::json!([
        {"occurrence": 1, "marker": 1, "source_kind": "memory", "locator": "first", "score": 0.9, "status": "verified"},
        {"occurrence": 2, "marker": 2, "source_kind": "memory", "locator": "second", "score": 0.9, "status": "verified"}
    ]);
    let cited = wenlan_core::post_write::update_page(
        &db,
        &note,
        wenlan_types::requests::UpdatePageRequest {
            content: "The release ships on Tuesday.[1] The team reviews on Monday.[2]".into(),
            source_memory_ids: page.source_memory_ids.clone(),
            expected_version: None,
            caller_id: None,
            operation_id: None,
        },
        "re_distill",
        false,
        None,
        Some((records.to_string(), "2 verified".into())),
    )
    .await
    .unwrap();
    assert!(cited.wrote);
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;

    night_over_read_notes(&db, &found, &due, &[SHIPS_ON_TUESDAY]).await;

    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "The release ships on Wednesday. The team reviews on Monday.[2]"
    );
    assert_eq!(after.citations.len(), 1, "{:?}", after.citations);
    assert_eq!(after.citations[0].locator, "second");
    assert_eq!(after.citations[0].occurrence, 1);
}

#[tokio::test]
async fn a_contradicted_ai_sentence_can_be_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(
        &db,
        "team",
        "Release",
        "The release ships on Tuesday. The team reviews every change on Monday morning before it goes out.",
    )
    .await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;

    night_over_read_notes(
        &db,
        &found,
        &due,
        &[r#"{"contradicted": [{"sentence": 0, "point": 0, "text": ""}]}"#],
    )
    .await;

    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "The team reviews every change on Monday morning before it goes out."
    );
}

#[tokio::test]
async fn a_computer_that_is_not_the_main_one_writes_and_proposes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    person_edits(
        &db,
        &note,
        "The release ships on Tuesday. I check the release on Tuesday morning.",
    )
    .await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let before = db.get_page(&note).await.unwrap().unwrap();

    let night = crate::things_intake::NOT_THE_MAIN_COMPUTER
        .scope(
            true,
            night_over_read_notes(&db, &found, &due, &[SHIPS_ON_TUESDAY, SHIPS_ON_TUESDAY]),
        )
        .await;

    assert!(night.read_page_prompts.lock().unwrap().is_empty());
    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(
        (after.content, after.version),
        (before.content, before.version)
    );
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert!(view.proposals.is_empty());
}

#[tokio::test]
async fn a_proposal_already_waiting_or_refused_is_not_filed_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    person_edits(
        &db,
        &note,
        "The release ships on Tuesday. I check the release on Tuesday morning.",
    )
    .await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let answer = r#"{"contradicted": [{"sentence": 1, "point": 0, "text": "I check the release on Wednesday morning."}]}"#;

    night_over_read_notes(&db, &found, &due, &[answer]).await;
    // The same night again, with the same point: nothing new is filed.
    night_over_read_notes(&db, &found, &due, &[answer]).await;
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert_eq!(view.proposals.len(), 1, "{:?}", view.proposals);

    // The person says no; the same point does not ask again.
    wenlan_core::sentences::reject_sentence_proposal(&db, &note, &view.proposals[0].id, None)
        .await
        .unwrap();
    night_over_read_notes(&db, &found, &due, &[answer]).await;
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert!(view.proposals.is_empty(), "{:?}", view.proposals);
}

#[tokio::test]
async fn a_point_from_another_space_cannot_ground_a_proposal() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.create_space("team", None, false).await.unwrap();
    db.create_space("home", None, false).await.unwrap();
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: "home_point".into(),
        title: "home_point".into(),
        content: "The release ships on Wednesday.".into(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".into()),
        space: Some("home".into()),
        ..Default::default()
    }])
    .await
    .unwrap();
    let sentences = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    let request = wenlan_types::authorship::SentenceProposeRequest {
        sentence_hash: sentences.sentences[0].hash.clone(),
        text: "The release ships on Wednesday.".into(),
        why: None,
        reason: None,
        expected_version: None,
    };
    let refused = wenlan_core::sentences::propose_contradicted_sentence(
        &db,
        &note,
        &request,
        "wenlan",
        "home_point",
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
}

#[tokio::test]
async fn a_sentence_too_long_to_show_whole_is_left_out_and_cannot_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let long = format!(
        "The release ships on Tuesday {}.",
        "after every check ".repeat(30)
    );
    let note = ai_note(
        &db,
        "team",
        "Release",
        &format!("{long} The team reviews on Monday."),
    )
    .await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let before = db.get_page(&note).await.unwrap().unwrap();

    // The model names sentence 0, the long one it was never shown.
    let night = night_over_read_notes(&db, &found, &due, &[SHIPS_ON_TUESDAY]).await;

    let asked = night.read_page_prompts.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].contains("after every check"), "{}", asked[0]);
    let after = db.get_page(&note).await.unwrap().unwrap();
    assert_eq!(
        (after.content, after.version),
        (before.content, before.version)
    );
}

/// Keeps points as `Keep` does, then forgets each one, as a night's tidy
/// does to a memory it takes the place of.
struct KeepThenEvict<'a>(Keep<'a>);

impl PointWriter for KeepThenEvict<'_> {
    fn write(&self, request: StoreMemoryRequest) -> Written<'_> {
        Box::pin(async move {
            let id = self.0.write(request).await?;
            self.0
                 .0
                .forget_memory(&id, "night-tidy", "evicted by tonight's tidy")
                .await
                .map_err(|e| e.to_string())?;
            Ok(id)
        })
    }
}

#[tokio::test]
async fn a_point_tonights_tidy_evicted_changes_nothing_on_a_read_note() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let ai = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    let mine = ai_note(
        &db,
        "team",
        "Cafeteria",
        "The cafeteria menu rotates weekly.",
    )
    .await;
    person_edits(
        &db,
        &mine,
        "Quarterly tax filings are due in April. I file mine on the first day.",
    )
    .await;
    for note in [&ai, &mine] {
        conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, note).await;
    }
    let before_ai = db.get_page(&ai).await.unwrap().unwrap();
    let before_mine = db.get_page(&mine).await.unwrap().unwrap();
    let keep = KeepThenEvict(Keep(&db, Default::default()));

    let night = night_over_read_notes_in(
        &db,
        &found,
        &due,
        &[
            // The newest-read note is asked first.
            r#"{"contradicted": [{"sentence": 1, "point": 0, "text": "I file mine on the last day."}]}"#,
            SHIPS_ON_TUESDAY,
        ],
        None,
        &keep,
    )
    .await;

    // The model was asked, and both the in-place write and the proposal were
    // held back because the point no longer stands.
    let asked = night.read_page_prompts.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{asked:?}");
    let after_ai = db.get_page(&ai).await.unwrap().unwrap();
    assert_eq!(
        (after_ai.content, after_ai.version),
        (before_ai.content, before_ai.version)
    );
    let after_mine = db.get_page(&mine).await.unwrap().unwrap();
    assert_eq!(after_mine.version, before_mine.version);
    let view = wenlan_core::sentences::page_sentences(&db, &mine)
        .await
        .unwrap();
    assert!(view.proposals.is_empty());
}

#[tokio::test]
async fn a_rejected_suggestion_does_not_come_back_under_another_memory() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    let (found, due) = one_thing_two_conversations(&db, "team", "team").await;
    let note = ai_note(&db, "team", "Release", "The release ships on Tuesday.").await;
    person_edits(
        &db,
        &note,
        "The release ships on Tuesday. I check the release on Tuesday morning.",
    )
    .await;
    conversation_read(&db, &due, 0, wenlan_types::SessionReadKind::ReadPage, &note).await;
    let answer = r#"{"contradicted": [{"sentence": 1, "point": 0, "text": "I check the release on Wednesday morning."}]}"#;
    let reworded =
        r#"{"contradicted": [{"sentence": 1, "point": 0, "text": "I check it on Wednesday."}]}"#;

    // mem_0 makes the claim; the suggestion waits.
    let first = Keep(&db, Default::default());
    night_over_read_notes_in(&db, &found, &due, &[answer], None, &first).await;
    // A different memory (mem_1) makes the same claim, worded another way:
    // still one suggestion on that sentence.
    let second = Keep(&db, std::sync::atomic::AtomicUsize::new(1));
    night_over_read_notes_in(&db, &found, &due, &[reworded], None, &second).await;
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert_eq!(view.proposals.len(), 1, "{:?}", view.proposals);

    // The person says no; a third memory (mem_2) does not bring it back.
    wenlan_core::sentences::reject_sentence_proposal(&db, &note, &view.proposals[0].id, None)
        .await
        .unwrap();
    let third = Keep(&db, std::sync::atomic::AtomicUsize::new(2));
    night_over_read_notes_in(&db, &found, &due, &[answer], None, &third).await;
    let view = wenlan_core::sentences::page_sentences(&db, &note)
        .await
        .unwrap();
    assert!(view.proposals.is_empty(), "{:?}", view.proposals);
}

#[tokio::test]
async fn one_space_comparison_resolves_names_and_ids() {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.create_space("team", None, false).await.unwrap();
    db.create_space("home", None, false).await.unwrap();
    let team = db.get_space("team").await.unwrap().unwrap().id;
    use wenlan_core::sentences::same_space;
    assert!(same_space(&db, Some("team"), Some(&team)).await);
    assert!(same_space(&db, Some(&team), Some("team")).await);
    assert!(!same_space(&db, Some("home"), Some(&team)).await);
    assert!(!same_space(&db, None, Some("team")).await);
}
