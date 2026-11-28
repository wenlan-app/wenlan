// SPDX-License-Identifier: Apache-2.0
//! Explicit team publication and the host's member-visible library.

use serde::{Deserialize, Serialize};

use crate::team::{TeamAccess, TeamInfo, TeamPerson};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamItemKind {
    Page,
    Memory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamAuthorship {
    Human,
    Ai,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamText {
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPublication {
    pub kind: TeamItemKind,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub authorship: Option<TeamAuthorship>,
    #[serde(default)]
    pub space_id: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<i64>,
    #[serde(default)]
    pub memories: Vec<TeamText>,
    #[serde(default)]
    pub private_source: bool,
    #[serde(default)]
    pub brief: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamGroup {
    pub id: String,
    pub name: String,
    pub restricted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAudience {
    pub now: Vec<TeamPerson>,
    pub later: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPublicationPreview {
    pub id: String,
    pub team: TeamInfo,
    pub space: Option<TeamGroup>,
    pub audience: TeamAudience,
    pub draft: TeamPublication,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPublicationConfirm {
    pub preview_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPublicationReceipt {
    pub team_id: String,
    pub item_id: String,
    pub revision: i64,
    pub memory_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamItem {
    pub team_id: String,
    pub item_id: String,
    pub kind: TeamItemKind,
    pub revision: i64,
    pub space_id: Option<String>,
    pub title: String,
    pub body: String,
    pub contributor: Option<TeamPerson>,
    pub authorship: Option<TeamAuthorship>,
    pub publisher: Option<TeamPerson>,
    pub last_editor: Option<TeamPerson>,
    /// RFC 3339 time at which this item was first published.
    ///
    /// Older team library manifests did not record this field, so it remains
    /// optional when reading those manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TeamCitation {
    Memory { memory: Box<TeamItem> },
    PrivateSource { label: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPageSources {
    pub page_id: String,
    pub sources: Vec<TeamCitation>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPages {
    pub pages: Vec<TeamItem>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamRecall {
    pub pages: Vec<TeamItem>,
    pub memories: Vec<TeamItem>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamBrief {
    pub brief: Option<TeamItem>,
    pub related: TeamRecall,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPageHistory {
    pub revisions: Vec<TeamItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TeamLibraryInfo {
    pub team: TeamInfo,
    pub spaces: Vec<TeamGroup>,
    pub team_memories: bool,
}

/// The compact page row shown in a member's team-library list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPageSummary {
    pub id: String,
    pub title: String,
    pub space: Option<String>,
    pub published_by: Option<TeamPerson>,
    pub published_at: Option<String>,
}

/// One page opened from a member's team-library list. Team pages are always
/// read-only in the member app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPageRead {
    pub id: String,
    pub title: String,
    pub body: String,
    pub space: Option<String>,
    pub published_by: Option<TeamPerson>,
    pub published_at: Option<String>,
    pub read_only: bool,
}

/// A library Space and the pages visible to one member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamLibrarySpace {
    pub space: TeamGroup,
    pub pages: Vec<TeamPageSummary>,
}

/// The connection state for one joined team's library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum JoinedTeamLibraryState {
    Ready {
        spaces: Vec<TeamLibrarySpace>,
        /// Pages published to the whole team, before any Space groups.
        #[serde(default)]
        team_pages: Vec<TeamPageSummary>,
        /// True when the member route stopped at its hard page ceiling.
        #[serde(default)]
        truncated: bool,
    },
    Unreachable {
        reason: String,
    },
}

/// One joined team as seen by the member app. A cached team identity is kept
/// when the server is unreachable so the UI can name the failed team.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinedTeamLibrary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
    pub library: JoinedTeamLibraryState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinedTeamLibraries {
    pub teams: Vec<JoinedTeamLibrary>,
}

/// The response for one page in a joined team library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum JoinedTeamPage {
    Ready {
        page: Box<TeamPageRead>,
        sources: TeamPageSources,
    },
    Unreachable {
        reason: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamGroupCreate {
    pub name: String,
    #[serde(default)]
    pub restricted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamLibraryGrant {
    pub access: Option<TeamAccess>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolScope {
    pub team_id: String,
    pub space_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamToolKeyCreate {
    pub name: String,
    pub space_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TeamToolKeyReceipt {
    pub id: String,
    pub key: String,
    pub scope: TeamToolScope,
}

/// Checked against the person's library again at confirmation. These IDs
/// are local to the terminal review and never enter team records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamOriginal {
    pub kind: TeamItemKind,
    pub id: String,
    pub revision: i64,
}

/// One personal original and the text shown in a publication review.
/// These IDs are checked against the personal library again at confirmation;
/// they are not stored in team records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamDraftOriginal {
    pub original: TeamOriginal,
    pub text: TeamText,
}

/// The server-built publication draft shared by the CLI and desktop app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamDraft {
    pub item: TeamDraftOriginal,
    pub sources: Vec<TeamDraftOriginal>,
    pub omitted_sources: bool,
    pub authorship: Option<TeamAuthorship>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TeamTerminalAction {
    Preview {
        draft: TeamPublication,
        originals: Vec<TeamOriginal>,
    },
    Publish {
        preview_id: String,
    },
    Withdraw {
        item_id: String,
    },
    CreateSpace {
        name: String,
        restricted: bool,
    },
    SetGrant {
        member_id: String,
        access: Option<TeamAccess>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum TeamTerminalReply {
    Preview {
        preview: Box<TeamPublicationPreview>,
    },
    Published {
        receipt: TeamPublicationReceipt,
    },
    Space {
        space: TeamGroup,
        /// Set when the Space's Rules page could not be made yet; the
        /// first proposal makes it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rules_note: Option<String>,
    },
    Done,
}
