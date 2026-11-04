// SPDX-License-Identifier: Apache-2.0
//! Shared wire shapes for the local Zotero connector.

use serde::{Deserialize, Serialize};

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
pub struct AddZoteroSourceRequest {
    pub collection_key: String,
    /// Prevents registering a different Zotero installation after the user
    /// reviewed the collection list.
    pub expected_server_id: Option<String>,
    /// Explicit destination Space name. `Uncategorized` is not accepted here.
    pub space: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroDocumentResponse {
    pub source_id: String,
    pub title: String,
    pub attachments: Vec<ZoteroAttachmentResponse>,
    pub bibtex: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoteroAttachmentResponse {
    pub key: String,
    pub title: String,
    pub available: bool,
}
