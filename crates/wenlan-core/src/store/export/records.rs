// SPDX-License-Identifier: Apache-2.0
//! The person's and the AI's records that have no file shape of their own
//! yet, copied whole into `backstage/records/<table>.json` in the same
//! shape as the settings files, so nothing is lost before each one gets its
//! final place (spec sections 3 and 7):
//!
//! - `source_annotations`: the person's notes about a source, kept apart
//!   from the source's own text.
//! - `okf_concepts`, `okf_concept_links`: an imported OKF page's original
//!   frontmatter and concept links.
//! - `page_basis`, `page_code_basis`: every confirmation detail; the page
//!   file's `basis:` lines carry only the page and version.
//! - `entity_page_map`: which page an entity's file belongs to, as stored.
//! - `page_evidence`, `page_sources`: derived from the citation block, kept
//!   until the rebuild re-creates them and the comparison shows it does.
//! - What the AI worked out that would cost a model to work out again
//!   after a restore: the graph (`edges`, `observations`, `relations`) with
//!   the provenance roots its edges name, and the judge's score for each
//!   sentence and source span (`entailment_cache`, keyed by their text).
//!   The claims themselves are not kept: they are derived again from the
//!   page text without a model, and their support edges with them
//!   (`KEPT_ROWS` in `db::export_reads`); the scores make that
//!   derivation call no model for text that has not changed.
//! - `eval_signals`: the person's feedback (confirm, pin, edit, a search
//!   result opened).
//! - `enrichment_steps`: which AI steps (classify, extract entities, title,
//!   page growth) each memory has had, at which version. Their results are
//!   in the memory, entity and page files; without this record a restore
//!   would run every step again over every memory.

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
    "edges",
    "provenance_roots",
    "provenance_root_minhash_bands",
    "observations",
    "relations",
    "entailment_cache",
    "eval_signals",
    "enrichment_steps",
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
