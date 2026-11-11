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
                        format!(
                            r#"{{"m": "{m}", "useful": {}, "tool": {habit}, "expires": false, "private": false}}"#,
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
                        Some(with) => format!(
                            r#"{{"m": "{m}", "verdict": "same", "with": "{with}", "sentence": "Releases go out only through the merge queue."}}"#
                        ),
                        None if text.contains("Lore") => {
                            format!(r#"{{"m": "{m}", "verdict": "new"}}"#)
                        }
                        None => format!(r#"{{"m": "{m}", "verdict": "conflict"}}"#),
                    });
                }
                Ok(format!(r#"{{"answers": [{}]}}"#, answers.join(",")))
            }
            "ai_memory_source_check" => {
                let json = prompt.split_once('\n').map(|(_, j)| j).unwrap_or("");
                let value: serde_json::Value = serde_json::from_str(json).unwrap();
                let scores: Vec<String> = value["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        let supported = item["evidence"].as_str().unwrap().contains("merge queue");
                        format!(
                            r#"{{"item_id": "{}", "score": {}}}"#,
                            item["item_id"].as_str().unwrap(),
                            if supported { 0.9 } else { 0.1 }
                        )
                    })
                    .collect();
                Ok(format!(r#"{{"scores": [{}]}}"#, scores.join(",")))
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

/// Writes points straight into the database, as the daemon's store would.
struct Keep<'a>(&'a MemoryDB, std::sync::atomic::AtomicUsize);

impl PointWriter for Keep<'_> {
    fn write(&self, request: StoreMemoryRequest) -> crate::things_intake::Written<'_> {
        Box::pin(async move {
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
                    ..Default::default()
                }])
                .await
                .map_err(|e| e.to_string())?;
            Ok(id)
        })
    }
}

/// The import runs one at a time in a process; so do the tests that run it.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn memory(text: &str) -> FoundMemory {
    FoundMemory {
        tool: MemoryTool::ClaudeCode,
        project_dir: None,
        path: "/nowhere/memory/x.md".into(),
        text: text.into(),
    }
}

fn found() -> Vec<FoundMemory> {
    vec![
        memory("CI publishes with token ghp_abcdefghijklmnopqrstuvwxyz0123456789 set."),
        memory("zsh quoting makes the check pass when it should fail."),
        memory("Lore must check the license before borrowing outside code."),
        memory("Releases only go out through the merge queue, never by hand."),
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

/// A secret never reaches a model, tool habits are left out, a new piece of
/// knowledge is added as a point marked with its tool's memory, one that an
/// existing point already says adds nothing, the import runs once, and its
/// Undo forgets what it wrote and nothing else.
#[tokio::test]
async fn the_import_keeps_knowledge_only_adds_and_undoes_in_one_step() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_a_point(dir.path()).await;
    let model = Judge::new(false);
    let keep = Keep(&db, Default::default());

    let pass = run(&db, model.as_ref(), &keep, found()).await.unwrap();
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
    let added = db
        .get_memory_detail(&pass.written[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        added.content,
        "Lore must check the license before borrowing outside code."
    );
    assert!(
        db.get_memory_detail("mem_release").await.unwrap().is_some(),
        "nothing it held is replaced"
    );

    assert!(
        run(&db, model.as_ref(), &keep, found()).await.is_err(),
        "it runs once until taken back"
    );
    let undone = db.undo_ai_memory_import().await.unwrap().unwrap();
    assert_eq!(undone.written, pass.written);
    for id in &pass.written {
        assert!(db.get_memory_detail(id).await.unwrap().is_none());
    }
    assert!(db.get_memory_detail("mem_release").await.unwrap().is_some());
    assert!(db.ai_memory_import().await.unwrap().is_none());
    assert!(
        run(&db, model.as_ref(), &keep, found()).await.is_ok(),
        "it can run again"
    );
}

/// A model that is down decides nothing and writes nothing: the memories it
/// could not judge are counted, not kept on a guess.
#[tokio::test]
async fn a_failed_model_call_keeps_nothing_on_a_guess() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db_with_a_point(dir.path()).await;
    let model = Judge::new(true);
    let keep = Keep(&db, Default::default());
    let pass = run(&db, model.as_ref(), &keep, found()).await.unwrap();
    assert!(pass.written.is_empty());
    assert_eq!(pass.counts.undecided, 2, "{:?}", pass.counts);
}

/// A memory an existing page already says becomes one more source of that
/// page once the source check agrees, and Undo takes the source off again.
#[tokio::test]
async fn a_memory_a_page_already_says_becomes_one_more_source_of_it() {
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
    let model = Judge::new(false);
    let keep = Keep(&db, Default::default());
    let pass = run(
        &db,
        model.as_ref(),
        &keep,
        vec![memory(
            "Releases only go out through the merge queue, never by hand.",
        )],
    )
    .await
    .unwrap();
    assert_eq!(pass.counts.attached, 1, "{:?}", pass.counts);
    assert_eq!(pass.attached, vec![(page.clone(), pass.written[0].clone())]);
    let sources = db.get_page(&page).await.unwrap().unwrap().source_memory_ids;
    assert!(sources.contains(&pass.written[0]), "{sources:?}");

    db.undo_ai_memory_import().await.unwrap().unwrap();
    let sources = db.get_page(&page).await.unwrap().unwrap().source_memory_ids;
    assert!(!sources.contains(&pass.written[0]), "{sources:?}");
    assert!(sources.contains(&"mem_first".to_string()), "{sources:?}");
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
