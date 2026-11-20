// SPDX-License-Identifier: Apache-2.0
//! Team page wire types shared by the daemon and the hosted MCP adapter.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct MemberPage {
    pub id: String,
    pub title: String,
    pub content: String,
    pub space: Option<String>,
    pub version: i64,
    pub status: String,
    pub last_modified: String,
    pub review_required: bool,
    pub review_status: String,
    pub creation_kind: String,
    pub relevance_score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truth: Option<crate::pages::PageTruth>,
    // Required by the existing app and MCP Page readers.
    pub source_memory_ids: [(); 0],
    pub sources_updated_count: u8,
    pub created_at: String,
    pub last_compiled: String,
    pub user_edited: bool,
}

#[derive(Serialize, Deserialize)]
pub struct MemberPages<T> {
    pub pages: Vec<T>,
}

#[derive(Serialize, Deserialize)]
pub struct MemberPageEntry {
    #[serde(flatten)]
    pub page: MemberPage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub basis: Vec<MemberBasis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_path: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct MemberPageRead {
    pub page: MemberPageEntry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub basis: Vec<MemberBasis>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_changed: Vec<MemberLinkedPage>,
    pub body: crate::chat_check::PageBodyParts,
}

#[derive(Serialize, Deserialize)]
pub struct MemberBasis {
    pub page_id: String,
    pub title: String,
    pub state: String,
    pub from_version: i64,
    pub to_version: Option<i64>,
    pub minor_only: bool,
}

#[derive(Serialize, Deserialize)]
pub struct MemberLinkedPage {
    pub page_id: String,
    pub title: String,
    pub version: i64,
}

/// The hosted tool adds only counts to the daemon's authorized page view.
#[derive(Serialize)]
pub struct HostedPageRead {
    pub page: HostedReadEntry,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub basis: Vec<MemberBasis>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linked_changed: Vec<MemberLinkedPage>,
    pub sources: [(); 0],
}

#[derive(Serialize)]
pub struct HostedReadEntry {
    #[serde(flatten)]
    pub entry: MemberPageEntry,
    pub basis_count: usize,
    pub source_count: usize,
}

impl From<MemberPageRead> for HostedPageRead {
    fn from(read: MemberPageRead) -> Self {
        Self {
            page: HostedReadEntry {
                entry: read.page,
                basis_count: read.basis.len(),
                source_count: 0,
            },
            basis: read.basis,
            linked_changed: read.linked_changed,
            sources: [],
        }
    }
}

#[derive(Serialize)]
pub struct HostedPageList {
    pub count: usize,
    pub pages: Vec<HostedListEntry>,
}

#[derive(Serialize)]
pub struct HostedListEntry {
    page_id: String,
    title: String,
    space: Option<String>,
    status: String,
    last_modified: String,
    creation_kind: String,
    review_required: bool,
    source_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    basis_pending: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    basis: Vec<MemberBasis>,
}

impl From<MemberPages<MemberPageEntry>> for HostedPageList {
    fn from(list: MemberPages<MemberPageEntry>) -> Self {
        Self {
            count: list.pages.len(),
            pages: list
                .pages
                .into_iter()
                .map(|entry| HostedListEntry {
                    page_id: entry.page.id,
                    title: entry.page.title,
                    space: entry.page.space,
                    status: entry.page.status,
                    last_modified: entry.page.last_modified,
                    creation_kind: entry.page.creation_kind,
                    review_required: entry.page.review_required,
                    source_count: 0,
                    basis_pending: (!entry.basis.is_empty()).then_some(entry.basis.len()),
                    basis: entry.basis,
                })
                .collect(),
        }
    }
}
