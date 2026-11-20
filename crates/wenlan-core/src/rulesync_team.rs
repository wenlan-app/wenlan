// SPDX-License-Identifier: Apache-2.0

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wenlan_types::team::{
    TeamAccess, TeamFileList, TeamInfo, TeamProjectSkill, TeamRole, TeamRulesManifest,
    TeamRulesProject, TeamRulesProposal,
};

use crate::error::WenlanError;
use crate::repo_wiki;
use crate::rulesync_personal::reject_secret_lines;
use crate::shared_space::{self, remote, version};
use crate::skill_registry::{self, project_sync};
use crate::store::format::page::PageFile;
use crate::team::{self, CredentialKind, Member, TeamFile};

pub const SPACE: &str = "everyone";
const TITLE: &str = "Rules";
const ID_PREFIX: &str = "team-rules-";
const ACCEPTS: &str = "accepts_team_proposal";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRuleLayer {
    pub id: String,
    pub name: String,
    pub markdown: String,
    pub revision: String,
    pub synced_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<TeamRulesProject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<TeamProjectSkill>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("the active team grant or person session does not allow this Rules action")]
    Forbidden,
    #[error("this Space has no hosted Rules master")]
    NotConfigured,
    #[error(transparent)]
    Failed(#[from] WenlanError),
}

pub fn load_layers(settings_path: &Path) -> Result<Vec<TeamRuleLayer>, WenlanError> {
    load_scope_layers(settings_path, false)
}

pub fn load_project_layers(settings_path: &Path) -> Result<Vec<TeamRuleLayer>, WenlanError> {
    load_scope_layers(settings_path, true)
}

pub fn hosted_project_layer(
    settings_path: &Path,
    space: &str,
) -> Result<Option<TeamRuleLayer>, WenlanError> {
    let Some(hosted) = hosted_rules(&team_path(settings_path), settings_path, space)? else {
        return Ok(None);
    };
    if hosted.project.is_none() {
        return Ok(None);
    }
    let Some(rules) = hosted.head()? else {
        return Ok(None);
    };
    if rules.meta.parent.is_none() && rules.page.body.is_empty() {
        // The seed lets a Write holder review the first proposal without
        // clearing existing repo rules before that proposal is approved.
        return Ok(None);
    }
    hosted.layer(settings_path, &rules).map(Some)
}

/// Reads the project master for this exact local Space, using its current team grant.
pub fn project_layer_for_local_space(
    settings_path: &Path,
    local_space: &str,
) -> Result<Option<TeamRuleLayer>, WenlanError> {
    if let Some(layer) =
        shared_space::cached_project_rule_layer_for_space(settings_path, local_space)?
    {
        check_markdown(&layer.markdown)?;
        check_skills(layer.project.is_some(), &layer.skills)?;
        return Ok(Some(layer));
    }
    hosted_project_layer(settings_path, local_space)
}

fn load_scope_layers(
    settings_path: &Path,
    projects: bool,
) -> Result<Vec<TeamRuleLayer>, WenlanError> {
    let mut layers = std::collections::BTreeMap::<String, TeamRuleLayer>::new();
    for layer in shared_space::cached_team_rule_layers(settings_path)?
        .into_iter()
        .filter(|layer| layer.project.is_some() == projects)
    {
        check_markdown(&layer.markdown)?;
        check_markdown(&layer.name)?;
        check_skills(layer.project.is_some(), &layer.skills)?;
        if layers
            .get(&layer.id)
            .is_none_or(|cached| cached.synced_at < layer.synced_at)
        {
            layers.insert(layer.id.clone(), layer);
        }
    }
    let file = team::read_team(&team_path(settings_path))?;
    let spaces = if projects {
        team::hosted_spaces(settings_path)?
            .into_iter()
            .map(|(space, _)| space)
            .filter(|space| space != SPACE)
            .collect()
    } else {
        vec![SPACE.to_string()]
    };
    for space in spaces {
        if let Some(hosted) = hosted_rules_in(&file, settings_path, &space)? {
            let Some(rules) = hosted.head()? else {
                continue;
            };
            let local = hosted.layer(settings_path, &rules)?;
            layers.insert(local.id.clone(), local);
        }
    }
    let mut layers: Vec<_> = layers.into_values().collect();
    layers.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    Ok(layers)
}

pub(crate) fn team_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(team::TEAM_FILE)
}

pub(crate) fn hosted_owner(settings_path: &Path) -> Result<Option<String>, WenlanError> {
    let file = team::read_team(&team_path(settings_path))?;
    Ok(file
        .members
        .into_iter()
        .find(|member| member.role == TeamRole::Owner && member.is_active())
        .map(|member| member.id))
}

fn page_id(team_id: &str, project: Option<&TeamRulesProject>) -> String {
    let scope = project.map_or_else(
        || team_id.to_string(),
        |project| format!("{team_id}\n{}", project.space_id),
    );
    format!("{ID_PREFIX}{}", version::sha256_hex(scope.as_bytes()))
}

/// The stable origin identifier can cross the team boundary without a URL or credentials.
pub fn repo_identity(remote: &str) -> Option<String> {
    if let Some(slug) = repo_wiki::github_slug(remote) {
        return Some(version::sha256_hex(
            format!("github.com/{}", slug.to_ascii_lowercase()).as_bytes(),
        ));
    }
    let id = if Path::new(remote).is_absolute() {
        remote.to_string()
    } else if let Ok(url) = reqwest::Url::parse(remote) {
        if url.scheme() == "file" {
            url.to_file_path().ok()?.to_string_lossy().into_owned()
        } else {
            let host = url.host_str()?;
            let authority = url
                .port()
                .map_or_else(|| host.to_string(), |port| format!("{host}:{port}"));
            let path = url.path().trim_end_matches('/').trim_end_matches(".git");
            if host == "github.com" {
                format!("{authority}{}", path.to_ascii_lowercase())
            } else {
                format!("{authority}{path}")
            }
        }
    } else {
        let (host, path) = remote.split_once(':')?;
        let host = host.rsplit('@').next()?;
        if host.is_empty() || path.is_empty() {
            return None;
        }
        format!(
            "{}/{}",
            host.to_ascii_lowercase(),
            path.trim_start_matches('/')
                .trim_end_matches('/')
                .trim_end_matches(".git")
        )
    };
    Some(version::sha256_hex(id.as_bytes()))
}

pub fn is_master_page(id: &str) -> bool {
    id.strip_prefix(ID_PREFIX).is_some_and(is_sha)
}

fn is_sha(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validation(message: impl Into<String>) -> WenlanError {
    WenlanError::Validation(message.into())
}

fn check_markdown(markdown: &str) -> Result<(), WenlanError> {
    reject_secret_lines(markdown).map_err(|error| validation(error.to_string()))
}

fn new_page(id: &str, markdown: &str) -> PageFile {
    let mut page = PageFile::new(TITLE, markdown);
    page.id = Some(id.to_string());
    page
}

fn metadata(
    markdown: &str,
    base: Option<String>,
    member: &str,
    kind: version::AuthorKind,
    ts: &str,
) -> version::VersionMeta {
    version::VersionMeta {
        parent: base,
        merged: Vec::new(),
        body_digest: version::body_digest(markdown),
        author: version::Author {
            member: member.to_string(),
            kind,
            agent: None,
        },
        ts: ts.to_string(),
        archived: false,
        locked: true,
        proposal: false,
        decides: None,
        device: None,
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

struct HostedRules {
    info: TeamInfo,
    page_id: String,
    folder: PathBuf,
    project: Option<TeamRulesProject>,
}

fn hosted_rules(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
) -> Result<Option<HostedRules>, WenlanError> {
    hosted_rules_in(&team::read_team(team_path)?, settings_path, space)
}

fn hosted_rules_in(
    file: &TeamFile,
    settings_path: &Path,
    space: &str,
) -> Result<Option<HostedRules>, WenlanError> {
    let Some(info) = file.info() else {
        return Ok(None);
    };
    let Some(folder) = team::hosted_folder(settings_path, space)? else {
        return Ok(None);
    };
    let project = if space == SPACE {
        if !file.everyone.contains_key(space) {
            return Ok(None);
        }
        None
    } else {
        let Some(binding) =
            repo_wiki::binding_at(&settings_path.with_file_name("repo-wiki.json"), space)?
        else {
            return Ok(None);
        };
        let Some(space_id) = team::hosted_space_id(&folder) else {
            return Ok(None);
        };
        let repo_id = binding.remote_url.as_deref().and_then(repo_identity);
        check_markdown(space)?;
        Some(TeamRulesProject {
            space: space.to_string(),
            space_id,
            repo_id,
        })
    };
    if !version::is_safe_id(&info.id) {
        return Ok(None);
    }
    Ok(Some(HostedRules {
        page_id: page_id(&info.id, project.as_ref()),
        info,
        folder,
        project,
    }))
}

impl HostedRules {
    fn history(&self) -> version::FolderRead {
        version::read_page(&self.folder, &self.page_id)
    }

    fn head(&self) -> Result<Option<version::PageVersion>, WenlanError> {
        Ok(approved_head(&self.history(), &self.page_id)?.cloned())
    }

    fn file_path(&self, rules: &version::PageVersion) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            version::MEMBERS_DIR,
            rules.meta.author.member,
            version::PAGES_DIR,
            self.page_id,
            version::version_file_name(&rules.sha)
        )
    }

    fn skills(&self, settings_path: &Path) -> Result<Vec<TeamProjectSkill>, WenlanError> {
        let Some(project) = &self.project else {
            return Ok(Vec::new());
        };
        let registry = if settings_path == shared_space::settings_path() {
            skill_registry::default_wenlan_root()
        } else {
            settings_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        };
        project_sync::project_sources(&registry, &project.space).map_err(|_| {
            WenlanError::Generic("the project skill registry could not be read".into())
        })
    }

    fn layer(
        &self,
        settings_path: &Path,
        rules: &version::PageVersion,
    ) -> Result<TeamRuleLayer, WenlanError> {
        layer(
            &self.info,
            self.project.clone(),
            rules,
            rules.meta.ts.clone(),
            self.skills(settings_path)?,
        )
    }

    fn lock(&self) -> Result<File, WenlanError> {
        fs::create_dir_all(&self.folder)?;
        let file = open_lock(&self.folder.join("rules.lock"))?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(file)
    }
}

fn open_lock(path: &Path) -> Result<File, WenlanError> {
    Ok(fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}

fn authority_lock(team_path: &Path) -> Result<File, WenlanError> {
    let file = open_lock(&team_path.with_extension("lock"))?;
    fs2::FileExt::lock_shared(&file)?;
    Ok(file)
}

fn approved_head<'a>(
    history: &'a version::FolderRead,
    id: &str,
) -> Result<Option<&'a version::PageVersion>, WenlanError> {
    let Some(head) = history
        .pages
        .get(id)
        .and_then(|versions| version::head(versions))
    else {
        return Ok(None);
    };
    if head.leaves.len() != 1 {
        return Err(WenlanError::Conflict(
            "the hosted Rules page has competing versions".into(),
        ));
    }
    check_approved(head.winner, id)?;
    Ok(Some(head.winner))
}

fn check_approved(rules: &version::PageVersion, id: &str) -> Result<(), WenlanError> {
    if rules.page.id.as_deref() != Some(id)
        || rules.page.title != TITLE
        || rules.meta.author.kind != version::AuthorKind::Person
        || !rules.meta.locked
        || rules.meta.archived
        || version::file_kind(&rules.meta) != version::PAGES_DIR
    {
        return Err(validation("invalid approved team Rules version"));
    }
    check_markdown(&rules.page.body)
}

/// Creates the empty locked master when an eligible hosted Space first syncs.
pub(crate) fn ensure_hosted_master(settings_path: &Path, space: &str) -> Result<(), WenlanError> {
    let team_path = team_path(settings_path);
    if !team_path.is_file() {
        return Ok(());
    }
    let _authority = authority_lock(&team_path)?;
    let file = team::read_team(&team_path)?;
    let Some(hosted) = hosted_rules_in(&file, settings_path, space)? else {
        return Ok(());
    };
    let Some(owner) = file
        .members
        .iter()
        .find(|member| member.role == TeamRole::Owner && member.is_active())
    else {
        return Ok(());
    };
    let _lock = hosted.lock()?;
    if hosted.head()?.is_none() {
        let page = new_page(&hosted.page_id, "");
        let meta = metadata("", None, &owner.id, version::AuthorKind::Person, &now());
        version::write_version(
            &hosted.folder,
            &owner.id,
            &hosted.page_id,
            &version::render(&page, &meta),
        )?;
    }
    Ok(())
}

/// Names the approved version in the existing authenticated shared Space file list.
pub fn hosted_manifest(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
) -> Result<Option<TeamRulesManifest>, WenlanError> {
    let Some(hosted) = hosted_rules(team_path, settings_path, space)? else {
        return Ok(None);
    };
    let Some(rules) = hosted.head()? else {
        return Ok(None);
    };
    Ok(Some(TeamRulesManifest {
        skills: hosted.skills(settings_path)?,
        file: hosted.file_path(&rules),
        team: hosted.info,
        page_id: hosted.page_id,
        project: hosted.project,
    }))
}

/// Lists the shared page versions and the approved Rules version for a Space pull.
pub fn hosted_file_list(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
    folder: &Path,
    access: TeamAccess,
    pages: &team::SharedFolderPages,
) -> Result<TeamFileList, WenlanError> {
    let rules = hosted_manifest(team_path, settings_path, space)?;
    let mut files = team::list_folder_files(folder, pages);
    // Space pulls require the manifest's approved path in the file list.
    if let Some(rules) = rules.as_ref().filter(|rules| !files.contains(&rules.file)) {
        files.push(rules.file.clone());
    }
    Ok(TeamFileList {
        access,
        files,
        rules,
    })
}

/// Reads the Space's approved Rules version named by its file manifest.
pub fn read_hosted_rule_file(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, WenlanError> {
    let Some(hosted) = hosted_rules(team_path, settings_path, space)? else {
        return Ok(None);
    };
    let Some(rules) = hosted
        .head()?
        .filter(|rules| hosted.file_path(rules) == path)
    else {
        return Ok(None);
    };
    let bytes = fs::read(hosted.folder.join(path))?;
    // The content must still match the version validated by `head`.
    Ok((version::sha256_hex(&bytes) == rules.sha).then_some(bytes))
}

fn layer(
    info: &TeamInfo,
    project: Option<TeamRulesProject>,
    rules: &version::PageVersion,
    synced_at: String,
    skills: Vec<TeamProjectSkill>,
) -> Result<TeamRuleLayer, WenlanError> {
    let name = info.name.as_deref().unwrap_or("Team");
    check_markdown(name)?;
    Ok(TeamRuleLayer {
        id: if project.is_some() {
            page_id(&info.id, project.as_ref())
        } else {
            info.id.clone()
        },
        name: name.to_string(),
        markdown: rules.page.body.clone(),
        revision: rules.sha.clone(),
        synced_at,
        project,
        skills,
    })
}

fn check_skills(project: bool, skills: &[TeamProjectSkill]) -> Result<(), WenlanError> {
    if !project && !skills.is_empty() {
        return Err(validation("project skills need a project Rules scope"));
    }
    project_sync::validate(skills).map_err(|error| validation(error.to_string()))
}

/// Reads only the approved version named by the authenticated pull response.
pub(crate) fn pulled_layer(
    link: &shared_space::HostLink,
    list: &TeamFileList,
    bytes: &[u8],
) -> Result<Option<TeamRuleLayer>, WenlanError> {
    let Some(manifest) = &list.rules else {
        return Ok(None);
    };
    if manifest.page_id != page_id(&manifest.team.id, manifest.project.as_ref())
        || !list.files.contains(&manifest.file)
    {
        return Err(validation("invalid team Rules manifest"));
    }
    match &manifest.project {
        None if link.space == SPACE => {}
        Some(project)
            if project.space == link.space
                && link.space != SPACE
                && version::is_safe_id(&project.space_id)
                && project.repo_id.as_deref().is_none_or(is_sha)
                && link
                    .space_id
                    .as_ref()
                    .is_none_or(|id| id == &project.space_id) =>
        {
            check_markdown(&project.space)?
        }
        _ => {
            return Err(validation(
                "the pulled Rules scope does not match this Space",
            ))
        }
    }
    let mut parts = manifest.file.split('/');
    let (
        Some(version::MEMBERS_DIR),
        Some(member),
        Some(version::PAGES_DIR),
        Some(id),
        Some(name),
        None,
    ) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    )
    else {
        return Err(validation("invalid team Rules version path"));
    };
    if !version::is_safe_id(member) {
        return Err(validation("invalid team Rules version path"));
    }
    let rules = version::parse(name, bytes)?;
    if id != manifest.page_id || rules.meta.author.member != member {
        return Err(validation(
            "the pulled Rules version does not match the manifest",
        ));
    }
    check_approved(&rules, id)?;
    check_skills(manifest.project.is_some(), &manifest.skills)?;
    layer(
        &manifest.team,
        manifest.project.clone(),
        &rules,
        now(),
        manifest.skills.clone(),
    )
    .map(Some)
}

/// Queue edits to the last approved layer for the next shared Space push.
pub fn queue_proposal(
    settings_path: &Path,
    layer_id: &str,
    revision: &str,
    markdown: &str,
) -> Result<String, WenlanError> {
    let file = team::read_team(&team_path(settings_path))?;
    for (space, _) in team::hosted_spaces(settings_path)? {
        if let Some(hosted) = hosted_rules_in(&file, settings_path, &space)?.filter(|hosted| {
            if hosted.project.is_some() {
                hosted.page_id == layer_id
            } else {
                hosted.info.id == layer_id
            }
        }) {
            let owner = hosted_owner(settings_path)?
                .ok_or_else(|| WenlanError::NotFound("the hosted team owner".into()))?;
            return write_proposal(
                &hosted.folder,
                &hosted.page_id,
                &owner,
                version::AuthorKind::Ai,
                revision,
                markdown,
                &now(),
            );
        }
    }
    let (host, share) = shared_space::team_rule_proposal_target(settings_path, layer_id, revision)?;
    let id = if host.space == SPACE {
        page_id(layer_id, None)
    } else {
        layer_id.to_string()
    };
    let folder = shared_space::folder_for(&shared_space::Scope::Space(host.space), &share.folder);
    write_proposal(
        &folder,
        &id,
        &share.member_id,
        version::AuthorKind::Ai,
        revision,
        markdown,
        &now(),
    )
}

pub(crate) fn write_proposal(
    folder: &Path,
    id: &str,
    member: &str,
    kind: version::AuthorKind,
    revision: &str,
    markdown: &str,
    timestamp: &str,
) -> Result<String, WenlanError> {
    let markdown = markdown.replace("\r\n", "\n").replace('\r', "\n");
    check_markdown(&markdown)?;
    let mut meta = metadata(
        &markdown,
        Some(revision.to_string()),
        member,
        kind,
        timestamp,
    );
    meta.proposal = true;
    let text = version::render(&new_page(id, &markdown), &meta);
    let name = version::version_file_name(&version::sha256_hex(text.as_bytes()));
    let proposal = version::parse(&name, text.as_bytes())?;
    check_proposal(&proposal, id, member, text.as_bytes())?;
    if let Some(existing) = version::read_page(folder, id)
        .proposals
        .get(id)
        .into_iter()
        .flatten()
        .find(|old| {
            old.meta.author == proposal.meta.author
                && old.meta.parent == proposal.meta.parent
                && old.meta.body_digest == proposal.meta.body_digest
        })
    {
        return Ok(existing.sha.clone());
    }
    version::write_file(folder, member, version::PROPOSALS_DIR, id, &text).map(|(sha, _)| sha)
}

fn check_proposal(
    proposal: &version::PageVersion,
    id: &str,
    member: &str,
    bytes: &[u8],
) -> Result<(), WenlanError> {
    let meta = &proposal.meta;
    let body = &proposal.page.body;
    check_markdown(body)?;
    if bytes.len() > remote::MAX_VERSION_BYTES
        || meta.author.member != member
        || meta.author.agent.is_some()
        || !meta.parent.as_deref().is_some_and(is_sha)
        || chrono::DateTime::parse_from_rfc3339(&meta.ts).is_err()
        || !meta.merged.is_empty()
        || meta.archived
        || !meta.locked
        || !meta.proposal
        || meta.decides.is_some()
        || meta.device.is_some()
        || version::render(&new_page(id, body), meta).as_bytes() != bytes
    {
        return Err(validation("invalid team Rules proposal"));
    }
    Ok(())
}

fn active_member(file: &TeamFile, member: &str, space: &str) -> Result<Member, ReviewError> {
    file.members
        .iter()
        .find(|entry| entry.id == member && entry.is_active() && entry.access(space).is_some())
        .cloned()
        .ok_or(ReviewError::Forbidden)
}

/// Receive a member's proposal through the same endpoint as ordinary shared versions.
/// A Read grant can submit this kind of file alone. Only the authority writes approved versions.
pub fn receive_proposal(
    team_path: &Path,
    settings_path: &Path,
    member: &str,
    space: &str,
    id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<bool, ReviewError> {
    let _authority = authority_lock(team_path)?;
    let file = team::read_team(team_path)?;
    let hosted = hosted_rules_in(&file, settings_path, space)?
        .filter(|hosted| hosted.page_id == id)
        .ok_or(ReviewError::NotConfigured)?;
    active_member(&file, member, space)?;
    let proposal = version::parse(name, bytes)?;
    check_proposal(&proposal, id, member, bytes)?;
    let _lock = hosted.lock()?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| validation("the team Rules proposal is not text"))?;
    Ok(version::write_file(&hosted.folder, member, version::PROPOSALS_DIR, id, text)?.1)
}

pub(crate) fn approved_proposal(version: &version::PageVersion) -> Option<&str> {
    version
        .page
        .other
        .get(ACCEPTS)?
        .as_str()
        .filter(|sha| is_sha(sha))
}

fn already_decided(history: &version::FolderRead, page: &str, proposal: &str) -> bool {
    history
        .decisions
        .get(page)
        .into_iter()
        .flatten()
        .any(|decision| {
            decision
                .meta
                .decides
                .as_ref()
                .is_some_and(|decision| decision.proposal == proposal)
        })
        || history
            .pages
            .get(page)
            .into_iter()
            .flatten()
            .any(|version| approved_proposal(version) == Some(proposal))
}

fn proposals(hosted: &HostedRules) -> Vec<TeamRulesProposal> {
    let history = hosted.history();
    let mut pending: Vec<_> = history
        .proposals
        .get(&hosted.page_id)
        .into_iter()
        .flatten()
        .filter(|proposal| !already_decided(&history, &hosted.page_id, &proposal.sha))
        .filter_map(|proposal| {
            check_markdown(&proposal.page.body).ok()?;
            Some(TeamRulesProposal {
                id: proposal.sha.clone(),
                author: proposal.meta.author.member.clone(),
                markdown: proposal.page.body.clone(),
                base: proposal.meta.parent.clone()?,
                proposed_at: proposal.meta.ts.clone(),
            })
        })
        .collect();
    pending.sort_by(|a, b| (&a.proposed_at, &a.id).cmp(&(&b.proposed_at, &b.id)));
    pending
}

fn review_authority(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
    person: Option<(&str, CredentialKind)>,
) -> Result<(HostedRules, Member), ReviewError> {
    let file = team::read_team(team_path)?;
    let member = match person {
        Some((member, CredentialKind::BrowserSession)) => active_member(&file, member, space)?,
        Some(_) => return Err(ReviewError::Forbidden),
        None => file
            .members
            .iter()
            .find(|member| member.role == TeamRole::Owner && member.is_active())
            .cloned()
            .ok_or(ReviewError::Forbidden)?,
    };
    if member.access(space) != Some(TeamAccess::Write) {
        return Err(ReviewError::Forbidden);
    }
    let hosted = hosted_rules_in(&file, settings_path, space)?.ok_or(ReviewError::NotConfigured)?;
    Ok((hosted, member))
}

/// Lists proposals for a person signed in to the team listener.
pub fn pending_proposals(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
    member: &str,
    credential: CredentialKind,
) -> Result<Vec<TeamRulesProposal>, ReviewError> {
    let _authority = authority_lock(team_path)?;
    let (hosted, _) =
        review_authority(team_path, settings_path, space, Some((member, credential)))?;
    Ok(proposals(&hosted))
}

/// Decides through the team listener. A legacy bearer is also usable by AI tools.
pub fn decide_proposal(
    team_path: &Path,
    settings_path: &Path,
    space: &str,
    member: &str,
    credential: CredentialKind,
    proposal: &str,
    accept: bool,
) -> Result<(), ReviewError> {
    let _authority = authority_lock(team_path)?;
    let (hosted, member) =
        review_authority(team_path, settings_path, space, Some((member, credential)))?;
    decide(&hosted, &member.id, proposal, accept)
}

/// Call only after authenticating the person on the team host's loopback daemon.
pub fn local_proposals(settings_path: &Path) -> Result<Vec<TeamRulesProposal>, ReviewError> {
    local_space_proposals(settings_path, SPACE)
}

/// Call only after authenticating the person on the team host's loopback daemon.
pub fn local_space_proposals(
    settings_path: &Path,
    space: &str,
) -> Result<Vec<TeamRulesProposal>, ReviewError> {
    let team_path = team_path(settings_path);
    let _authority = authority_lock(&team_path)?;
    let (hosted, _) = review_authority(&team_path, settings_path, space, None)?;
    Ok(proposals(&hosted))
}

/// Call only after authenticating the person on the team host's loopback daemon.
pub fn update_hosted_rules(
    settings_path: &Path,
    space: &str,
    markdown: &str,
) -> Result<String, ReviewError> {
    check_markdown(markdown)?;
    ensure_hosted_master(settings_path, space)?;
    let team_path = team_path(settings_path);
    let _authority = authority_lock(&team_path)?;
    let (hosted, owner) = review_authority(&team_path, settings_path, space, None)?;
    let _lock = hosted.lock()?;
    let head = hosted.head()?.ok_or(ReviewError::NotConfigured)?;
    if head.page.body == markdown {
        return Err(validation("the proposed Rules are already approved").into());
    }
    write_proposal(
        &hosted.folder,
        &hosted.page_id,
        &owner.id,
        version::AuthorKind::Person,
        &head.sha,
        markdown,
        &now(),
    )
    .map_err(Into::into)
}

/// Call only after authenticating the person on the team host's loopback daemon.
pub fn decide_local_proposal(
    settings_path: &Path,
    proposal: &str,
    accept: bool,
) -> Result<(), ReviewError> {
    decide_local_space_proposal(settings_path, SPACE, proposal, accept)
}

/// Call only after authenticating the person on the team host's loopback daemon.
pub fn decide_local_space_proposal(
    settings_path: &Path,
    space: &str,
    proposal: &str,
    accept: bool,
) -> Result<(), ReviewError> {
    let team_path = team_path(settings_path);
    let _authority = authority_lock(&team_path)?;
    let (hosted, owner) = review_authority(&team_path, settings_path, space, None)?;
    decide(&hosted, &owner.id, proposal, accept)
}

fn decide(hosted: &HostedRules, member: &str, id: &str, accept: bool) -> Result<(), ReviewError> {
    let _lock = hosted.lock()?;
    let history = hosted.history();
    let page_id = &hosted.page_id;
    if already_decided(&history, page_id, id) {
        return Err(
            WenlanError::Conflict("this team Rules proposal was already decided".into()).into(),
        );
    }
    let proposal = history
        .proposals
        .get(page_id)
        .into_iter()
        .flatten()
        .find(|proposal| proposal.sha == id)
        .ok_or_else(|| WenlanError::NotFound("team Rules proposal".into()))?;
    let head = approved_head(&history, page_id)?.ok_or(ReviewError::NotConfigured)?;
    let (mut page, mut meta) = if accept {
        if proposal.meta.parent.as_deref() != Some(head.sha.as_str()) {
            return Err(WenlanError::Conflict(
                "the approved Rules changed; review the change against the new version".into(),
            )
            .into());
        }
        check_markdown(&proposal.page.body)?;
        (
            new_page(page_id, &proposal.page.body),
            metadata(
                &proposal.page.body,
                Some(head.sha.clone()),
                member,
                version::AuthorKind::Person,
                &now(),
            ),
        )
    } else {
        (
            new_page(page_id, ""),
            metadata("", None, member, version::AuthorKind::Person, &now()),
        )
    };
    if accept {
        page.other.insert(
            serde_yaml::Value::String(ACCEPTS.into()),
            serde_yaml::Value::String(id.to_string()),
        );
    } else {
        meta.decides = Some(version::Decides {
            proposal: id.to_string(),
            accepted: false,
        });
    }
    let text = version::render(&page, &meta);
    version::write_file(
        &hosted.folder,
        member,
        version::file_kind(&meta),
        page_id,
        &text,
    )?;
    Ok(())
}
