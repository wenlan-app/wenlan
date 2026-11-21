// SPDX-License-Identifier: Apache-2.0
//! The wire projections for pages that leave a team host. Changes to the
//! local page, changelog, and Markdown frontmatter do not extend these views.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
pub use wenlan_types::member_view::{
    MemberBasis, MemberLinkedPage, MemberPage, MemberPageEntry, MemberPageRead, MemberPages,
};
use wenlan_types::responses::{
    CreatePageResponse, ManualPageWriteResponse, PageChangelogEntry, PageHistoryItem,
    PageHistoryKind, PageProjectionStatus, PageRemovalResponse, PageRemovalStatus,
    PageWriteOutcome, PageWriteResponse, RestorePageResponse,
};
use wenlan_types::{
    ActivityBadge, ActivityKind, BasisRef, LinkedChanged, Page, PageChange, PageChangeKind,
};

use crate::shared_space::{rewrite, version};
use crate::store::format::page::PageFile;

#[derive(Deserialize, Serialize)]
pub struct MemberHealth {
    pub status: String,
    pub db_initialized: bool,
    pub version: String,
}

#[derive(Deserialize, Serialize)]
pub struct MemberSpace {
    id: String,
    name: String,
    sort_order: i64,
    memory_count: u64,
    entity_count: u64,
}

impl From<wenlan_types::Space> for MemberSpace {
    fn from(space: wenlan_types::Space) -> Self {
        Self {
            id: space.id,
            name: space.name,
            sort_order: space.sort_order,
            memory_count: 0,
            entity_count: 0,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
pub struct MemberGraph {
    #[serde(skip_deserializing)]
    entities: [(); 0],
    #[serde(skip_deserializing)]
    relations: [(); 0],
    #[serde(skip_deserializing)]
    memories: [(); 0],
    #[serde(skip_deserializing)]
    memory_links: [(); 0],
    pages: Vec<MemberGraphPage>,
    page_links: Vec<MemberGraphLink>,
}

#[derive(Deserialize, Serialize)]
struct MemberGraphPage {
    id: String,
    title: String,
    space: Option<String>,
    creation_kind: String,
    last_modified: String,
    #[serde(skip_deserializing)]
    entity_id: Option<()>,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct MemberGraphLink {
    from: MemberGraphEndpoint,
    to: MemberGraphEndpoint,
    link_type: String,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct MemberGraphEndpoint {
    kind: String,
    id: String,
}

impl From<wenlan_types::KnowledgeGraphResponse> for MemberGraph {
    fn from(graph: wenlan_types::KnowledgeGraphResponse) -> Self {
        let endpoint = |endpoint: wenlan_types::GraphRef| MemberGraphEndpoint {
            kind: endpoint.kind,
            id: endpoint.id,
        };
        Self {
            pages: graph
                .pages
                .into_iter()
                .map(|page| MemberGraphPage {
                    id: page.id,
                    title: page.title,
                    space: page.space,
                    creation_kind: page.creation_kind,
                    last_modified: page.last_modified,
                    entity_id: None,
                })
                .collect(),
            page_links: graph
                .page_links
                .into_iter()
                .filter(|link| link.from.kind == "page" && link.to.kind == "page")
                .map(|link| MemberGraphLink {
                    from: endpoint(link.from),
                    to: endpoint(link.to),
                    link_type: link.link_type,
                })
                .collect(),
            ..Self::default()
        }
    }
}

impl MemberGraph {
    pub fn merge(graphs: impl Iterator<Item = Self>, has_space: impl Fn(&str) -> bool) -> Self {
        let mut merged = Self::default();
        let mut visible = HashSet::new();
        for graph in graphs {
            merged.pages.extend(graph.pages.into_iter().filter(|page| {
                page.space.as_deref().is_some_and(&has_space) && visible.insert(page.id.clone())
            }));
            for link in graph.page_links {
                if !merged.page_links.contains(&link) {
                    merged.page_links.push(link);
                }
            }
        }
        let visible = |end: &MemberGraphEndpoint| end.kind == "page" && visible.contains(&end.id);
        merged
            .page_links
            .retain(|link| visible(&link.from) && visible(&link.to));
        merged
    }
}

#[derive(Serialize)]
pub struct MemberRules {
    space_id: String,
    space_name: String,
    lines: Vec<MemberRule>,
}

#[derive(Serialize)]
struct MemberRule {
    id: String,
    kind: wenlan_types::SkillLineKind,
    text: String,
    source_page_id: &'static str,
    source_page_title: &'static str,
    source_revision: i64,
    source_revision_at: i64,
    source_quote: &'static str,
    person_edited: bool,
    general: bool,
    hand_written: bool,
}

impl MemberRules {
    pub fn from_approved(space: &wenlan_types::Space, lines: Vec<wenlan_types::SkillLine>) -> Self {
        Self {
            space_id: space.id.clone(),
            space_name: space.name.clone(),
            lines: lines
                .into_iter()
                .map(|line| MemberRule {
                    id: line.id,
                    kind: wenlan_types::SkillLineKind::Rule,
                    text: line.text,
                    source_page_id: "",
                    source_page_title: "Shared Rules",
                    source_revision: 0,
                    source_revision_at: 0,
                    source_quote: "",
                    person_edited: false,
                    general: false,
                    hand_written: false,
                })
                .collect(),
        }
    }
}

pub fn page(page: Page) -> MemberPage {
    MemberPage {
        id: page.id,
        title: page.title,
        content: published_body(&page.content, &page.citations),
        space: page.space,
        version: page.version,
        status: page.status,
        last_modified: page.last_modified,
        review_required: page.review_required,
        review_status: page.review_status,
        creation_kind: page.creation_kind,
        relevance_score: page.relevance_score,
        truth: page.truth,
        source_memory_ids: [],
        sources_updated_count: 0,
        created_at: String::new(),
        last_compiled: String::new(),
        user_edited: false,
    }
}

pub fn inventory(page: MemberPage, basis: Vec<BasisRef>) -> MemberPageEntry {
    MemberPageEntry {
        tags: crate::page_markdown::page_tags(&page.content),
        page,
        basis: basis.into_iter().map(member_basis).collect(),
        space_path: None,
    }
}

pub fn read(
    page: MemberPage,
    basis: Vec<BasisRef>,
    linked_changed: Vec<LinkedChanged>,
) -> MemberPageRead {
    let body = crate::page_body::body_parts(&page.content);
    MemberPageRead {
        page: inventory(page, Vec::new()),
        basis: basis.into_iter().map(member_basis).collect(),
        linked_changed: linked_changed.into_iter().map(member_linked_page).collect(),
        body,
    }
}

pub fn draft(page: Page) -> MemberDraft {
    MemberDraft {
        page: self::page(page),
        space_path: None,
        projection_status: None,
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberMove {
    pub space_path: String,
}

#[derive(Deserialize, Serialize)]
pub struct MemberCreate {
    id: String,
    attached_to: Option<String>,
    space: Option<String>,
    space_source: Option<wenlan_types::WriteSpaceSource>,
    write_outcome: Option<wenlan_types::WriteOutcome>,
    basis: Vec<MemberBasis>,
    next_step: String,
}

impl From<CreatePageResponse> for MemberCreate {
    fn from(saved: CreatePageResponse) -> Self {
        Self {
            id: saved.id,
            attached_to: saved.attached_to,
            space: saved.space,
            space_source: saved.space_source,
            write_outcome: saved.write_outcome,
            basis: saved.basis.into_iter().map(member_basis).collect(),
            next_step: saved.next_step,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberWrite {
    ok: bool,
    write_outcome: Option<PageWriteOutcome>,
    revision_card_id: Option<String>,
    gated: bool,
    basis: Vec<MemberBasis>,
    sentence_proposals: Vec<String>,
    next_step: String,
}

impl From<PageWriteResponse> for MemberWrite {
    fn from(saved: PageWriteResponse) -> Self {
        let next_step = if saved.edits_not_applied.is_empty() {
            saved.next_step
        } else {
            "The person's sentences were preserved. Read the latest page before another change."
                .into()
        };
        Self {
            ok: saved.ok,
            write_outcome: saved.write_outcome,
            revision_card_id: saved.revision_card_id,
            gated: saved.gated,
            basis: saved.basis.into_iter().map(member_basis).collect(),
            sentence_proposals: saved.sentence_proposals,
            next_step,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberManualWrite {
    ok: bool,
    projection_status: PageProjectionStatus,
    basis_confirm: Option<wenlan_types::BasisConfirmOutcome>,
}

impl From<ManualPageWriteResponse> for MemberManualWrite {
    fn from(saved: ManualPageWriteResponse) -> Self {
        Self {
            ok: saved.ok,
            projection_status: saved.projection_status,
            basis_confirm: saved.basis_confirm,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberRemoval {
    status: PageRemovalStatus,
    restorable: bool,
    version: Option<i64>,
    projection_status: Option<PageProjectionStatus>,
}

impl From<PageRemovalResponse> for MemberRemoval {
    fn from(removed: PageRemovalResponse) -> Self {
        Self {
            status: removed.status,
            restorable: removed.restorable,
            version: removed.version,
            projection_status: removed.projection_status,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberRestore {
    id: String,
    title: String,
    version: i64,
    status: String,
    projection_status: PageProjectionStatus,
    links_updated: u32,
    links_pending_review: u32,
    links_not_updated: u32,
}

impl From<RestorePageResponse> for MemberRestore {
    fn from(restored: RestorePageResponse) -> Self {
        Self {
            id: restored.id,
            title: restored.title,
            version: restored.version,
            status: restored.status,
            projection_status: restored.projection_status,
            links_updated: restored.links_updated,
            links_pending_review: restored.links_pending_review,
            links_not_updated: restored.links_not_updated,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub enum MemberPageDetail {
    Page(Box<MemberPageRead>),
    DraftSpace { page: MemberDraftSpace },
}

impl MemberPageDetail {
    pub fn draft_space(page: Page) -> Self {
        Self::DraftSpace {
            page: MemberDraftSpace {
                id: page.id,
                space: page.space,
            },
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberDraft {
    page: MemberPage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_status: Option<PageProjectionStatus>,
}

#[derive(Deserialize, Serialize)]
pub struct MemberDraftSpace {
    pub id: String,
    pub space: Option<String>,
}

pub fn member_basis(basis: BasisRef) -> MemberBasis {
    MemberBasis {
        page_id: basis.page_id,
        title: basis.title,
        state: basis.state,
        from_version: basis.from_version,
        to_version: basis.to_version,
        minor_only: basis.minor_only,
    }
}

fn member_linked_page(link: LinkedChanged) -> MemberLinkedPage {
    MemberLinkedPage {
        page_id: link.page_id,
        title: link.title,
        version: link.version,
    }
}

/// JSON page responses do not publish the citation records, so their body
/// must omit the corresponding markers too. Version files can carry shared
/// page citations and use the same rewrite with their published target set.
pub(crate) fn published_body(
    body: &str,
    _citations: &[wenlan_types::pages::PageCitation],
) -> String {
    crate::citations::strip_member_citation_markers(body)
}

pub fn search_hit(page: Page) -> wenlan_types::SearchResult {
    let trusted = crate::pages::page_is_trusted(&page);
    // This typed SearchResult envelope is also used for memory hits. Build a
    // page hit solely from the projected page every time.
    let member = self::page(page);
    let mut hit = wenlan_types::SearchResult {
        id: member.id.clone(),
        source_id: member.id,
        title: member.title,
        content: member.content,
        source: "page".into(),
        space: member.space,
        version: member.version,
        raw_score: member.relevance_score,
        confirmed: Some(trusted),
        ..wenlan_types::SearchResult::default()
    };
    hit.last_modified = chrono::DateTime::parse_from_rfc3339(&member.last_modified)
        .map(|at| at.timestamp())
        .unwrap_or_default();
    hit
}

/// Writer labels stored locally are open-ended. Publish only the product's
/// operation vocabulary and its authenticated shared-member attribution.
pub(crate) fn writer(label: &str) -> Option<String> {
    if matches!(
        label,
        "manual_edit"
            | "fs_edit"
            | "restore"
            | "person_create"
            | "publish"
            | "agent_create"
            | "agent_refresh"
            | "create"
            | "person_asked"
            | "space_move"
            | "rename"
            | "human"
            | "person"
            | "archive"
    ) || crate::shared_space::shared_writer(label).is_some_and(|(_, id)| version::is_safe_id(id))
    {
        return Some(label.to_owned());
    }
    if let Some((_, member)) = wenlan_types::team::split_team_agent(label) {
        if version::is_safe_id(member) {
            return Some(format!("shared_ai:{member}"));
        }
    }
    None
}

#[derive(Deserialize, Serialize)]
pub struct MemberRevisions {
    page_id: String,
    current_version: i64,
    user_edited: bool,
    entries: Vec<MemberRevision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<MemberHistory>,
}

#[derive(Deserialize, Serialize)]
struct MemberRevision {
    version: i64,
    at: i64,
    edited_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    concurrent_with: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_from: Option<i64>,
}

#[derive(Deserialize, Serialize)]
struct MemberHistory {
    kind: PageHistoryKind,
    at: i64,
    version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edited_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<MemberHistorySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_sentences: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_version: Option<i64>,
}

#[derive(Deserialize, Serialize)]
struct MemberHistorySource {
    page_id: String,
    title: String,
    version: i64,
    relied_on_version: i64,
    pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

impl MemberRevisions {
    /// The shared history builder supplies witnessed, citation-free text and
    /// discards events outside that history before folding or measuring diffs.
    pub fn new(
        page: &Page,
        entries: Vec<PageChangelogEntry>,
        history: Vec<PageHistoryItem>,
        witnessed: &HashSet<i64>,
    ) -> Self {
        let version = |number: &i64| witnessed.contains(number);
        Self {
            page_id: page.id.clone(),
            current_version: page.version,
            user_edited: history
                .iter()
                .any(|item| item.kind == PageHistoryKind::PersonEdit),
            entries: entries
                .into_iter()
                .map(|entry| MemberRevision {
                    version: entry.version,
                    at: entry.at,
                    edited_by: writer(&entry.edited_by).unwrap_or_else(|| "ai_edit".into()),
                    text: entry.text,
                    concurrent_with: entry.concurrent_with.filter(version),
                    restored_from: entry.restored_from.filter(version),
                })
                .collect(),
            history: history
                .into_iter()
                .map(|item| MemberHistory {
                    kind: item.kind,
                    at: item.at,
                    version: item.version,
                    edited_by: item.edited_by.as_deref().and_then(writer),
                    text: item.text,
                    source: item.source.map(|source| MemberHistorySource {
                        page_id: source.page_id,
                        title: source.title,
                        version: source.version,
                        relied_on_version: source.relied_on_version,
                        pending: source.pending,
                        text: source.text,
                    }),
                    changed_sentences: item.changed_sentences,
                    restored_from: item.restored_from.filter(version),
                    first_version: item.first_version.filter(version),
                })
                .collect(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberChange {
    pub page_id: String,
    title: String,
    change_kind: PageChangeKind,
    changed_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    edited_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_sentences: Option<u32>,
}

impl MemberChange {
    pub(crate) fn new(change: PageChange) -> Self {
        Self {
            page_id: change.page_id,
            title: change.title,
            change_kind: change.change_kind,
            changed_at_ms: change.changed_at_ms,
            edited_by: change.edited_by.as_deref().and_then(writer),
            changed_sentences: change.changed_sentences,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MemberActivity {
    kind: ActivityKind,
    pub id: String,
    title: String,
    timestamp_ms: u64,
    badge: ActivityBadge,
}

impl MemberActivity {
    pub fn new(activity: wenlan_types::RecentActivityItem) -> Self {
        Self {
            kind: ActivityKind::Page,
            id: activity.id,
            title: activity.title,
            timestamp_ms: activity.timestamp_ms,
            badge: activity.badge,
        }
    }
}

/// The shared version format is content-addressed. An old version can be
/// sent under its hash only when its bytes are exactly those of this view.
pub(crate) struct MemberSnapshot {
    page: PageFile,
    meta: version::VersionMeta,
}

impl MemberSnapshot {
    pub(crate) fn new(
        file: &PageFile,
        meta: &version::VersionMeta,
        space_pages: &HashMap<String, String>,
    ) -> Self {
        let ctx = rewrite::RewriteContext {
            space_pages,
            keep_all: false,
        };
        let content = rewrite::rewrite_body(&file.body, &file.citations, &ctx);
        let mut page = PageFile::new(&file.title, content);
        page.id = file.id.clone();
        (page.citations, page.basis) = rewrite::rewrite_sources(&file.citations, &file.basis, &ctx);
        Self {
            meta: version::VersionMeta {
                parent: meta.parent.clone().filter(|sha| is_digest(sha)),
                merged: meta
                    .merged
                    .iter()
                    .filter(|sha| is_digest(sha))
                    .cloned()
                    .collect(),
                body_digest: version::body_digest(&page.body),
                author: version::Author {
                    member: meta.author.member.clone(),
                    kind: meta.author.kind,
                    agent: None,
                },
                ts: meta.ts.clone(),
                archived: meta.archived,
                locked: meta.locked,
                proposal: meta.proposal,
                decides: meta
                    .decides
                    .clone()
                    .filter(|decision| is_digest(&decision.proposal)),
                device: None,
                stay: meta.stay.clone(),
            },
            page,
        }
    }

    pub(crate) fn render(&self) -> String {
        version::render(&self.page, &self.meta)
    }

    pub(crate) fn matches(
        read: &version::PageVersion,
        space_pages: &HashMap<String, String>,
    ) -> bool {
        version::is_safe_id(&read.meta.author.member)
            && version::ts_key(&read.meta.ts).is_some()
            && version::sha256_hex(
                Self::new(&read.page, &read.meta, space_pages)
                    .render()
                    .as_bytes(),
            ) == read.sha
    }

    pub(crate) fn matches_in_stay(
        read: &version::PageVersion,
        space_pages: &HashMap<String, String>,
        stay: &str,
    ) -> bool {
        read.meta.stay.as_deref() == Some(stay) && Self::matches(read, space_pages)
    }

    pub(crate) fn approved_rules_match(read: &version::PageVersion) -> bool {
        let mut view = Self::new(&read.page, &read.meta, &HashMap::new());
        if let Some(approval) = crate::rulesync_team::approved_proposal(read) {
            view.page
                .other
                .insert("accepts_team_proposal".into(), approval.into());
        }
        version::is_safe_id(&read.meta.author.member)
            && version::ts_key(&read.meta.ts).is_some()
            && version::sha256_hex(view.render().as_bytes()) == read.sha
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Content-addressed bytes validated for a hosted member download.
pub struct MemberVersionFile(Vec<u8>);

impl MemberVersionFile {
    pub fn shared(
        page: &str,
        name: &str,
        bytes: Vec<u8>,
        pages: &crate::team::SharedFolderPages,
    ) -> Option<Self> {
        crate::team::can_serve_version(page, name, &bytes, pages).then_some(Self(bytes))
    }

    pub fn approved_rules(
        team_path: &std::path::Path,
        settings_path: &std::path::Path,
        space: &str,
        path: &str,
    ) -> Result<Option<Self>, crate::WenlanError> {
        Ok(
            crate::rulesync_team::read_hosted_rule_file(team_path, settings_path, space, path)?
                .map(Self),
        )
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Serialize)]
pub struct MemberSpaceFile {
    format: u32,
    space: String,
    members: Vec<MemberSpacePerson>,
}

#[derive(Serialize)]
struct MemberSpacePerson {
    id: String,
    name: String,
}

impl From<crate::shared_space::SpaceFile> for MemberSpaceFile {
    fn from(space: crate::shared_space::SpaceFile) -> Self {
        Self {
            format: space.format,
            space: space.space,
            members: space
                .members
                .into_iter()
                .map(|person| MemberSpacePerson {
                    id: person.id,
                    name: person.name,
                })
                .collect(),
        }
    }
}
