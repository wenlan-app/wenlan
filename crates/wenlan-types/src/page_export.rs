// SPDX-License-Identifier: Apache-2.0
//! Immutable portable page versions. Availability is recorded, never inferred from today's sources.
use crate::pages::PageCitation;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageExportKind {
    Saved,
    PendingDraft,
    LegacyUnavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BibliographyAvailability {
    Available,
    Missing,
    GenerationMismatch,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageExportSource {
    pub locator: String,
    pub source_kind: String,
    pub source_id: String,
    pub title: String,
    pub source_agent: Option<String>,
    pub url: Option<String>,
    pub text_digest: Option<String>,
    pub historical: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageExportBibliography {
    pub source_id: String,
    pub title: String,
    pub bibtex: Option<String>,
    pub availability: BibliographyAvailability,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageExportBundle {
    pub id: String,
    pub digest: String,
    pub kind: PageExportKind,
    pub page_id: String,
    pub page_incarnation: String,
    pub space_id: Option<String>,
    pub version: i64,
    pub export_revision: u64,
    pub source_revision: i64,
    pub title: String,
    pub content: String,
    pub last_modified: String,
    pub captured_at: i64,
    pub citations: Option<Vec<PageCitation>>,
    pub sources: Vec<PageExportSource>,
    pub bibliography: Vec<PageExportBibliography>,
    pub references_available: bool,
    pub pending_revision_id: Option<String>,
    /// Exact card bytes/fences; omitted for saved bundles.
    pub pending_card_digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageExportVersion {
    pub bundle_id: Option<String>,
    pub kind: PageExportKind,
    pub version: i64,
    pub export_revision: Option<u64>,
    pub title: Option<String>,
    pub saved_at: String,
    pub captured_at: Option<i64>,
    pub references_available: bool,
    pub bibliography_available: bool,
    pub unavailable_reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageExportVersionsResponse {
    pub versions: Vec<PageExportVersion>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapturePageExportRequest {
    pub space: Option<String>,
}
/// Internal typed import bridge; never populated from an export client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishExportBibliography {
    pub space_id: String,
    pub source_id: String,
    pub import_content_hash: String,
    pub title: String,
    pub bibtex: String,
    pub url: Option<String>,
}
