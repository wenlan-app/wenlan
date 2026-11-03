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
    let group =
        Scripted::new(&[r#"{"new": [{"title": "首頁改版", "conversations": ["C0", "C1"]}]}"#]);
    let models = IntakeModels {
        label: label.clone(),
        group: group.clone(),
    };

    let report = look_with(&db, found.clone(), &models, "Traditional Chinese")
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
            things_made: 1
        }
    );
    let things = db.list_things(&ReadScope::Global, None).await.unwrap();
    assert_eq!(things.len(), 1);
    assert_eq!(things[0].title.as_deref(), Some("首頁改版"));
    assert_eq!(things[0].status_line.as_deref(), Some("藏起來的按鈕做好了"));
    assert_eq!(things[0].spaces, vec!["wenlan".to_string()]);
    assert!(!things[0].quiet);

    // The grouping model saw only the labels, never what was said.
    let grouped = group.prompts.lock().unwrap().join("\n");
    assert!(grouped.contains("首頁卡片") && grouped.contains("in Traditional Chinese"));
    assert!(!grouped.contains("hide button") && !grouped.contains("smaller cards"));

    // Looking again with nothing new reads nothing and asks nobody.
    let report = look_with(&db, found, &models, "Traditional Chinese")
        .await
        .unwrap();
    assert_eq!((report.labelled, report.grouped), (0, 0));
    assert_eq!(label.prompts.lock().unwrap().len(), 3);
    assert_eq!(group.prompts.lock().unwrap().len(), 1);
}
