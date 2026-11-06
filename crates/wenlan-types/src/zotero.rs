// SPDX-License-Identifier: Apache-2.0
//! Shared wire shapes for the local Zotero connector.

use serde::{Deserialize, Serialize};

/// Existing Review gate reason for retained evidence whose Zotero selection changed.
pub const ZOTERO_SELECTION_REVIEW_REQUIRED: &str = "zotero_selection_changed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroCollectionSummary {
    pub key: String,
    pub name: String,
    pub item_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroCollectionsResponse {
    pub server_id: Option<String>,
    pub collections: Vec<ZoteroCollectionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroItemSummary {
    pub key: String,
    pub title: String,
    pub item_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroItemsResponse {
    pub server_id: String,
    pub collection_key: String,
    pub items: Vec<ZoteroItemSummary>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddZoteroSourceRequest {
    /// None imports the collection; Some imports exactly this nonempty selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_keys: Option<Vec<String>>,
    pub collection_key: String,
    /// Prevents registering a different Zotero installation after the user
    /// reviewed the collection list.
    pub expected_server_id: Option<String>,
    /// Explicit destination Space name. `Uncategorized` is not accepted here.
    pub space: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoteroDocumentResponse {
    /// The selected attachment belongs to an authorized retained PDF generation.
    #[serde(default)]
    pub historical: bool,
    pub source_id: String,
    pub title: String,
    pub attachments: Vec<ZoteroAttachmentResponse>,
    pub bibtex: String,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub annotations: Vec<ZoteroPdfAnnotation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Exact surviving registration that imported this snapshot; not live remote membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_source_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroAttachmentResponse {
    pub key: String,
    pub title: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoteroPdfAnnotation {
    pub key: String,
    pub attachment_key: String,
    pub page_index: u32,
    pub rects: Vec<[f64; 4]>,
    pub text: String,
    pub comment: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoteroPdfPageResponse {
    #[serde(default)]
    pub source_lifecycle: crate::source_lifecycle::SourceLifecycleSummary,
    pub source_id: String,
    pub attachment_key: String,
    pub pdf_digest: String,
    pub page: u32,
    pub page_count: u32,
    pub text: String,
    pub extraction_complete: bool,
}

/// Immutable provenance binding; mutable registration IDs alone are insufficient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroRegistrationIdentity {
    pub registration_id: String,
    pub locator: String,
    pub server_id: String,
    pub space_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroDisconnectPreview {
    /// False for legacy registrations whose prior imported impact cannot be proven.
    pub impact_scope_known: bool,
    pub identity_digest: String,
    pub space_name: String,
    pub identity: ZoteroRegistrationIdentity,
    pub revision: u64,
    pub canonical_item_ids: Vec<String>,
    pub affected_page_ids: Vec<String>,
    pub pending_disconnect: bool,
    pub disconnected: bool,
}
