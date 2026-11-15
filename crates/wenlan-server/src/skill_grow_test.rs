// SPDX-License-Identifier: Apache-2.0

use super::*;
use async_trait::async_trait;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use wenlan_core::events::NoopEmitter;
use wenlan_core::llm_provider::{LlmBackend, LlmError};
use wenlan_types::requests::CreateConceptRequest;

struct GrowModel {
    prompts: Mutex<Vec<String>>,
}

impl GrowModel {
    fn new() -> Self {
        Self {
            prompts: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmProvider for GrowModel {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.prompts.lock().unwrap().push(request.user_prompt);
        match request.label.as_deref() {
            Some("skills_from_library") => Ok(r#"{"lines":[
                {"kind":"rule","text":"Verify the user-visible surface before claiming completion.","page":"P0","lasting":true,"tool":false,"expires":false,"others":false,"personal":false,"person_words":true,"support_pages":["P0"]},
                {"kind":"decision","text":"Keep the source page canonical because installed skills are disposable copies.","page":"P0","lasting":true,"tool":false,"expires":false,"others":false,"personal":false,"person_words":true,"support_pages":["P0"]},
                {"kind":"pitfall","text":"A successful background launch is not proof that the run completed.","page":"P0","lasting":true,"tool":false,"expires":false,"others":false,"personal":false,"person_words":true,"support_pages":["P0"]}
            ]}"#.into()),
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
        // The canonical create contract records an authored page from the app
        // as the person's when there is no AI agent identity.
        "unknown",
        Some(pages),
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn refresh_isolated_home_writes_source_skill_and_borrowing_records() {
    let _lock = crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let scratch = tempfile::tempdir().unwrap();
    let data = scratch.path().join("data");
    let home = scratch.path().join("home");
    let config_dir = scratch.path().join("config");
    let pages = scratch.path().join("pages");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&pages).unwrap();
    let _env = EnvGuard::set(&[("WENLAN_DATA_DIR", &data), ("HOME", &home)]).set_text(&[
        ("WENLAN_HOST", "http://127.0.0.1:0"),
        ("WENLAN_NO_AUTOSTART", "1"),
    ]);
    wenlan_core::config::save_config(&wenlan_core::config::Config {
        skills_from_library: true,
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
    db.register_agent("codex").await.unwrap();
    db.create_space("Research", None, false).await.unwrap();
    db.create_space("Personal", None, false).await.unwrap();
    db.create_space("Filtered", None, false).await.unwrap();
    db.create_space("Locked", None, false).await.unwrap();
    create_person_page(
        &db,
        &pages,
        "Research",
        "Working agreements",
        "Verify the user-visible surface before claiming completion. Keep the source page canonical because installed skills are disposable copies. A successful background launch is not proof that the run completed.",
    )
    .await;
    create_person_page(
        &db,
        &pages,
        "Personal",
        "Private detail",
        "Never extract this private Space.",
    )
    .await;
    create_person_page(
        &db,
        &pages,
        "Filtered",
        "Explicitly excluded",
        &format!(
            "{}\nNever send this page to the model.",
            skill_grow::NOT_FOR_AI_TOOLS_MARKER
        ),
    )
    .await;
    let locked = create_person_page(
        &db,
        &pages,
        "Locked",
        "Locked instructions",
        "Never send this locked page to the model.",
    )
    .await;
    db.set_page_review_required(&locked, true).await.unwrap();

    let mut state = crate::state::ServerState::default().with_page_root(pages.clone());
    state.db = Some(db.clone());
    let shared = Arc::new(RwLock::new(state));
    let model = GrowModel::new();
    let report = refresh_with(
        &shared,
        &model,
        SkillDirs {
            home: Some(home.clone()),
            config: Some(config_dir),
        },
    )
    .await
    .unwrap();
    assert_eq!(report.spaces_read, 3); // Personal is excluded before extraction.
    assert_eq!(report.skills_written, 1);

    let skill_dir = home.join(".agents/skills/wenlan-research");
    assert_eq!(
        std::fs::read_to_string(skill_dir.join(MANAGED_MARKER)).unwrap(),
        MANAGED_VALUE
    );
    let skill = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    assert!(skill.contains("name: wenlan-research"));
    assert!(skill.contains("Source: Working agreements"));
    println!("--- scratch generated SKILL.md ---\n{skill}--- end SKILL.md ---");

    {
        let prompts = model.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("Working agreements"));
        assert!(!prompts[0].contains("Private detail"));
        assert!(!prompts[0].contains("Explicitly excluded"));
        assert!(!prompts[0].contains("Locked instructions"));
    }

    let source = db
        .skill_page_states()
        .await
        .unwrap()
        .into_iter()
        .find(|state| state.space_name == "Research")
        .unwrap();
    let source_page = db.get_page(&source.page_id).await.unwrap().unwrap();
    assert_eq!(source_page.title, "How I work — Research");
    assert!(source_page.content.contains("wenlan-skill-line:"));

    db.record_skill_page_reads("codex", std::slice::from_ref(&source.page_id))
        .await
        .unwrap();
    assert!(db
        .record_skill_self_report(
            "codex",
            &source.page_id,
            "This rule changed the verification plan; contact me@example.com",
        )
        .await
        .unwrap());
    let now = chrono::Utc::now().timestamp();
    let records = db.skill_borrow_records(now - 2, now + 2).await.unwrap();
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .any(|record| !record.ai_reported && record.report.is_none()));
    assert!(records.iter().any(|record| {
        record.ai_reported
            && record
                .report
                .as_deref()
                .is_some_and(|text| !text.contains("me@example.com"))
    }));

    let unchanged = refresh_with(
        &shared,
        &model,
        SkillDirs {
            home: Some(home.clone()),
            config: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(unchanged.source_pages_written, 0);
    assert_eq!(unchanged.skills_written, 0);
    assert_eq!(unchanged.skills_unchanged, 1);

    db.update_space("Research", "Field Notes", None)
        .await
        .unwrap();
    refresh_with(
        &shared,
        &model,
        SkillDirs {
            home: Some(home.clone()),
            config: None,
        },
    )
    .await
    .unwrap();
    let renamed_dir = home.join(".agents/skills/wenlan-field-notes");
    assert!(!skill_dir.exists());
    assert!(renamed_dir.join("SKILL.md").is_file());
    let renamed_source = db.get_page(&source.page_id).await.unwrap().unwrap();
    assert_eq!(renamed_source.title, "How I work — Field Notes");

    db.delete_space("Field Notes", "unassign").await.unwrap();
    refresh_with(
        &shared,
        &model,
        SkillDirs {
            home: Some(home.clone()),
            config: None,
        },
    )
    .await
    .unwrap();
    assert!(!renamed_dir.exists());

    // Stop uses only durable install rows plus the feature-specific marker.
    // Seed one exact managed copy so the route still proves its own removal
    // after the rename/delete checks consumed the real Space copy.
    let stop_parent = home.join(".agents/skills");
    assert_eq!(
        write_managed_skill(&stop_parent, "wenlan-stop-check", "stop me\n").unwrap(),
        InstallOutcome::Written
    );
    let stop_dir = stop_parent.join("wenlan-stop-check");
    db.set_skill_install_state(&SkillInstallState {
        tool: "codex".into(),
        name: "wenlan-stop-check".into(),
        dir: stop_dir.to_string_lossy().into_owned(),
        content_hash: content_hash("stop me\n"),
    })
    .await
    .unwrap();
    let _ = handle_stop(axum::extract::State(shared.clone()))
        .await
        .unwrap();
    assert!(!stop_dir.exists());
    assert!(!wenlan_core::config::load_config().skills_from_library);
    assert!(refresh_with(
        &shared,
        &model,
        SkillDirs {
            home: Some(home),
            config: None,
        },
    )
    .await
    .is_err());
}

#[test]
fn managed_folder_boundary_never_overwrites_or_removes_a_user_skill() {
    let scratch = tempfile::tempdir().unwrap();
    let parent = scratch.path().join("skills");
    let user = parent.join("wenlan-research");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("SKILL.md"), "the person's own skill\n").unwrap();

    assert_eq!(
        write_managed_skill(&parent, "wenlan-research", "generated\n").unwrap(),
        InstallOutcome::Unmanaged
    );
    assert_eq!(
        std::fs::read_to_string(user.join("SKILL.md")).unwrap(),
        "the person's own skill\n"
    );
    assert!(!remove_managed_skill(&user).unwrap());
    assert!(user.exists());
}

#[test]
fn duplicate_and_reserved_space_slugs_get_stable_distinct_folders() {
    let mut spaces = vec![
        SpaceLines {
            space_id: "12345678-a".into(),
            space_name: "A B".into(),
            slug: "a-b".into(),
            lines: Vec::new(),
        },
        SpaceLines {
            space_id: "87654321-b".into(),
            space_name: "A-B".into(),
            slug: "a-b".into(),
            lines: Vec::new(),
        },
        SpaceLines {
            space_id: "abcdef00-c".into(),
            space_name: "Me".into(),
            slug: "me".into(),
            lines: Vec::new(),
        },
    ];
    disambiguate_space_slugs(&mut spaces);
    assert_eq!(spaces[0].slug, "a-b-12345678");
    assert_eq!(spaces[1].slug, "a-b-87654321");
    assert_eq!(spaces[2].slug, "me-abcdef00");
}

#[tokio::test]
async fn suggestion_apply_keeps_one_backup_and_supports_one_undo_or_dismissal() {
    let scratch = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new_with_embedding_recovery(
            &scratch.path().join("db"),
            Arc::new(NoopEmitter),
            wenlan_core::db::MemoryEmbeddingRecovery::Deferred,
        )
        .await
        .unwrap(),
    );
    let folder = scratch.path().join("skills/ship");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("SKILL.md");
    let original = "---\nname: ship\n---\n- Merge directly to main.\n";
    std::fs::write(&file, original).unwrap();
    let first = SkillSuggestion {
        id: "first".into(),
        skill: "ship".into(),
        old_line: "- Merge directly to main.".into(),
        new_line: "Use the merge queue because required checks run there.".into(),
        reason: "The library has a newer release decision.".into(),
        source_page: "Release process".into(),
    };
    db.set_skill_suggestion(&StoredSkillSuggestion {
        suggestion: first.clone(),
        skill_file: file.to_string_lossy().into_owned(),
        status: SuggestionStatus::Pending,
        backup_file: None,
    })
    .await
    .unwrap();
    let second = SkillSuggestion {
        id: "second".into(),
        skill: "ship".into(),
        old_line: String::new(),
        new_line: "A green launch acknowledgement is not completion evidence.".into(),
        reason: "The library records a missing pitfall.".into(),
        source_page: "Delivery evidence".into(),
    };
    db.set_skill_suggestion(&StoredSkillSuggestion {
        suggestion: second.clone(),
        skill_file: file.to_string_lossy().into_owned(),
        status: SuggestionStatus::Pending,
        backup_file: None,
    })
    .await
    .unwrap();
    let server = crate::state::ServerState {
        db: Some(db.clone()),
        ..Default::default()
    };
    let state = Arc::new(RwLock::new(server));

    let applied = handle_apply_suggestion(
        axum::extract::State(state.clone()),
        AxumPath(first.id.clone()),
    )
    .await
    .unwrap()
    .0;
    assert!(applied.applied && applied.undo_available);
    assert_eq!(
        std::fs::read_to_string(folder.join("SKILL.md.wenlan-backup")).unwrap(),
        original
    );
    assert!(std::fs::read_to_string(&file).unwrap().contains(
        "- Use the merge queue because required checks run there. — Source: Release process"
    ));

    assert!(handle_apply_suggestion(
        axum::extract::State(state.clone()),
        AxumPath(second.id.clone()),
    )
    .await
    .is_err());
    let undone = handle_undo_suggestion(axum::extract::State(state.clone()), AxumPath(first.id))
        .await
        .unwrap()
        .0;
    assert!(!undone.applied && !undone.undo_available);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert!(!folder.join("SKILL.md.wenlan-backup").exists());

    let dismissed = handle_dismiss_suggestion(axum::extract::State(state), AxumPath(second.id))
        .await
        .unwrap()
        .0;
    assert!(!dismissed.applied && !dismissed.undo_available);
    let stored = db.skill_suggestions().await.unwrap();
    assert!(stored
        .iter()
        .any(|item| item.status == SuggestionStatus::Dismissed));
}

#[tokio::test]
async fn source_page_title_collision_never_claims_a_persons_page() {
    let scratch = tempfile::tempdir().unwrap();
    let pages = scratch.path().join("pages");
    std::fs::create_dir_all(&pages).unwrap();
    let db = MemoryDB::new_with_embedding_recovery(
        &scratch.path().join("db"),
        Arc::new(NoopEmitter),
        wenlan_core::db::MemoryEmbeddingRecovery::Deferred,
    )
    .await
    .unwrap();
    let space = db.create_space("Research", None, false).await.unwrap();
    let existing = create_person_page(
        &db,
        &pages,
        "Research",
        "How I work — Research",
        "This title and this person's content were already here.",
    )
    .await;
    let lines = [
        SkillLine {
            kind: wenlan_types::SkillLineKind::Rule,
            text: "Verify the visible behavior before reporting completion.".into(),
            source_page: "Agreements".into(),
        },
        SkillLine {
            kind: wenlan_types::SkillLineKind::Decision,
            text: "Keep canonical source material separate from generated copies.".into(),
            source_page: "Agreements".into(),
        },
        SkillLine {
            kind: wenlan_types::SkillLineKind::Pitfall,
            text: "A launched task is not evidence of a completed run.".into(),
            source_page: "Agreements".into(),
        },
    ];
    let result = write_source_page(
        &db,
        Some(&pages),
        None,
        SourcePageSpec {
            space_id: &space.id,
            space_name: &space.name,
            skill_slug: "research",
            title: "How I work — Research",
            target: WriteSpaceTarget::Named(space.name.clone()),
            workspace: Some(space.name.clone()),
            extracted: &lines,
        },
    )
    .await;
    assert!(matches!(result, Err(ServerError::Conflict(_))));
    assert_eq!(
        db.get_page(&existing).await.unwrap().unwrap().content,
        "This title and this person's content were already here."
    );
    assert!(db.skill_page_states().await.unwrap().is_empty());
}
