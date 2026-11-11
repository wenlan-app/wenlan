// SPDX-License-Identifier: Apache-2.0

use super::*;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use wenlan_core::ai_memory_import::MemoryTool;
use wenlan_core::events::NoopEmitter;
use wenlan_core::llm_provider::{LlmBackend, LlmError};

/// Answers each step from what its prompt shows, the way a careful model
/// would, and keeps every prompt.
struct Judge {
    prompts: Mutex<Vec<(String, String)>>,
    /// Fail the match step, as a model that is down would.
    match_down: bool,
}

impl Judge {
    fn new(match_down: bool) -> Arc<Self> {
        Arc::new(Self {
            prompts: Mutex::new(Vec::new()),
            match_down,
        })
    }
}

fn lines_with_prefix<'a>(prompt: &'a str, prefix: &str) -> Vec<(String, &'a str)> {
    prompt
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let (label, rest) = l.split_once(": ")?;
            let label = label.split_whitespace().next()?;
            (label.starts_with(prefix) && label[1..].chars().all(|c| c.is_ascii_digit()))
                .then(|| (label.to_string(), rest))
        })
        .collect()
}

#[async_trait]
impl LlmProvider for Judge {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        let label = request.label.clone().unwrap_or_default();
        let prompt = request.user_prompt.clone();
        self.prompts
            .lock()
            .unwrap()
            .push((label.clone(), prompt.clone()));
        match label.as_str() {
            "ai_memory_keep" => {
                let answers: Vec<String> = lines_with_prefix(&prompt, "M")
                    .into_iter()
                    .map(|(m, text)| {
                        let habit = text.contains("zsh");
                        let others = text.contains("Dana");
                        let personal = text.contains("dentist") || others;
                        format!(
                            r#"{{"m": "{m}", "useful": {}, "tool": {habit}, "expires": false, "others": {others}, "personal": {personal}}}"#,
                            !habit
                        )
                    })
                    .collect();
                Ok(format!(r#"{{"answers": [{}]}}"#, answers.join(",")))
            }
            "ai_memory_match" if self.match_down => Err(LlmError::NotAvailable),
            "ai_memory_match" => {
                let mut answers = Vec::new();
                for (m, text) in lines_with_prefix(&prompt, "M") {
                    let same = text.contains("queue").then(|| {
                        prompt.lines().find_map(|l| {
                            let l = l.trim_start();
                            let (label, rest) = l.split_once(' ')?;
                            (label.starts_with(&format!("{m}K"))
                                && rest.contains("Releases go out only through the merge queue."))
                            .then(|| label.to_string())
                        })
                    });
                    answers.push(match same.flatten() {
                        Some(with) => {
                            format!(r#"{{"m": "{m}", "verdict": "same", "with": "{with}"}}"#)
                        }
                        None if text.contains("Lore") || text.contains("dentist") => {
                            format!(r#"{{"m": "{m}", "verdict": "new"}}"#)
                        }
                        None => format!(r#"{{"m": "{m}", "verdict": "conflict"}}"#),
                    });
                }
                Ok(format!(r#"{{"answers": [{}]}}"#, answers.join(",")))
            }
            _ => Err(LlmError::NotAvailable),
        }
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "judge"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

/// Writes points straight into the database, as the daemon's store would,
/// and keeps every request.
struct Keep<'a>(
    &'a MemoryDB,
    std::sync::atomic::AtomicUsize,
    Mutex<Vec<StoreMemoryRequest>>,
);

fn keep(db: &MemoryDB) -> Keep<'_> {
    Keep(db, Default::default(), Mutex::new(Vec::new()))
}

impl PointWriter for Keep<'_> {
    fn write(&self, request: StoreMemoryRequest) -> crate::things_intake::Written<'_> {
        Box::pin(async move {
            self.2.lock().unwrap().push(request.clone());
            let n = self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let id = format!("aimem_{n}");
            self.0
                .upsert_documents(vec![wenlan_core::sources::RawDocument {
                    source: "memory".into(),
                    source_id: id.clone(),
                    title: request.title.unwrap_or_default(),
                    content: request.content,
                    last_modified: chrono::Utc::now().timestamp(),
                    memory_type: request.memory_type,
                    space: request.space.named().map(str::to_string),
                    source_agent: request.source_agent,
                    supersedes: request.supersedes,
                    ..Default::default()
                }])
                .await
                .map_err(|e| e.to_string())?;
            Ok(id)
        })
    }
}

/// A pass runs one at a time in a process; so do the tests that run it.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn memory_in(file: &str, text: &str) -> FoundMemory {
    FoundMemory {
        tool: MemoryTool::ClaudeCode,
        project_dir: None,
        path: format!("/nowhere/memory/{file}").into(),
        text: text.into(),
    }
}

fn memory(text: &str) -> FoundMemory {
    memory_in("x.md", text)
}

fn seen(found: Vec<FoundMemory>) -> Seen {
    Seen {
        found,
        present: vec![MemoryTool::ClaudeCode, MemoryTool::Codex],
    }
}

fn found() -> Vec<FoundMemory> {
    vec![
        memory_in(
            "a.md",
            "CI publishes with token ghp_abcdefghijklmnopqrstuvwxyz0123456789 set.",
        ),
        memory_in(
            "b.md",
            "zsh quoting makes the check pass when it should fail.",
        ),
        memory_in(
            "c.md",
            "Lore must check the license before borrowing outside code.",
        ),
        memory_in(
            "d.md",
            "Releases only go out through the merge queue, never by hand.",
        ),
    ]
}

async fn db_with_a_point(dir: &std::path::Path) -> MemoryDB {
    let db = MemoryDB::new(dir, Arc::new(NoopEmitter)).await.unwrap();
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: "mem_release".into(),
        title: "Releases".into(),
        content: "Releases go out only through the merge queue.".into(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("decision".into()),
        source_agent: Some("claude-code".into()),
        ..Default::default()
    }])
    .await
    .unwrap();
    db
}

async fn on(db: &MemoryDB) {
    start(db).await.unwrap();
}

/// The id of the point a memory became, if it became one.
async fn point_of(db: &MemoryDB, text: &str) -> Option<String> {
    let key = wenlan_core::ai_memory_import::memory_key(&memory(text));
    db.ai_memory_ledger()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.key == key)
        .find_map(|r| match r.decided {
            Decided::Point { id, .. } => Some(id),
            _ => None,
        })
}

/// A secret never reaches a model, tool habits are left out, new knowledge
/// is added as a point marked with its tool's memory, one Wenlan already
/// holds adds nothing, a second pass over the same files asks nothing again,
/// and turning it off forgets what it wrote and nothing else.
#[tokio::test]
async fn following_keeps_knowledge_only_asks_once_and_turns_off_in_one_step() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_a_point(dir.path()).await;
    let model = Judge::new(false);
    let keep = keep(&db);

    assert!(
        follow(&db, Some(model.as_ref()), &keep, seen(found()))
            .await
            .is_err(),
        "nothing is followed until the person turns it on"
    );
    on(&db).await;
    let pass = follow(&db, Some(model.as_ref()), &keep, seen(found()))
        .await
        .unwrap();
    let c = &pass.counts;
    assert_eq!(c.read, 4);
    assert_eq!(c.sensitive, 1, "{c:?}");
    assert_eq!(c.not_knowledge, 1, "{c:?}");
    assert_eq!(c.added, 1, "{c:?}");
    assert_eq!(c.added + c.already + c.conflicts, 2, "{c:?}");
    assert_eq!(pass.by_tool, vec![("claude-code".to_string(), 4)]);
    for (_, prompt) in model.prompts.lock().unwrap().iter() {
        assert!(!prompt.contains("ghp_"), "a secret reached a model");
    }
    let written = keep.2.lock().unwrap().clone();
    assert_eq!(written.len(), 1);
    assert_eq!(
        written[0].content,
        "Lore must check the license before borrowing outside code."
    );
    assert_eq!(
        written[0].source_agent.as_deref(),
        Some("claude-code-memory")
    );
    assert!(
        written[0].supersedes.is_none(),
        "a new memory replaces nothing"
    );

    let asked = model.prompts.lock().unwrap().len();
    let again = follow(&db, Some(model.as_ref()), &keep, seen(found()))
        .await
        .unwrap();
    assert_eq!(model.prompts.lock().unwrap().len(), asked, "decided once");
    assert_eq!(
        (
            again.counts.added,
            again.counts.removed,
            again.counts.updated
        ),
        (0, 0, 0)
    );

    let undone = db.undo_ai_memory_import().await.unwrap();
    assert!(undone.is_some());
    assert!(db.get_memory_detail("aimem_0").await.unwrap().is_none());
    assert!(db.get_memory_detail("mem_release").await.unwrap().is_some());
    assert!(db.ai_memory_import().await.unwrap().is_none());
    assert!(db.ai_memory_ledger().await.unwrap().is_empty());
}

/// A model that is down decides nothing and writes nothing; the next pass
/// asks again.
#[tokio::test]
async fn a_failed_model_call_keeps_nothing_on_a_guess_and_is_asked_again() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_a_point(dir.path()).await;
    on(&db).await;
    let down = Judge::new(true);
    let keep = keep(&db);
    let pass = follow(&db, Some(down.as_ref()), &keep, seen(found()))
        .await
        .unwrap();
    assert!(keep.2.lock().unwrap().is_empty());
    assert_eq!(pass.counts.undecided, 2, "{:?}", pass.counts);

    let up = Judge::new(false);
    let pass = follow(&db, Some(up.as_ref()), &keep, seen(found()))
        .await
        .unwrap();
    assert_eq!(pass.counts.added, 1, "{:?}", pass.counts);
}

/// With no model set up, nothing new is read: new memories wait.
#[tokio::test]
async fn without_a_model_new_memories_wait() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_a_point(dir.path()).await;
    on(&db).await;
    let keep = keep(&db);
    let pass = follow(&db, None, &keep, seen(found())).await.unwrap();
    assert_eq!((pass.counts.sensitive, pass.counts.waiting), (1, 3));
    assert!(keep.2.lock().unwrap().is_empty());
}

/// An edit in the tool brings its own point up to date where the point is
/// now: the new words supersede the old, so pages built on it are rewritten.
#[tokio::test]
async fn an_edited_memory_brings_its_own_point_up_to_date() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    on(&db).await;
    let model = Judge::new(false);
    let keep = keep(&db);
    let old = "Lore must check the license before borrowing outside code.";
    follow(&db, Some(model.as_ref()), &keep, seen(vec![memory(old)]))
        .await
        .unwrap();
    let first = point_of(&db, old).await.unwrap();

    let new = "Lore must check the license and the notice file before borrowing outside code.";
    let pass = follow(&db, Some(model.as_ref()), &keep, seen(vec![memory(new)]))
        .await
        .unwrap();
    assert_eq!(
        (pass.counts.updated, pass.counts.added),
        (1, 0),
        "{:?}",
        pass.counts
    );
    let request = keep.2.lock().unwrap().last().unwrap().clone();
    assert_eq!(request.supersedes.as_deref(), Some(first.as_str()));
    assert_eq!(request.content, new);
    assert_eq!(request.space, WriteSpaceTarget::Uncategorized);
    assert!(point_of(&db, new).await.is_some());
    assert!(
        point_of(&db, old).await.is_none(),
        "the old text is no longer followed"
    );
}

/// A memory gone from the tool takes its own point with it; a point the
/// person edited stays theirs; a point the person removed does not come back.
#[tokio::test]
async fn a_gone_memory_takes_only_its_own_untouched_point() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    on(&db).await;
    let model = Judge::new(false);
    let keep = keep(&db);
    let a = memory_in(
        "a.md",
        "Lore must check the license before borrowing outside code.",
    );
    let b = memory_in(
        "b.md",
        "Lore keeps every borrowed file under its original license.",
    );
    let c = memory_in(
        "c.md",
        "Lore lists borrowed code in a notice file at the root.",
    );
    follow(
        &db,
        Some(model.as_ref()),
        &keep,
        seen(vec![a.clone(), b.clone(), c.clone()]),
    )
    .await
    .unwrap();
    let ids: Vec<String> = (0..keep.2.lock().unwrap().len())
        .map(|n| format!("aimem_{n}"))
        .collect();
    assert_eq!(ids.len(), 3);
    // The person edits b's point and removes c's.
    db.update_memory(
        &ids[1],
        "Lore keeps borrowed files under their own license, always.",
    )
    .await
    .unwrap();
    db.delete_by_source_id("memory", &ids[2]).await.unwrap();

    // The tool drops a and b; c is still there.
    let asked = model.prompts.lock().unwrap().len();
    let pass = follow(&db, Some(model.as_ref()), &keep, seen(vec![c.clone()]))
        .await
        .unwrap();
    assert_eq!(pass.counts.removed, 1, "{:?}", pass.counts);
    assert!(db.get_memory_detail(&ids[0]).await.unwrap().is_none());
    assert!(
        db.get_memory_detail(&ids[1]).await.unwrap().is_some(),
        "an edited point is the person's"
    );
    assert!(db.get_memory_detail(&ids[2]).await.unwrap().is_none());
    assert_eq!(
        model.prompts.lock().unwrap().len(),
        asked,
        "a removed point is not brought back"
    );
}

/// A tool whose memory folder is missing is left as it was: its memories are
/// not read as deleted.
#[tokio::test]
async fn a_missing_memory_folder_removes_nothing() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    on(&db).await;
    let model = Judge::new(false);
    let keep = keep(&db);
    follow(
        &db,
        Some(model.as_ref()),
        &keep,
        seen(vec![memory(
            "Lore must check the license before borrowing outside code.",
        )]),
    )
    .await
    .unwrap();
    let pass = follow(
        &db,
        Some(model.as_ref()),
        &keep,
        Seen {
            found: Vec::new(),
            present: vec![MemoryTool::Codex],
        },
    )
    .await
    .unwrap();
    assert_eq!(pass.counts.removed, 0);
    assert!(db.get_memory_detail("aimem_0").await.unwrap().is_some());
}

/// A memory an existing page already says adds nothing and changes
/// nothing: the page keeps its sources as they were.
#[tokio::test]
async fn a_memory_a_page_already_says_adds_nothing() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: "mem_first".into(),
        title: "Releases".into(),
        content: "We ship releases through the merge queue so checks always run.".into(),
        last_modified: chrono::Utc::now().timestamp(),
        source_agent: Some("claude-code".into()),
        ..Default::default()
    }])
    .await
    .unwrap();
    let page = wenlan_core::post_write::create_page(
        &db,
        wenlan_types::requests::CreateConceptRequest {
            title: "Releases".into(),
            content: "Releases go out only through the merge queue.".into(),
            summary: None,
            entity_id: None,
            space: WriteSpaceTarget::Uncategorized,
            source_memory_ids: vec!["mem_first".into()],
            creation_kind: None,
            workspace: None,
        },
        "wenlan",
        None,
    )
    .await
    .unwrap()
    .id;
    let before = db.get_page(&page).await.unwrap().unwrap().source_memory_ids;
    on(&db).await;
    let model = Judge::new(false);
    let keep = keep(&db);
    let pass = follow(
        &db,
        Some(model.as_ref()),
        &keep,
        seen(vec![memory(
            "Releases only go out through the merge queue, never by hand.",
        )]),
    )
    .await
    .unwrap();
    assert_eq!(pass.counts.already, 1, "{:?}", pass.counts);
    assert!(keep.2.lock().unwrap().is_empty());
    assert_eq!(
        db.get_page(&page).await.unwrap().unwrap().source_memory_ids,
        before
    );
}

/// The person's own life is kept and filed unsorted, never in the project's
/// Space; someone else's personal details are left out.
#[tokio::test]
async fn the_persons_own_life_is_kept_apart_and_other_peoples_is_left_out() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    on(&db).await;
    let model = Judge::new(false);
    let keep = keep(&db);
    let pass = follow(
        &db,
        Some(model.as_ref()),
        &keep,
        seen(vec![
            memory_in("a.md", "I see the dentist every March for a checkup."),
            memory_in("b.md", "Dana's divorce is why she moved teams last spring."),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        (pass.counts.added, pass.counts.not_knowledge),
        (1, 1),
        "{:?}",
        pass.counts
    );
    let kept = db.get_memory_detail("aimem_0").await.unwrap().unwrap();
    assert_eq!(kept.content, "I see the dentist every March for a checkup.");
    assert_eq!(kept.space, None, "filed unsorted, not in a project's Space");
}

/// Reading is all the import does to the tools' own files.
#[test]
fn scanning_leaves_the_tools_files_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".claude/projects/p/memory/a.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "Releases only go out through the merge queue.\n").unwrap();
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    let found = wenlan_core::ai_memory_import::scan(&TranscriptRoots {
        claude_dir: Some(dir.path().join(".claude")),
        codex_dir: None,
    });
    assert_eq!(found.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Releases only go out through the merge queue.\n"
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        before
    );
}

/// Stores each point the way a long one is stored, in parts: only the first
/// part's words are read back. Refuses one too short to be a memory, as the
/// store's quality checks do.
struct Parts<'a>(Keep<'a>);

impl PointWriter for Parts<'_> {
    fn write(&self, mut request: StoreMemoryRequest) -> crate::things_intake::Written<'_> {
        Box::pin(async move {
            if request.content.contains("tiny") {
                return Err("Quality gate rejected: Only 4 meaningful words".into());
            }
            request.content = request.content.chars().take(30).collect();
            self.0.write(request).await
        })
    }
}

/// One memory the store refuses is left out and the pass goes on; a point
/// stored in parts is still Wenlan's own, so turning following off takes it
/// back.
#[tokio::test]
async fn a_refused_memory_does_not_stop_the_pass_and_a_long_point_is_taken_back() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
        .await
        .unwrap();
    on(&db).await;
    let model = Judge::new(false);
    let parts = Parts(keep(&db));
    let pass = follow(
        &db,
        Some(model.as_ref()),
        &parts,
        seen(vec![
            memory_in("a.md", "Lore tiny note that the store refuses."),
            memory_in(
                "b.md",
                "Lore must check the license before borrowing outside code, every time.",
            ),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        (pass.counts.added, pass.counts.not_knowledge),
        (1, 1),
        "{:?}",
        pass.counts
    );
    let id = point_of(
        &db,
        "Lore must check the license before borrowing outside code, every time.",
    )
    .await
    .unwrap();
    // Asked once: the refused one is not asked about again.
    let asked = model.prompts.lock().unwrap().len();
    follow(
        &db,
        Some(model.as_ref()),
        &parts,
        seen(vec![
            memory_in("a.md", "Lore tiny note that the store refuses."),
            memory_in(
                "b.md",
                "Lore must check the license before borrowing outside code, every time.",
            ),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(model.prompts.lock().unwrap().len(), asked);

    db.undo_ai_memory_import().await.unwrap();
    assert!(
        db.get_memory_detail(&id).await.unwrap().is_none(),
        "a point in parts is taken back"
    );
}
