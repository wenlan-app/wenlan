// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::{fs, sync::Arc};
use wenlan_types::team::{TeamAccess, TeamSpaceAccess};

use crate::rulesync_team::{self, ReviewError};
use crate::shared_space::version;
use crate::skill_registry::{project_sync::store_test_skill, Scope as SkillScope, SkillFile};
use crate::team::{self, CredentialKind, Member};
use crate::{db::MemoryDB, events::NoopEmitter};

const URL: &str = "http://team.invalid";
const PROJECT: &str = "Project X";
const OTHER: &str = "Project Y";

struct Fixture {
    db: MemoryDB,
    root: tempfile::TempDir,
    team: PathBuf,
    settings: PathBuf,
    writer: (Member, String),
    reader: (Member, String),
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let server = root.path().join("server");
        fs::create_dir(&server).unwrap();
        let db = MemoryDB::new(&server.join("memorydb"), Arc::new(NoopEmitter))
            .await
            .unwrap();
        let team = server.join(team::TEAM_FILE);
        let settings = server.join(SETTINGS_FILE);
        let (owner, _) = team::owner_key(&team, "Owner").unwrap();
        team::rename_team(&team, "Research Team").unwrap();
        let project = [PROJECT, OTHER].map(|space| {
            serde_json::json!({
                "space": space,
                "repo_root": server.join(space),
                "folder": "wiki",
                "remote_url": format!("https://github.com/org/{}", space.replace(' ', "-")),
            })
        });
        fs::write(
            server.join("repo-wiki.json"),
            serde_json::to_vec(&serde_json::json!({"bindings": project})).unwrap(),
        )
        .unwrap();
        for name in [rulesync_team::SPACE, PROJECT, OTHER] {
            db.create_space(name, None, false).await.unwrap();
            let scope = Scope::Space(name.into());
            let folder = team::new_hosted_folder(&settings);
            join_at(&settings, &scope, &folder, "Owner").unwrap();
            update_share(&settings, &scope, |share| share.hosted = true).unwrap();
            ensure_space_file(
                &folder,
                &scope,
                name,
                &SpaceMember {
                    id: owner.id.clone(),
                    name: owner.name.clone(),
                },
            )
            .unwrap();
        }
        team::set_everyone(&team, rulesync_team::SPACE, Some(TeamAccess::Read)).unwrap();
        for name in [rulesync_team::SPACE, PROJECT, OTHER] {
            rulesync_team::ensure_hosted_master(&settings, name).unwrap();
        }
        let other_only =
            rulesync_team::update_hosted_rules(&settings, OTHER, "Private project wording")
                .unwrap();
        rulesync_team::decide_local_space_proposal(&settings, OTHER, &other_only, true).unwrap();
        let writer =
            team::add_member(&team, "Writer", &[grant(PROJECT, TeamAccess::Write)]).unwrap();
        let reader =
            team::add_member(&team, "Reader", &[grant(PROJECT, TeamAccess::Read)]).unwrap();
        Self {
            db,
            root,
            team,
            settings,
            writer,
            reader,
        }
    }

    async fn pull(&self, name: &str, key: &str, space: &str) -> (PathBuf, PathBuf, HostLink) {
        let member = team::authenticate(&self.team, key).unwrap().unwrap().member;
        let hosted = team::hosted_spaces(&self.settings).unwrap();
        let me = member.me(&hosted);
        let grant = me.spaces.iter().find(|entry| entry.space == space).unwrap();
        let server = team::hosted_folder(&self.settings, space).unwrap().unwrap();
        let pages = self.db.shared_folder_pages(space).await.unwrap();
        let list = rulesync_team::hosted_file_list(
            &self.team,
            &self.settings,
            space,
            &server,
            grant.access,
            &pages,
            &member.id,
        )
        .unwrap();
        let settings = self.root.path().join(name).join(SETTINGS_FILE);
        let folder = remote::mirror_folder(&settings, URL, space);
        let host = HostLink {
            key: key.into(),
            url: URL.into(),
            space: space.into(),
            space_id: grant.id.clone(),
        };
        let scope = Scope::Space(space.into());
        join_as(&settings, &scope, &folder, name, Some(&member.id)).unwrap();
        update_share(&settings, &scope, |share| {
            share.host = Some(host.clone());
            share.read_only = grant.access == TeamAccess::Read;
        })
        .unwrap();
        remember_membership(&settings, URL, key, &me).unwrap();
        for path in &list.files {
            if path
                .split('/')
                .nth(3)
                .is_some_and(rulesync_team::is_master_page)
            {
                continue;
            }
            let original = team::served_path(&server, path).unwrap();
            let copy = team::served_path(&folder, path).unwrap();
            fs::create_dir_all(copy.parent().unwrap()).unwrap();
            fs::copy(original, copy).unwrap();
        }
        let manifest = list.rules.as_ref().unwrap();
        let approved =
            rulesync_team::read_hosted_rule_file(&self.team, &self.settings, space, &manifest.file)
                .unwrap()
                .unwrap();
        let layer = rulesync_team::pulled_layer(&host, &list, &approved).unwrap();
        remember_team_rule_pull(&settings, &scope, &host, layer).unwrap();
        (settings, folder, host)
    }

    fn send_reader_proposal(&self, settings: &Path, mirror: &Path, markdown: &str) -> String {
        let layer = rulesync_team::load_project_layers(settings)
            .unwrap()
            .remove(0);
        let sha = rulesync_team::queue_proposal(
            settings,
            &layer.id,
            &layer.revision,
            markdown,
            version::AuthorKind::Ai,
        )
        .unwrap();
        let name = version::version_file_name(&sha);
        let path = mirror
            .join(version::MEMBERS_DIR)
            .join(&self.reader.0.id)
            .join(version::PROPOSALS_DIR)
            .join(&layer.id)
            .join(&name);
        let bytes = fs::read(path).unwrap();
        assert!(rulesync_team::receive_proposal(
            &self.team,
            &self.settings,
            &self.reader.0.id,
            PROJECT,
            &layer.id,
            &name,
            &bytes,
        )
        .unwrap());
        sha
    }

    fn decide(
        &self,
        member: &Member,
        credential: CredentialKind,
        sha: &str,
        accept: bool,
    ) -> Result<(), ReviewError> {
        rulesync_team::decide_proposal(
            &self.team,
            &self.settings,
            PROJECT,
            &member.id,
            credential,
            sha,
            accept,
        )
    }
}

fn grant(space: &str, access: TeamAccess) -> TeamSpaceAccess {
    TeamSpaceAccess {
        space: space.into(),
        access,
        id: None,
    }
}

#[tokio::test]
async fn rulesync_team_rejection_reaches_only_the_proposer_without_a_revision_change() {
    let fixture = Fixture::new().await;
    let (settings, mirror, _) = fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    let original = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    let proposal = fixture.send_reader_proposal(&settings, &mirror, "Await review.\n");
    fixture
        .decide(
            &fixture.writer.0,
            CredentialKind::BrowserSession,
            &proposal,
            false,
        )
        .unwrap();

    fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    let updated = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    assert_eq!(updated.revision, original.revision);
    assert_eq!(updated.markdown, original.markdown);
    assert_eq!(
        updated.proposal_decisions,
        BTreeMap::from([(proposal, false)])
    );

    let (writer, _, _) = fixture.pull("writer", &fixture.writer.1, PROJECT).await;
    assert!(rulesync_team::load_project_layers(&writer).unwrap()[0]
        .proposal_decisions
        .is_empty());
}

#[tokio::test]
async fn rulesync_team_uses_space_grants_and_retains_only_approved_offline_layers() {
    let fixture = Fixture::new().await;
    let reader = team::authenticate(&fixture.team, &fixture.reader.1)
        .unwrap()
        .unwrap()
        .member;
    assert_eq!(reader.access(OTHER), None);
    let (settings, _, _) = fixture
        .pull("reader", &fixture.reader.1, rulesync_team::SPACE)
        .await;
    let (_, mirror, host) = fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    let (writer, _, _) = fixture.pull("writer", &fixture.writer.1, PROJECT).await;
    assert_eq!(rulesync_team::load_layers(&settings).unwrap().len(), 1);
    let original = rulesync_team::load_project_layers(&settings).unwrap();
    assert_eq!(original.len(), 1);
    assert_eq!(original[0].project.as_ref().unwrap().space, PROJECT);
    assert!(!remote::mirror_folder(&settings, URL, OTHER).exists());
    assert!(!fs::read_to_string(&settings)
        .unwrap()
        .contains("Private project wording"));
    assert_eq!(
        original[0].project.as_ref().unwrap().repo_id,
        rulesync_team::repo_identity("git@github.com:org/project-x.git")
    );
    let first = fixture.send_reader_proposal(&settings, &mirror, "Use the review checklist.\n");
    let stale = fixture.send_reader_proposal(&settings, &mirror, "Use a different checklist.\n");
    assert_eq!(
        rulesync_team::load_project_layers(&settings).unwrap(),
        original
    );
    assert_eq!(
        rulesync_team::load_project_layers(&writer).unwrap()[0].markdown,
        ""
    );
    assert!(matches!(
        fixture.decide(
            &fixture.reader.0,
            CredentialKind::BrowserSession,
            &first,
            true
        ),
        Err(ReviewError::Forbidden)
    ));
    assert!(matches!(
        fixture.decide(&fixture.writer.0, CredentialKind::LegacyKey, &first, true),
        Err(ReviewError::Forbidden)
    ));
    assert_eq!(
        rulesync_team::pending_proposals(
            &fixture.team,
            &fixture.settings,
            PROJECT,
            &fixture.writer.0.id,
            CredentialKind::BrowserSession
        )
        .unwrap()
        .len(),
        2
    );
    fixture
        .decide(
            &fixture.writer.0,
            CredentialKind::BrowserSession,
            &first,
            true,
        )
        .unwrap();
    assert!(matches!(
        fixture.decide(
            &fixture.writer.0,
            CredentialKind::BrowserSession,
            &stale,
            true
        ),
        Err(ReviewError::Failed(WenlanError::Conflict(_)))
    ));
    assert!(matches!(
        fixture.decide(
            &fixture.writer.0,
            CredentialKind::BrowserSession,
            &first,
            false
        ),
        Err(ReviewError::Failed(WenlanError::Conflict(_)))
    ));
    assert_eq!(
        rulesync_team::hosted_project_layer(&fixture.settings, PROJECT)
            .unwrap()
            .unwrap()
            .markdown,
        "Use the review checklist.\n"
    );
    fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    fixture.pull("writer", &fixture.writer.1, PROJECT).await;
    let approved = rulesync_team::load_project_layers(&settings).unwrap();
    assert_eq!(approved[0].markdown, "Use the review checklist.\n");
    assert_eq!(
        rulesync_team::project_layer_for_local_space(&settings, PROJECT).unwrap(),
        Some(approved[0].clone())
    );
    assert!(
        rulesync_team::project_layer_for_local_space(&settings, OTHER)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        approved[0].revision,
        rulesync_team::load_project_layers(&writer).unwrap()[0].revision
    );
    clear_rule_layer_if_refused(
        &settings,
        &host,
        &remote::SyncError::Unavailable("offline".into()),
    )
    .unwrap();
    assert_eq!(
        rulesync_team::load_project_layers(&settings).unwrap(),
        approved
    );

    team::update_member(&fixture.team, &fixture.reader.0.id, None, Some(&[])).unwrap();
    let reader = team::authenticate(&fixture.team, &fixture.reader.1)
        .unwrap()
        .unwrap()
        .member;
    assert_eq!(reader.access(PROJECT), None);
    let me = reader.me(&team::hosted_spaces(&fixture.settings).unwrap());
    remember_membership(&settings, URL, &fixture.reader.1, &me).unwrap();
    remember_team_rule_pull(
        &settings,
        &Scope::Space(PROJECT.into()),
        &host,
        Some(approved[0].clone()),
    )
    .unwrap();
    assert!(rulesync_team::load_project_layers(&settings)
        .unwrap()
        .is_empty());
    assert!(
        rulesync_team::project_layer_for_local_space(&settings, PROJECT)
            .unwrap()
            .is_none()
    );
    assert_eq!(rulesync_team::load_layers(&settings).unwrap().len(), 1);
    team::remove_member(&fixture.team, &fixture.reader.0.id).unwrap();
    assert!(team::authenticate(&fixture.team, &fixture.reader.1)
        .unwrap()
        .is_none());
    clear_rule_layer_if_refused(
        &settings,
        &host,
        &remote::SyncError::Revoked("key refused".into()),
    )
    .unwrap();
    assert!(rulesync_team::load_layers(&settings).unwrap().is_empty());
}

#[tokio::test]
async fn rulesync_team_rejects_credentials_before_shipping_a_proposal() {
    let fixture = Fixture::new().await;
    let (settings, _, _) = fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    let layer = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    let credential = team::new_key();
    let error = rulesync_team::queue_proposal(
        &settings,
        &layer.id,
        &layer.revision,
        &credential,
        version::AuthorKind::Ai,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Validation error: possible credential on line 1"
    );
    assert!(
        rulesync_team::local_space_proposals(&fixture.settings, PROJECT)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rulesync_team_distinguishes_registry_read_failure_from_empty_inventory() {
    let fixture = Fixture::new().await;
    let root = fixture.settings.parent().unwrap();
    store_test_skill(
        root,
        SkillScope::Space {
            name: PROJECT.into(),
        },
        "project-checklist",
        vec![],
    );
    let read_manifest =
        || rulesync_team::hosted_manifest(&fixture.team, &fixture.settings, PROJECT);
    assert_eq!(read_manifest().unwrap().unwrap().skills.len(), 1);
    let registry = root
        .join("spaces")
        .join(crate::space_folders::folder_name(PROJECT))
        .join("skills");
    let record = registry
        .join("project-checklist")
        .join(crate::skill_registry::META_DIR)
        .join(crate::skill_registry::RECORD_FILE);
    let saved_record = record.with_extension("backup");
    let displaced = registry.with_extension("backup");
    fs::rename(&registry, &displaced).unwrap();
    // A plain file in the registry location fails even when tests run with broad permissions.
    fs::write(&registry, "registry unavailable").unwrap();
    let remote = read_manifest();
    let local = rulesync_team::load_project_layers(&fixture.settings);
    fs::remove_file(&registry).unwrap();
    let empty = read_manifest().unwrap().unwrap().skills;
    fs::rename(displaced, &registry).unwrap();
    fs::rename(&record, &saved_record).unwrap();
    let partial = read_manifest();
    fs::rename(saved_record, record).unwrap();
    assert!(
        remote.is_err() && local.is_err() && partial.is_err(),
        "a failed inventory was published: remote={remote:?}, local={local:?}, partial={partial:?}"
    );
    assert!(empty.is_empty());
    assert_eq!(read_manifest().unwrap().unwrap().skills.len(), 1);
}

#[tokio::test]
async fn rulesync_team_shares_only_approved_safe_skills_from_the_exact_project() {
    let fixture = Fixture::new().await;
    let root = fixture.settings.parent().unwrap();
    let scope = |name: &str| SkillScope::Space { name: name.into() };
    let file = |path: &str, body: &str| SkillFile {
        path: path.into(),
        bytes: body.as_bytes().to_vec(),
    };
    let skill_document = |name: &str, extra: &str| {
        format!("---\r\nname: {name}\r\ndescription: A registry fixture\r\n{extra}---\r\nUse the approved source.\r\n")
    };
    store_test_skill(root, SkillScope::All, "personal-only", vec![]);
    store_test_skill(
        root,
        scope(OTHER),
        "other-project",
        vec![file("notes.txt", "Private project skill")],
    );
    store_test_skill(
        root,
        scope(PROJECT),
        "project-checklist",
        vec![
            file("SKILL.md", &skill_document("project-checklist", "")),
            file("docs/checklist.md", "Approved checklist"),
        ],
    );
    store_test_skill(
        root,
        scope(PROJECT),
        "with-credential",
        vec![file("notes.txt", &format!("API_KEY={}", team::new_key()))],
    );
    store_test_skill(
        root,
        scope(PROJECT),
        "with-script",
        vec![file("scripts/tool.sh", "#!/bin/sh\n")],
    );
    store_test_skill(
        root,
        scope(PROJECT),
        "with-renderer-directives",
        vec![file(
            "SKILL.md",
            &skill_document("with-renderer-directives", "targets: [claudecode]\r\n"),
        )],
    );
    store_test_skill(
        root,
        scope(PROJECT),
        "with-nested-skill",
        vec![file("docs/skill.md", &skill_document("another-skill", ""))],
    );

    let (settings, _, _) = fixture
        .pull("reader", &fixture.reader.1, rulesync_team::SPACE)
        .await;
    let (_, mirror, host) = fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    assert!(rulesync_team::load_layers(&settings).unwrap()[0]
        .skills
        .is_empty());
    assert!(!remote::mirror_folder(&settings, URL, OTHER).exists());
    assert!(!mirror.join("skills").exists());
    let original = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    assert_eq!(
        original
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["project-checklist"]
    );
    assert!(original.skills[0]
        .files
        .iter()
        .any(|file| file.path == "docs/checklist.md" && file.text == "Approved checklist"));
    let document = &original.skills[0].files[0];
    assert_eq!(document.path, "SKILL.md");
    assert!(document
        .text
        .starts_with("---\nname: project-checklist\ndescription: A registry fixture\n---\n"));
    assert!(document.text.ends_with("Use the approved source.\r\n"));
    let saved = fs::read_to_string(&settings).unwrap();
    assert!(
        !saved.contains("with-credential")
            && !saved.contains("personal-only")
            && !saved.contains("other-project")
    );
    let folder = team::hosted_folder(&fixture.settings, PROJECT)
        .unwrap()
        .unwrap();
    let pages = fixture.db.shared_folder_pages(PROJECT).await.unwrap();
    let list = rulesync_team::hosted_file_list(
        &fixture.team,
        &fixture.settings,
        PROJECT,
        &folder,
        TeamAccess::Read,
        &pages,
        &fixture.reader.0.id,
    )
    .unwrap();
    let manifest = list.rules.as_ref().unwrap();
    let approved = rulesync_team::read_hosted_rule_file(
        &fixture.team,
        &fixture.settings,
        PROJECT,
        &manifest.file,
    )
    .unwrap()
    .unwrap();
    let invalid_bundle = |skills| {
        let mut list = list.clone();
        list.rules.as_mut().unwrap().skills = skills;
        assert!(rulesync_team::pulled_layer(&host, &list, &approved).is_err());
    };
    let mut unsafe_path = manifest.skills.clone();
    unsafe_path[0].files[0].path = "../unsafe.md".into();
    invalid_bundle(unsafe_path);
    let mut unsafe_metadata = manifest.skills.clone();
    unsafe_metadata[0].files[0].text = skill_document(
        "project-checklist",
        "claudecode:\r\n  scheduled-task: fixture\r\n",
    )
    .replace("\r\n", "\n");
    invalid_bundle(unsafe_metadata);

    store_test_skill(
        root,
        scope(PROJECT),
        "project-checklist",
        vec![file("docs/checklist.md", "Updated checklist")],
    );
    fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    let updated = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    assert_eq!(updated.revision, original.revision);
    assert!(updated.skills[0]
        .files
        .iter()
        .any(|file| file.text == "Updated checklist"));

    team::update_member(&fixture.team, &fixture.reader.0.id, None, Some(&[])).unwrap();
    let member = team::authenticate(&fixture.team, &fixture.reader.1)
        .unwrap()
        .unwrap()
        .member;
    let me = member.me(&team::hosted_spaces(&fixture.settings).unwrap());
    remember_membership(&settings, URL, &fixture.reader.1, &me).unwrap();
    assert!(rulesync_team::load_project_layers(&settings)
        .unwrap()
        .is_empty());
    assert!(!fs::read_to_string(&settings)
        .unwrap()
        .contains("Updated checklist"));
}

/// UX-SKILL-7 / team layers: a member's pulled layer carries the name of
/// whoever approved that version and when, so the member's block can say so.
#[tokio::test]
async fn rulesync_team_pulled_layer_names_who_approved_it() {
    let fixture = Fixture::new().await;
    let approved =
        rulesync_team::update_hosted_rules(&fixture.settings, PROJECT, "Use the checklist")
            .unwrap();
    rulesync_team::decide_local_space_proposal(&fixture.settings, PROJECT, &approved, true)
        .unwrap();
    let manifest = rulesync_team::hosted_manifest(&fixture.team, &fixture.settings, PROJECT)
        .unwrap()
        .unwrap();
    assert_eq!(manifest.approved_by.as_deref(), Some("Owner"));
    let (settings, _, _) = fixture.pull("reader", &fixture.reader.1, PROJECT).await;
    let layer = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    assert_eq!(layer.approved_by.as_deref(), Some("Owner"));
    let at = layer.approved_at.clone().unwrap();
    assert_eq!(
        layer.source_line("Project"),
        format!(
            "Project rules · Space {PROJECT} · approved by Owner · {}",
            &at[..10]
        )
    );
}
