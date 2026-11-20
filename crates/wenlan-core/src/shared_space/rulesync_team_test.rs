// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::fs;
use wenlan_types::team::{TeamAccess, TeamFileList, TeamSpaceAccess};

use crate::rulesync_team::{self, ReviewError};
use crate::team::{self, CredentialKind, Member};

const URL: &str = "http://team.invalid";
const PROJECT: &str = "Project X";
const OTHER: &str = "Project Y";

struct Fixture {
    root: tempfile::TempDir,
    team: PathBuf,
    settings: PathBuf,
    writer: (Member, String),
    reader: (Member, String),
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let server = root.path().join("server");
        fs::create_dir(&server).unwrap();
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
            root,
            team,
            settings,
            writer,
            reader,
        }
    }

    fn pull(&self, name: &str, key: &str, space: &str) -> (PathBuf, PathBuf, HostLink) {
        let member = team::authenticate(&self.team, key).unwrap().unwrap().member;
        let hosted = team::hosted_spaces(&self.settings).unwrap();
        let me = member.me(&hosted);
        let grant = me.spaces.iter().find(|entry| entry.space == space).unwrap();
        let server = team::hosted_folder(&self.settings, space).unwrap().unwrap();
        let list = TeamFileList {
            access: grant.access,
            files: team::list_folder_files(&server),
            rules: rulesync_team::hosted_manifest(&self.team, &self.settings, space).unwrap(),
        };
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
        let approved = fs::read(team::served_path(&server, &manifest.file).unwrap()).unwrap();
        let layer = rulesync_team::pulled_layer(&host, &list, &approved).unwrap();
        remember_team_rule_pull(&settings, &scope, &host, layer).unwrap();
        (settings, folder, host)
    }

    fn send_reader_proposal(&self, settings: &Path, mirror: &Path, markdown: &str) -> String {
        let layer = rulesync_team::load_project_layers(settings)
            .unwrap()
            .remove(0);
        let sha =
            rulesync_team::queue_proposal(settings, &layer.id, &layer.revision, markdown).unwrap();
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

#[test]
fn rulesync_team_uses_space_grants_and_retains_only_approved_offline_layers() {
    let fixture = Fixture::new();
    let reader = team::authenticate(&fixture.team, &fixture.reader.1)
        .unwrap()
        .unwrap()
        .member;
    assert_eq!(reader.access(OTHER), None);
    let (settings, _, _) = fixture.pull("reader", &fixture.reader.1, rulesync_team::SPACE);
    let (_, mirror, host) = fixture.pull("reader", &fixture.reader.1, PROJECT);
    let (writer, _, _) = fixture.pull("writer", &fixture.writer.1, PROJECT);
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
    fixture.pull("reader", &fixture.reader.1, PROJECT);
    fixture.pull("writer", &fixture.writer.1, PROJECT);
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

#[test]
fn rulesync_team_rejects_credentials_before_shipping_a_proposal() {
    let fixture = Fixture::new();
    let (settings, _, _) = fixture.pull("reader", &fixture.reader.1, PROJECT);
    let layer = rulesync_team::load_project_layers(&settings)
        .unwrap()
        .remove(0);
    let credential = team::new_key();
    let error = rulesync_team::queue_proposal(&settings, &layer.id, &layer.revision, &credential)
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
