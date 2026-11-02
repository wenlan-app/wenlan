// SPDX-License-Identifier: Apache-2.0
//! The person's and the AI's records that have no file shape of their own
//! yet, copied whole into `backstage/records/<table>.json` in the same
//! shape as the settings files, so nothing is lost before each one gets its
//! final place (spec sections 3 and 7):
//!
//! - `source_annotations`: the person's notes about a source. Many sources
//!   (connected-folder documents) have no file of their own to hold them.
//! - `okf_concepts`, `okf_concept_links`: an imported OKF page's original
//!   frontmatter and concept links.
//! - `page_basis`, `page_code_basis`: every confirmation detail; the page
//!   file's `basis:` lines carry only the page and version.
//! - `entity_page_map`: which page an entity's file belongs to, as stored.
//! - `page_evidence`, `page_sources`: derived from the citation block, kept
//!   until the rebuild re-creates them and the comparison shows it does.

use super::settings::write_tables;
use super::{ExportDirs, Manifest};
use crate::db::MemoryDB;
use crate::error::WenlanError;

pub const RECORD_TABLES: &[&str] = &[
    "source_annotations",
    "okf_concepts",
    "okf_concept_links",
    "page_basis",
    "page_code_basis",
    "entity_page_map",
    "page_evidence",
    "page_sources",
];

/// Writes one file per record table; returns how many rows they hold.
pub async fn export_records(
    db: &MemoryDB,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<usize, WenlanError> {
    let mut rows = 0;
    for table in RECORD_TABLES {
        rows += write_tables(
            db,
            dirs,
            manifest,
            &format!("backstage/records/{table}.json"),
            table,
            "records",
            &[table],
        )
        .await?;
    }
    Ok(rows)
}
