// SPDX-License-Identifier: Apache-2.0

use super::*;
use async_trait::async_trait;
use axum::http::HeaderValue;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use wenlan_core::events::NoopEmitter;
use wenlan_core::llm_provider::{LlmBackend, LlmError};
use wenlan_types::requests::{CALLER_HEADER, PERSON_CALLER_VALUE};

const SOURCE_QUOTE: &str = "Always inspect the rendered output before claiming success.";
const RULE_TEXT: &str = "Always inspect the rendered output before claiming success.";

const GENERAL_QUOTE: &str = "Discuss the approach with the person before writing any code.";

struct GrowModel {
    prompts: Mutex<Vec<String>>,
    fail_on: Option<String>,
    /// Pages of this Space answer with GENERAL_QUOTE marked general.
    general_in: Option<String>,
}

impl GrowModel {
    fn new() -> Self {
        Self {
            prompts: Mutex::new(Vec::new()),
            fail_on: None,
            general_in: None,
        }
    }

    fn failing(space: &str) -> Self {
        Self {
            prompts: Mutex::new(Vec::new()),
            fail_on: Some(space.to_string()),
            general_in: None,
        }
    }

    fn call_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmProvider for GrowModel {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        let prompt = request.user_prompt;
        self.prompts.lock().unwrap().push(prompt.clone());
        if self
            .fail_on
            .as_deref()
            .is_some_and(|space| prompt.contains(space))
        {
            return Err(LlmError::NotAvailable);
        }
        match request.label.as_deref() {
            Some("skills_from_library")
                if self
                    .general_in
                    .as_deref()
                    .is_some_and(|space| prompt.contains(&format!("{space:?}"))) =>
            {
                Ok(format!(
                    r#"{{"lines":[{{"kind":"rule","page":"P0","quote":{GENERAL_QUOTE:?},"general":true}}]}}"#
                ))
            }
            Some("skills_from_library") => Ok(format!(
                r#"{{"lines":[{{"kind":"rule","text":{RULE_TEXT:?},"page":"P0","quote":{SOURCE_QUOTE:?}}}]}}"#
            )),
            Some("skills_conflict_check") => Ok(r#"{"suggestions":[]}"#.into()),
            _ => Err(LlmError::NotAvailable),
        }
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "skill-grow-test"
    }

    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

struct EnvGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&'static str, &std::path::Path)]) -> Self {
        let mut previous = Vec::new();
        for (name, value) in values {
            previous.push((*name, std::env::var_os(name)));
            std::env::set_var(name, value);
        }
        Self { previous }
    }

    fn set_text(mut self, values: &[(&'static str, &'static str)]) -> Self {
        for (name, value) in values {
            self.previous.push((*name, std::env::var_os(name)));
            std::env::set_var(name, value);
        }
        self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

struct Scratch {
    _temp: tempfile::TempDir,
    _env: EnvGuard,
    data: PathBuf,
    home: PathBuf,
    config: PathBuf,
    pages: PathBuf,
    db: Arc<MemoryDB>,
    shared: SharedState,
}

impl Scratch {
    async fn new(enabled: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let home = temp.path().join("home");
        let config = temp.path().join("config");
        let pages = temp.path().join("pages");
        for directory in [&data, &home, &config, &pages] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let env = EnvGuard::set(&[
            ("WENLAN_DATA_DIR", &data),
            ("HOME", &home),
            ("XDG_CONFIG_HOME", &config),
        ])
        .set_text(&[
            ("WENLAN_HOST", "http://127.0.0.1:17917"),
            ("WENLAN_NO_AUTOSTART", "1"),
        ]);
        wenlan_core::config::save_config(&wenlan_core::config::Config {
            skills_from_library: enabled,
            knowledge_path: Some(pages.clone()),
            ..Default::default()
        })
        .unwrap();
        let db = Arc::new(
            MemoryDB::new_with_embedding_recovery(
                &data.join("db"),
                Arc::new(NoopEmitter),
                wenlan_core::db::MemoryEmbeddingRecovery::Deferred,
            )
            .await
            .unwrap(),
        );
        let mut server = crate::state::ServerState::default().with_page_root(pages.clone());
        server.db = Some(db.clone());
        Self {
            _temp: temp,
            _env: env,
            data,
            home,
            config,
            pages,
            db,
            shared: Arc::new(RwLock::new(server)),
        }
    }

    fn skill_dirs(&self) -> SkillDirs {
        SkillDirs {
            home: Some(self.home.clone()),
            config: Some(self.config.clone()),
            tool_files: true,
        }
    }
}

fn person_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CALLER_HEADER, HeaderValue::from_static(PERSON_CALLER_VALUE));
    headers
}

async fn create_person_page(
    db: &MemoryDB,
    pages: &Path,
    space: &str,
    title: &str,
    content: &str,
) -> String {
    wenlan_core::post_write::create_page(
        db,
        CreateConceptRequest {
            title: title.into(),
            content: content.into(),
            summary: None,
            entity_id: None,
            space: WriteSpaceTarget::Named(space.into()),
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".into()),
            workspace: Some(space.into()),
        },
        "unknown",
        Some(pages),
    )
    .await
    .unwrap()
    .id
}

async fn refresh(scratch: &Scratch, model: &dyn LlmProvider) -> SkillsRefreshResponse {
    refresh_with(&scratch.shared, model, scratch.skill_dirs())
        .await
        .unwrap()
}

#[tokio::test]
async fn suggests_then_person_accepts_and_reads_one_space_scoped_rules_page() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    assert_eq!(
        std::env::var_os("WENLAN_DATA_DIR"),
        Some(scratch.data.clone().into_os_string())
    );
    let research = scratch
        .db
        .create_space("Research", None, false)
        .await
        .unwrap();
    let personal = scratch
        .db
        .create_space("Personal", None, false)
        .await
        .unwrap();
    scratch
        .db
        .update_space("Personal", "Health", None)
        .await
        .unwrap();
    assert_eq!(
        scratch
            .db
            .space_rules_state(&personal.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SpaceRulesStatus::ExcludedPrivate
    );
    scratch
        .db
        .create_space("Filtered", None, false)
        .await
        .unwrap();
    scratch
        .db
        .create_space("Locked", None, false)
        .await
        .unwrap();
    let source_page_id = create_person_page(
        &scratch.db,
        &scratch.pages,
        "Research",
        "Working agreements",
        SOURCE_QUOTE,
    )
    .await;
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Health",
        "Private detail",
        "Always keep this private detail out of AI tools.",
    )
    .await;
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Filtered",
        "Excluded page",
        &format!("{}\n{}", skill_grow::NOT_FOR_AI_TOOLS_MARKER, SOURCE_QUOTE),
    )
    .await;
    let locked = create_person_page(
        &scratch.db,
        &scratch.pages,
        "Locked",
        "Locked source",
        SOURCE_QUOTE,
    )
    .await;
    scratch
        .db
        .set_page_review_required(&locked, true)
        .await
        .unwrap();

    let model = GrowModel::new();
    let report = refresh(&scratch, &model).await;
    assert_eq!(report.suggestions, 1);
    assert_eq!(report.spaces_updated, 0);
    assert_eq!(report.model_calls, 1);
    assert_eq!(model.call_count(), 1);
    for name in ["Research", "Health", "Filtered", "Locked"] {
        let pages = scratch
            .db
            .list_pages_by_space("active", Some(name), 100, 0)
            .await
            .unwrap();
        assert!(
            pages
                .iter()
                .all(|page| !page.title.starts_with("How I work")),
            "suggestion-only refresh wrote a rules page in {name}"
        );
    }
    assert_eq!(
        scratch
            .db
            .space_rules_state(&research.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SpaceRulesStatus::Suggested
    );
    let suggestions = handle_space_suggestions(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].space_id, research.id);
    assert_eq!(suggestions[0].lines[0].source_quote, SOURCE_QUOTE);

    // AI tool and unsigned requests cannot accept; the person-marked request can.
    assert!(handle_accept_space_rules(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        AxumPath(research.id.clone()),
        Query(SpaceAnswerScope::default()),
    )
    .await
    .is_err());
    let accepted = handle_accept_space_rules(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(research.id.clone()),
        Query(SpaceAnswerScope::default()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(accepted.status, "accepted");
    let rules_page_id = accepted.rules_page_id.unwrap();
    let page = scratch.db.get_page(&rules_page_id).await.unwrap().unwrap();
    assert_eq!(page.title, "How I work — Research");
    assert!(page.content.contains(SOURCE_QUOTE));
    assert!(page
        .content
        .contains(&suggestions[0].lines[0].id.to_string()));

    let response = handle_read_rules(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        Query(RulesQuery {
            space: Some("Research".into()),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(response.space_id, research.id);
    assert_eq!(response.lines.len(), 1);
    assert_eq!(response.lines[0].source_page_title, "Working agreements");
    assert_eq!(response.lines[0].source_quote, SOURCE_QUOTE);
    assert!(response.lines[0].source_revision > 0);
    println!(
        "--- scratch How I work page ---\n{}\n--- scratch read_space_rules response ---\n{}\n--- end example ---",
        page.content,
        serde_json::to_string_pretty(&response).unwrap()
    );

    // Model a nightly addition, then exercise the person-only one-step Undo.
    // Page history stores both versions; Undo restores the exact prior body.
    let mut rules_state = scratch
        .db
        .space_rules_state(&research.id)
        .await
        .unwrap()
        .unwrap();
    let prior_body = page.content.clone();
    let mut additional = rules_state.lines[0].clone();
    additional.id = "nightly-addition".into();
    additional.text = "Use isolated test data before writing pages.".into();
    let existing_line = rules_state.lines[0].clone();
    assert!(write_maintained_rules_page(
        &scratch.db,
        Some(&scratch.pages),
        &mut rules_state,
        &[existing_line, additional],
    )
    .await
    .unwrap());
    scratch
        .db
        .set_space_rules_state(&rules_state)
        .await
        .unwrap();
    assert!(rules_state.undo.is_some());
    assert!(handle_undo_space_rules(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        AxumPath(research.id.clone()),
    )
    .await
    .is_err());
    let undone = handle_undo_space_rules(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(research.id.clone()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(undone.status, "undone");
    assert_eq!(
        scratch
            .db
            .get_page(&rules_page_id)
            .await
            .unwrap()
            .unwrap()
            .content,
        prior_body
    );

    // The first input hash was persisted at suggestion time; an unchanged pass
    // does not call the model again for this Space.
    let before_calls = model.call_count();
    let unchanged = refresh(&scratch, &model).await;
    assert_eq!(model.call_count(), before_calls);
    assert!(unchanged.spaces_unchanged >= 1);

    scratch
        .db
        .record_skill_rules_read("codex", &research.id, &response.lines)
        .await
        .unwrap();
    let source = scratch.db.get_page(&source_page_id).await.unwrap().unwrap();
    wenlan_core::post_write::rename_page(
        &scratch.db,
        &source_page_id,
        "Renamed private source",
        source.version,
        &wenlan_core::read_scope::ReadScope::Global,
        Some(&scratch.pages),
    )
    .await
    .unwrap();
    let records = scratch
        .db
        .skill_borrow_records(&research.id, 0, i64::MAX)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].page_title, "Working agreements");

    // Moving either the source or the approved rules page out of this Space
    // revokes the old Space's read immediately.
    scratch.db.create_space("Other", None, false).await.unwrap();
    let source = scratch.db.get_page(&source_page_id).await.unwrap().unwrap();
    scratch
        .db
        .set_page_space(
            &source_page_id,
            Some("Other"),
            source.version,
            &wenlan_core::read_scope::ReadScope::Global,
        )
        .await
        .unwrap();
    assert!(read_rules_for_space(&scratch.db, &research)
        .await
        .unwrap()
        .lines
        .is_empty());
    let source = scratch.db.get_page(&source_page_id).await.unwrap().unwrap();
    scratch
        .db
        .set_page_space(
            &source_page_id,
            Some("Research"),
            source.version,
            &wenlan_core::read_scope::ReadScope::Global,
        )
        .await
        .unwrap();
    let rules_page = scratch.db.get_page(&rules_page_id).await.unwrap().unwrap();
    scratch
        .db
        .set_page_space(
            &rules_page_id,
            Some("Other"),
            rules_page.version,
            &wenlan_core::read_scope::ReadScope::Global,
        )
        .await
        .unwrap();
    assert!(read_rules_for_space(&scratch.db, &research)
        .await
        .unwrap()
        .lines
        .is_empty());
    let rules_page = scratch.db.get_page(&rules_page_id).await.unwrap().unwrap();
    scratch
        .db
        .set_page_space(
            &rules_page_id,
            Some("Research"),
            rules_page.version,
            &wenlan_core::read_scope::ReadScope::Global,
        )
        .await
        .unwrap();

    // Page-level opt-out and lock revoke even an accepted line at read time.
    scratch
        .db
        .set_page_review_required(&rules_page_id, true)
        .await
        .unwrap();
    assert!(handle_read_rules(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        Query(RulesQuery {
            space: Some("Research".into()),
        }),
    )
    .await
    .unwrap()
    .0
    .lines
    .is_empty());
    scratch
        .db
        .set_page_review_required(&rules_page_id, false)
        .await
        .unwrap();
    let current = scratch.db.get_page(&rules_page_id).await.unwrap().unwrap();
    let opted_out = format!(
        "{}\n{}",
        skill_grow::NOT_FOR_AI_TOOLS_MARKER,
        current.content
    );
    scratch
        .db
        .update_page_content(&rules_page_id, &opted_out, &[], "manual_edit")
        .await
        .unwrap();
    assert!(handle_read_rules(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        Query(RulesQuery {
            space: Some("Research".into()),
        }),
    )
    .await
    .unwrap()
    .0
    .lines
    .is_empty());

    // UX-SKILL-1: asked once for the whole library. After that yes, another
    // Space with lines is accepted by the next night with no ask, and its
    // acceptance is bound to the Space id through a rename.
    assert!(wenlan_core::config::load_config().skills_answered);
    let other = scratch
        .db
        .create_space("Second Space", None, false)
        .await
        .unwrap();
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Second Space",
        "Rules",
        SOURCE_QUOTE,
    )
    .await;
    let report = refresh(&scratch, &model).await;
    assert_eq!(report.suggestions, 0, "a second Space was asked again");
    assert!(handle_space_suggestions(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
    )
    .await
    .unwrap()
    .0
    .is_empty());
    let second = scratch
        .db
        .space_rules_state(&other.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.status, SpaceRulesStatus::Accepted);
    assert!(second.rules_page_id.is_some());
    scratch
        .db
        .update_space("Second Space", "Renamed Space", None)
        .await
        .unwrap();
    let _ = refresh(&scratch, &model).await;
    assert_eq!(
        scratch
            .db
            .space_rules_state(&other.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SpaceRulesStatus::Accepted
    );

    // Turning the feature off returns no rules and records no borrowing event.
    let records_before_off = scratch
        .db
        .skill_borrow_records(&research.id, 0, i64::MAX)
        .await
        .unwrap()
        .len();
    wenlan_core::config::update_config(|config| {
        config.skills_from_library = false;
        Ok(())
    })
    .unwrap();
    let off_response = handle_read_rules(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        Query(RulesQuery {
            space: Some("Research".into()),
        }),
    )
    .await
    .unwrap()
    .0;
    assert!(off_response.lines.is_empty());
    assert_eq!(
        scratch
            .db
            .skill_borrow_records(&research.id, 0, i64::MAX)
            .await
            .unwrap()
            .len(),
        records_before_off
    );
}

#[tokio::test]
async fn one_space_failure_is_backed_off_without_repeating_other_space_work() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    scratch
        .db
        .create_space("Failing", None, false)
        .await
        .unwrap();
    scratch
        .db
        .create_space("Healthy", None, false)
        .await
        .unwrap();
    for space in ["Failing", "Healthy"] {
        create_person_page(
            &scratch.db,
            &scratch.pages,
            space,
            "Agreements",
            SOURCE_QUOTE,
        )
        .await;
    }
    let model = GrowModel::failing("Failing");
    let first = refresh(&scratch, &model).await;
    assert_eq!(first.failures, 1);
    assert_eq!(first.suggestions, 1);
    assert_eq!(model.call_count(), 2);
    let failed_state = scratch
        .db
        .space_rules_states()
        .await
        .unwrap()
        .into_iter()
        .find(|state| state.space_name == "Failing")
        .unwrap();
    assert_eq!(failed_state.failure_count, 1);
    assert!(failed_state.next_retry_at > chrono::Utc::now().timestamp());
    let before = model.call_count();
    let second = refresh(&scratch, &model).await;
    assert!(second.spaces_unchanged >= 1);
    assert_eq!(model.call_count(), before);
}

#[tokio::test]
async fn existing_skill_suggestions_are_person_only_positional_and_undo_conflict_safe() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    let space = scratch
        .db
        .create_space("Research", None, false)
        .await
        .unwrap();
    let source_id = create_person_page(
        &scratch.db,
        &scratch.pages,
        "Research",
        "Release process",
        "Use the merge queue because required checks run there.",
    )
    .await;
    let source = scratch.db.get_page(&source_id).await.unwrap().unwrap();
    let revision = scratch
        .db
        .get_page_history_at(&source_id, source.version)
        .await
        .unwrap()
        .unwrap();
    let approved_line = SkillLine {
        id: "rule-1".into(),
        kind: wenlan_types::SkillLineKind::Decision,
        text: "Use the merge queue because required checks run there.".into(),
        source_page_id: source_id,
        source_page_title: source.title,
        source_revision: revision.version,
        source_revision_at: revision.created_at,
        source_quote: "Use the merge queue because required checks run there.".into(),
        person_edited: false,
        general: false,
    };
    let rules_body = skill_grow::reconcile_rules_page(
        &space.name,
        std::slice::from_ref(&approved_line),
        &[],
        &[],
        None,
        false,
    );
    let rules_page_id = create_person_page(
        &scratch.db,
        &scratch.pages,
        "Research",
        "How I work — Research",
        &rules_body.body,
    )
    .await;
    scratch
        .db
        .set_space_rules_state(&SpaceRulesState {
            space_id: space.id.clone(),
            space_name: space.name,
            status: SpaceRulesStatus::Accepted,
            rules_page_id: Some(rules_page_id),
            lines: rules_body.lines,
            deleted_fingerprints: Vec::new(),
            input_hash: String::new(),
            last_success_at: chrono::Utc::now().timestamp(),
            failure_count: 0,
            next_retry_at: 0,
            undo: None,
        })
        .await
        .unwrap();
    let skills_root = scratch.home.join(".agents/skills");
    let folder = skills_root.join("ship");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("SKILL.md");
    let original =
        "---\nname: ship\n---\nExplanation: Merge directly to main.\n- Merge directly to main.\n";
    std::fs::write(&file, original).unwrap();
    let suggestion = SkillSuggestion {
        id: "replace-line".into(),
        skill: "ship".into(),
        old_line: "- Merge directly to main.".into(),
        new_line: "Use the merge queue because required checks run there.".into(),
        reason: "A newer decision changes the landing procedure.".into(),
        source_rule_id: "rule-1".into(),
        source_page: "Release process".into(),
    };
    scratch
        .db
        .set_skill_suggestion(&StoredSkillSuggestion {
            suggestion: suggestion.clone(),
            skill_file: file.to_string_lossy().into_owned(),
            skill_root: skills_root.to_string_lossy().into_owned(),
            status: SuggestionStatus::Pending,
            backup_file: None,
            before_hash: Some(content_hash(original)),
            applied_hash: None,
        })
        .await
        .unwrap();
    let ai_attempt = handle_apply_suggestion(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        AxumPath(suggestion.id.clone()),
    )
    .await;
    assert!(ai_attempt.is_err());
    let applied = handle_apply_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(suggestion.id.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(applied.applied && applied.undo_available);
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.contains("Explanation: Merge directly to main."));
    assert!(after.contains("- Use the merge queue because required checks run there."));

    let undone = handle_undo_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(suggestion.id.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(!undone.applied && !undone.undo_available);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);

    let mut dismissed = suggestion.clone();
    dismissed.id = "dismiss-me".into();
    scratch
        .db
        .set_skill_suggestion(&StoredSkillSuggestion {
            suggestion: dismissed.clone(),
            skill_file: file.to_string_lossy().into_owned(),
            skill_root: skills_root.to_string_lossy().into_owned(),
            status: SuggestionStatus::Pending,
            backup_file: None,
            before_hash: Some(content_hash(original)),
            applied_hash: None,
        })
        .await
        .unwrap();
    assert!(handle_dismiss_suggestion(
        axum::extract::State(scratch.shared.clone()),
        HeaderMap::new(),
        AxumPath(dismissed.id.clone()),
    )
    .await
    .is_err());
    let dismissed_response = handle_dismiss_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(dismissed.id.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(!dismissed_response.applied && !dismissed_response.undo_available);

    // Re-apply then edit the file. Undo must refuse and keep both versions.
    let stored = scratch
        .db
        .skill_suggestions()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.suggestion.id == suggestion.id)
        .unwrap();
    scratch
        .db
        .set_skill_suggestion(&StoredSkillSuggestion {
            status: SuggestionStatus::Pending,
            backup_file: None,
            applied_hash: None,
            ..stored
        })
        .await
        .unwrap();
    let _reapplied = handle_apply_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(suggestion.id.clone()),
    )
    .await
    .unwrap();
    let after = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{after}\nHuman improvement.\n")).unwrap();

    // An edit after Apply makes Undo refuse, retaining current and prior files.
    let refused = handle_undo_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(suggestion.id.clone()),
    )
    .await;
    assert!(refused.is_err());
    assert!(file.exists());
    let stored = scratch
        .db
        .skill_suggestions()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.suggestion.id == suggestion.id)
        .unwrap();
    assert_eq!(stored.status, SuggestionStatus::Applied);
    assert!(stored
        .backup_file
        .as_deref()
        .is_some_and(|backup| Path::new(backup).is_file()));

    // Crash recovery: an Applying row with SKILL.md already moved aside can
    // finish, and an Undoing row with only the applied copy can restore.
    std::fs::write(&file, original).unwrap();
    let mut crash = suggestion;
    crash.id = "crash-recovery".into();
    let after = apply_suggestion_text(original, &crash).unwrap();
    let backup = folder.join(format!("SKILL.md.wenlan-before-{}", crash.id));
    std::fs::write(&backup, original).unwrap();
    let aside = folder.join(format!(".SKILL.md.wenlan-replace-{}", crash.id));
    std::fs::rename(&file, &aside).unwrap();
    scratch
        .db
        .set_skill_suggestion(&StoredSkillSuggestion {
            suggestion: crash.clone(),
            skill_file: file.to_string_lossy().into_owned(),
            skill_root: skills_root.to_string_lossy().into_owned(),
            status: SuggestionStatus::Applying,
            backup_file: Some(backup.to_string_lossy().into_owned()),
            before_hash: Some(content_hash(original)),
            applied_hash: Some(content_hash(&after)),
        })
        .await
        .unwrap();
    let _ = handle_apply_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(crash.id.clone()),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), after);
    assert!(!aside.exists());

    let applied_copy = folder.join(format!("SKILL.md.wenlan-applied-{}", crash.id));
    std::fs::rename(&file, &applied_copy).unwrap();
    let mut recovering = stored_suggestion(&scratch.db, &crash.id).await.unwrap();
    recovering.status = SuggestionStatus::Undoing;
    scratch.db.set_skill_suggestion(&recovering).await.unwrap();
    let _ = handle_undo_suggestion(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(crash.id),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert_eq!(std::fs::read_to_string(&applied_copy).unwrap(), after);
}

#[tokio::test]
async fn existing_skill_discovery_never_follows_skill_file_symlinks() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("skills");
    let folder = root.join("escape");
    std::fs::create_dir_all(&folder).unwrap();
    let outside = scratch.path().join("outside.md");
    std::fs::write(&outside, "private source must not be read by the model").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, folder.join("SKILL.md")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, folder.join("SKILL.md")).unwrap();
    assert!(unmanaged_skill_files(&[root]).is_empty());

    let real_root = scratch.path().join("real-skills");
    let real_folder = real_root.join("private");
    std::fs::create_dir_all(&real_folder).unwrap();
    std::fs::write(real_folder.join("SKILL.md"), "private skill").unwrap();
    let linked_root = scratch.path().join("linked-skills");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real_root, &linked_root).unwrap();
    assert!(unmanaged_skill_files(&[linked_root]).is_empty());
}

#[tokio::test]
async fn ux_skill_1_one_no_stops_asking_and_model_calls_everywhere() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    let first = scratch.db.create_space("First", None, false).await.unwrap();
    scratch
        .db
        .create_space("Second", None, false)
        .await
        .unwrap();
    for name in ["First", "Second"] {
        create_person_page(&scratch.db, &scratch.pages, name, "Rules", SOURCE_QUOTE).await;
    }
    let model = GrowModel::new();
    let report = refresh(&scratch, &model).await;
    assert_eq!(report.suggestions, 2);

    let declined = handle_decline_space_rules(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(first.id.clone()),
        Query(SpaceAnswerScope::default()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(declined.status, "declined");
    let config = wenlan_core::config::load_config();
    assert!(config.skills_answered);
    assert!(!config.skills_from_library, "a no must stop the night pass");
    assert!(
        handle_space_suggestions(
            axum::extract::State(scratch.shared.clone()),
            person_headers(),
        )
        .await
        .unwrap()
        .0
        .is_empty(),
        "the other Space still asks after one no"
    );
    let calls = model.call_count();
    assert!(refresh_with(&scratch.shared, &model, scratch.skill_dirs())
        .await
        .is_err());
    assert_eq!(model.call_count(), calls, "a no still spent model calls");
}

#[tokio::test]
async fn ux_skill_1_an_answer_with_the_box_cleared_covers_only_that_space() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    let first = scratch.db.create_space("First", None, false).await.unwrap();
    let second = scratch
        .db
        .create_space("Second", None, false)
        .await
        .unwrap();
    let third = scratch.db.create_space("Third", None, false).await.unwrap();
    for name in ["First", "Second", "Third"] {
        create_person_page(&scratch.db, &scratch.pages, name, "Rules", SOURCE_QUOTE).await;
    }
    let model = GrowModel::new();
    assert_eq!(refresh(&scratch, &model).await.suggestions, 3);
    let only_this = || {
        Query(SpaceAnswerScope {
            this_space_only: true,
        })
    };

    let accepted = handle_accept_space_rules(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(first.id.clone()),
        only_this(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(accepted.status, "accepted");
    let declined = handle_decline_space_rules(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(second.id.clone()),
        only_this(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(declined.status, "declined");

    let config = wenlan_core::config::load_config();
    assert!(
        !config.skills_answered,
        "a one-Space answer is not the library answer"
    );
    assert!(
        config.skills_from_library,
        "a one-Space no must not turn it off"
    );
    let still_asking = handle_space_suggestions(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        still_asking
            .iter()
            .map(|s| s.space_id.clone())
            .collect::<Vec<_>>(),
        vec![third.id.clone()],
        "only the unanswered Space still asks"
    );
    refresh(&scratch, &model).await;
    let second_state = scratch
        .db
        .space_rules_state(&second.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_state.status, SpaceRulesStatus::Declined);
}

#[tokio::test]
async fn ux_skill_5_general_rules_are_read_in_every_space_and_with_no_space() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    let general = scratch
        .db
        .create_space("Habits", None, false)
        .await
        .unwrap();
    let project = scratch
        .db
        .create_space("Project", None, false)
        .await
        .unwrap();
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Habits",
        "Habits",
        GENERAL_QUOTE,
    )
    .await;
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Project",
        "Rules",
        SOURCE_QUOTE,
    )
    .await;
    let model = GrowModel {
        general_in: Some("Habits".into()),
        ..GrowModel::new()
    };
    assert_eq!(refresh(&scratch, &model).await.suggestions, 2);
    for space in [&general, &project] {
        let _ = handle_accept_space_rules(
            axum::extract::State(scratch.shared.clone()),
            person_headers(),
            AxumPath(space.id.clone()),
            Query(SpaceAnswerScope::default()),
        )
        .await
        .unwrap();
    }
    let read = |space: Option<&str>| {
        let shared = scratch.shared.clone();
        let space = space.map(str::to_string);
        async move {
            let mut headers = HeaderMap::new();
            headers.insert("x-agent-name", HeaderValue::from_static("codex"));
            handle_read_rules(
                axum::extract::State(shared),
                headers,
                Query(RulesQuery { space }),
            )
            .await
            .unwrap()
            .0
        }
    };

    let in_project = read(Some("Project")).await;
    let quotes: Vec<_> = in_project
        .lines
        .iter()
        .map(|l| l.source_quote.as_str())
        .collect();
    assert_eq!(quotes, vec![SOURCE_QUOTE, GENERAL_QUOTE]);
    assert!(in_project.lines[1].general);

    let nowhere = read(None).await;
    let quotes: Vec<_> = nowhere
        .lines
        .iter()
        .map(|l| l.source_quote.as_str())
        .collect();
    assert_eq!(
        quotes,
        vec![GENERAL_QUOTE],
        "no Space reads only the general rules"
    );

    // A report of the general rule from another Space lands on its own Space.
    let mut headers = HeaderMap::new();
    headers.insert("x-agent-name", HeaderValue::from_static("codex"));
    let _ = handle_self_report(
        axum::extract::State(scratch.shared.clone()),
        headers,
        Json(SkillReportRequest {
            space: "Project".into(),
            rule_id: in_project.lines[1].id.clone(),
            report: wenlan_types::SkillReportKind::ChangedPlan,
        }),
    )
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp();
    let records = scratch
        .db
        .skill_borrow_records(&general.id, now - 3_600, now + 3_600)
        .await
        .unwrap();
    assert!(records
        .iter()
        .any(|r| r.self_report == Some(wenlan_types::SkillReportKind::ChangedPlan)));
}

#[tokio::test]
async fn ux_skill_6_approved_rules_are_written_as_layered_files_and_follow_the_setting() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    let habits = scratch
        .db
        .create_space("Habits", None, false)
        .await
        .unwrap();
    let project = scratch
        .db
        .create_space("Project", None, false)
        .await
        .unwrap();
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Habits",
        "Habits",
        GENERAL_QUOTE,
    )
    .await;
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Project",
        "Rules",
        SOURCE_QUOTE,
    )
    .await;
    let model = GrowModel {
        general_in: Some("Habits".into()),
        ..GrowModel::new()
    };
    assert_eq!(refresh(&scratch, &model).await.suggestions, 2);

    let general_file = scratch.data.join("AGENTS.md");
    let project_dir = scratch.data.join("spaces").join("Project");
    let project_file = project_dir.join("AGENTS.md");
    assert!(
        !general_file.exists() && !project_file.exists(),
        "a suggestion nobody accepted writes no file"
    );
    // The person's own file in a layer folder is never Wenlan's to change.
    std::fs::create_dir_all(&project_dir).unwrap();
    let own = project_dir.join("decisions.md");
    std::fs::write(&own, "my own notes\n").unwrap();

    for space in [&habits, &project] {
        let _ = handle_accept_space_rules(
            axum::extract::State(scratch.shared.clone()),
            person_headers(),
            AxumPath(space.id.clone()),
            Query(SpaceAnswerScope::default()),
        )
        .await
        .unwrap();
    }
    let general = std::fs::read_to_string(&general_file).unwrap();
    assert!(general.starts_with(skill_grow::RULES_FILE_MARK));
    assert!(general.contains(GENERAL_QUOTE) && !general.contains(SOURCE_QUOTE));
    let own_rules = std::fs::read_to_string(&project_file).unwrap();
    assert!(own_rules.contains("# How I work — Project"));
    assert!(own_rules.contains(RULE_TEXT) && own_rules.contains("(from \"Rules\")"));
    assert!(
        !own_rules.contains(GENERAL_QUOTE),
        "a general rule is written once, in the general layer"
    );
    assert!(
        !scratch.data.join("spaces").join("Habits").exists(),
        "a Space with only general rules has no layer of its own"
    );
    assert_eq!(std::fs::read_to_string(&own).unwrap(), "my own notes\n");

    // Turning the setting off removes what Wenlan wrote and nothing else.
    wenlan_core::config::update_config(|cfg| {
        cfg.skills_from_library = false;
        Ok(())
    })
    .unwrap();
    sync_rules_files_quietly(&scratch.db).await;
    assert!(!general_file.exists() && !project_file.exists());
    assert_eq!(std::fs::read_to_string(&own).unwrap(), "my own notes\n");

    // A file of the person's own at a name Wenlan uses is left as it is.
    std::fs::write(&general_file, "# mine\n").unwrap();
    wenlan_core::config::update_config(|cfg| {
        cfg.skills_from_library = true;
        Ok(())
    })
    .unwrap();
    sync_rules_files_quietly(&scratch.db).await;
    assert_eq!(std::fs::read_to_string(&general_file).unwrap(), "# mine\n");
    assert!(project_file.exists());
}

#[tokio::test]
async fn ux_skill_8_general_rules_are_kept_in_each_connected_tools_own_file() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = Scratch::new(true).await;
    let habits = scratch
        .db
        .create_space("Habits", None, false)
        .await
        .unwrap();
    create_person_page(
        &scratch.db,
        &scratch.pages,
        "Habits",
        "Habits",
        GENERAL_QUOTE,
    )
    .await;
    for tool in ["codex", "claude-code"] {
        scratch.db.register_trusted_agent(tool).await.unwrap();
    }
    let codex_file = scratch.home.join(".codex/AGENTS.md");
    let claude_file = scratch.home.join(".claude/CLAUDE.md");
    for file in [&codex_file, &claude_file] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    }
    std::fs::write(&codex_file, "# Mine\n\nUse pnpm.\n").unwrap();

    let model = GrowModel {
        general_in: Some("Habits".into()),
        ..GrowModel::new()
    };
    assert_eq!(refresh(&scratch, &model).await.suggestions, 1);
    assert_eq!(
        std::fs::read_to_string(&codex_file).unwrap(),
        "# Mine\n\nUse pnpm.\n",
        "nothing is written before the person says yes"
    );
    let _ = handle_accept_space_rules(
        axum::extract::State(scratch.shared.clone()),
        person_headers(),
        AxumPath(habits.id.clone()),
        Query(SpaceAnswerScope::default()),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&codex_file).unwrap(),
        "# Mine\n\nUse pnpm.\n",
        "a run on scratch data never writes the person's tool files by itself"
    );
    sync_rules_files_in(&scratch.db, &scratch.skill_dirs()).await;

    let codex = std::fs::read_to_string(&codex_file).unwrap();
    assert!(codex.starts_with("# Mine\n\nUse pnpm.\n\n<!-- WENLAN:RULES:START -->"));
    assert!(codex.contains(&format!("- {GENERAL_QUOTE}\n")));
    let claude = std::fs::read_to_string(&claude_file).unwrap();
    assert!(claude.starts_with("<!-- WENLAN:RULES:START -->") && claude.contains(GENERAL_QUOTE));
    assert!(
        !scratch.home.join(".gemini").exists(),
        "a tool that is not there gets no folder"
    );

    // A tool the person turned off loses the part; the other keeps it.
    scratch
        .db
        .update_agent("claude-code", None, None, Some(false), None, None)
        .await
        .unwrap();
    sync_rules_files_in(&scratch.db, &scratch.skill_dirs()).await;
    assert!(!claude_file.exists());
    assert!(std::fs::read_to_string(&codex_file)
        .unwrap()
        .contains(GENERAL_QUOTE));

    let set = |on: bool| {
        wenlan_core::config::update_config(move |cfg| {
            cfg.skills_from_library = on;
            Ok(())
        })
        .unwrap();
    };
    // Off takes Wenlan's part out and leaves the person's text as it was.
    set(false);
    sync_rules_files_in(&scratch.db, &scratch.skill_dirs()).await;
    assert_eq!(
        std::fs::read_to_string(&codex_file).unwrap(),
        "# Mine\n\nUse pnpm.\n"
    );
    assert!(!claude_file.exists(), "a file with only Wenlan's part goes");

    // A file that already brings in a written one gets no second copy.
    std::fs::write(&claude_file, "@~/.codex/AGENTS.md\n").unwrap();
    set(true);
    sync_rules_files_in(&scratch.db, &scratch.skill_dirs()).await;
    assert!(std::fs::read_to_string(&codex_file)
        .unwrap()
        .contains(GENERAL_QUOTE));
    assert_eq!(
        std::fs::read_to_string(&claude_file).unwrap(),
        "@~/.codex/AGENTS.md\n"
    );
}
