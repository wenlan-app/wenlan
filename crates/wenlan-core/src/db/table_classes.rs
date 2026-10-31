// SPDX-License-Identifier: Apache-2.0
//! Where every table goes when Markdown files become the source of truth,
//! and migration 145, which drops the first retired tables.
//!
//! [`TABLE_CLASSES`] is the allowlist the storage migration works from. It is
//! read off `sqlite_master` of a fresh database (an upgraded one has the same
//! tables) and the non-test readers of each table:
//!
//! - [`TableClass::File`]: state the person or the AI made that will live in
//!   visible or settings files.
//! - [`TableClass::History`]: what happened, in the append-only history.
//! - [`TableClass::Cache`]: derived, rebuilt from files and history.
//! - [`TableClass::Retire`]: no place in the new layout, but still read by live
//!   code (a reader gate, a default-off experiment, a legacy import guard).
//!   Each one is removed together with its readers in a later change.
//!
//! [`DROPPED_TABLES`] had no live reader left: only the migration that filled
//! them and the migration tests read them. Migration 145 drops them, after
//! the usual pre-migration backup.
use super::{commit_or_rollback, MemoryDB};
use crate::error::WenlanError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableClass {
    File,
    History,
    Cache,
    Retire,
}

/// Every table and view in a fresh database after the last migration,
/// SQLite's own `sqlite_*` tables aside.
pub const TABLE_CLASSES: &[(&str, TableClass)] = {
    use TableClass::*;
    &[
        // Pages, canvases, memories, Brief, the person's notes about sources,
        // imported OKF metadata, confirmations, settings, entity files.
        ("pages", File),
        ("page_maps", File),
        ("page_map_nodes", File),
        ("page_map_edges", File),
        ("memories", File),
        ("briefs", File),
        ("brief_items", File),
        ("source_annotations", File),
        ("okf_concepts", File),
        ("okf_concept_links", File),
        ("page_basis", File),
        ("page_code_basis", File),
        ("spaces", File),
        ("profiles", File),
        ("agent_connections", File),
        ("onboarding_milestones", File),
        ("app_metadata", File),
        ("entity_page_map", File),
        ("page_evidence", File),
        ("page_sources", File),
        // History: versions, approvals, receipts, tombstones, sessions, reads.
        ("page_history", History),
        ("brief_history", History),
        ("operation_receipts", History),
        ("presence_receipts", History),
        ("presence_nonces", History),
        ("rejected_memories", History),
        ("page_truth_state", History),
        ("page_draft_create_requests", History),
        ("session_reads", History),
        ("agent_sessions", History),
        ("agent_activity", History),
        ("enrichment_origin", History),
        // Usage counts are rebuilt from the reads stream, never reset.
        ("access_log", History),
        // Search indexes and their shadow tables.
        ("memories_fts", Cache),
        ("memories_fts_config", Cache),
        ("memories_fts_data", Cache),
        ("memories_fts_docsize", Cache),
        ("memories_fts_idx", Cache),
        ("pages_fts", Cache),
        ("pages_fts_config", Cache),
        ("pages_fts_data", Cache),
        ("pages_fts_docsize", Cache),
        ("pages_fts_idx", Cache),
        ("summary_nodes_fts", Cache),
        ("summary_nodes_fts_config", Cache),
        ("summary_nodes_fts_data", Cache),
        ("summary_nodes_fts_docsize", Cache),
        ("summary_nodes_fts_idx", Cache),
        ("child_vectors", Cache),
        ("child_vectors_vec_idx_shadow", Cache),
        ("idx_pages_embedding_shadow", Cache),
        ("idx_summary_nodes_embedding_shadow", Cache),
        ("libsql_vector_meta_shadow", Cache),
        ("memories_vec_idx_shadow", Cache),
        ("memory_enrichment_summary", Cache),
        // Links, entities, relations, claims and checks.
        ("page_links", Cache),
        ("memory_entities", Cache),
        ("entity_minhash_bands", Cache),
        ("entity_type_vocabulary", Cache),
        ("relation_type_vocabulary", Cache),
        ("vocab_heal_ledger", Cache),
        ("relations", Cache),
        ("observations", Cache),
        ("edges", Cache),
        ("claims", Cache),
        ("claim_anchors", Cache),
        ("claim_derivation_jobs", Cache),
        ("claim_derivation_markers", Cache),
        ("claim_judge_eligibility", Cache),
        ("claim_judge_eligibility_generation", Cache),
        ("claim_judgment_attempts", Cache),
        ("claim_revisions", Cache),
        ("claim_run_candidates", Cache),
        ("claim_run_generations", Cache),
        ("claim_run_inventories", Cache),
        ("page_version_claims", Cache),
        ("entailment_cache", Cache),
        ("truth_marker_audit", Cache),
        // Grouping, summaries, provenance.
        ("communities", Cache),
        ("community_members", Cache),
        ("space_graph_state", Cache),
        ("grouping_leases", Cache),
        ("page_community_assignments", Cache),
        ("page_community_route_inputs", Cache),
        ("community_route_space_inputs", Cache),
        ("summary_nodes", Cache),
        ("summary_node_sources", Cache),
        ("provenance_roots", Cache),
        ("provenance_root_minhash_bands", Cache),
        // Work queues and caches.
        ("enrichment_steps", Cache),
        ("document_enrichment_queue", Cache),
        ("derived_artifact_sweeps", Cache),
        ("refinement_queue", Cache),
        ("narrative_cache", Cache),
        ("briefing_cache", Cache),
        ("source_sync_state", Cache),
        ("import_state", Cache),
        ("document_tags", Cache),
        // Eval harness; moves to the eval database.
        ("eval_judgments", Cache),
        ("eval_signals", Cache),
        ("eval_tune_log", Cache),
        // The community reader gate: `community_reader_durable_gate_sql` and
        // `reconcile_pending_community_readers` still pick between the durable
        // and the legacy grouping on every summary read.
        ("community_parity_input_state", Retire),
        ("community_publication_receipts", Retire),
        ("community_reader_current_input", Retire),
        ("community_reader_cutover", Retire),
        ("community_reader_space_proof", Retire),
        ("community_reader_watermark", Retire),
        // The default-off M6/genesis lane (`WENLAN_ENABLE_GENESIS_SHADOW`).
        ("genesis_candidate_roots", Retire),
        ("genesis_candidates", Retire),
        ("genesis_card_binding", Retire),
        ("genesis_coverage_state", Retire),
        ("genesis_frontier", Retire),
        ("genesis_group_coverage", Retire),
        ("genesis_quarantine", Retire),
        ("genesis_refresh_jobs", Retire),
        ("genesis_suppression", Retire),
        ("m6_adjacency", Retire),
        ("m6_counters", Retire),
        ("m6_deploy_attestation", Retire),
        ("m6_genesis_provenance", Retire),
        ("m6_overview_subscriptions", Retire),
        ("m6_pair_stats", Retire),
        ("m6_readiness", Retire),
        ("m6_readiness_soak_receipts", Retire),
        ("m6_refresh_dependencies", Retire),
        ("page_projection_outbox", Retire),
        // Guards the one-time import of legacy Brief files.
        ("brief_legacy_imports", Retire),
        // Legacy screen captures (`wenlan cleanup-legacy-captures`).
        ("activities", Retire),
        ("capture_refs", Retire),
        ("session_snapshots", Retire),
    ]
};

/// Tables migration 145 drops. Each one was written and read only by the
/// migration that filled it and by that migration's tests.
pub const DROPPED_TABLES: &[&str] = &[
    // Edges cutover state. The only later reader was `bump_dual_write_epoch`,
    // which nothing called.
    "edges_migration_state",
    // Entity shadow-page backfill cursor (migration 92).
    "entity_page_migration_state",
    // Pre-fold values kept by the space fold (80) and the kind fold (81/107).
    "page_space_fold_ledger",
    "page_kind_fold_ledger",
];

/// The dropped tables as their migrations created them, with the migration
/// that created each one.
const DROPPED_TABLE_DDL: &[(i64, &str)] = &[
    (
        80,
        "CREATE TABLE IF NOT EXISTS page_space_fold_ledger (
            page_id TEXT PRIMARY KEY,
            prior_space TEXT,
            prior_workspace TEXT,
            assigned_space TEXT NOT NULL,
            rule TEXT NOT NULL CHECK(rule IN ('workspace','space_residue','unfiled')),
            migrated_at TEXT NOT NULL
        )",
    ),
    (
        81,
        "CREATE TABLE IF NOT EXISTS edges_migration_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            stage TEXT NOT NULL,
            cursor TEXT,
            batch_checksum TEXT,
            epoch INTEGER NOT NULL DEFAULT 1,
            completed_at INTEGER,
            report_json TEXT
        )",
    ),
    (
        89,
        "CREATE TABLE IF NOT EXISTS page_kind_fold_ledger (
            page_id TEXT PRIMARY KEY,
            prior_creation_kind TEXT NOT NULL,
            assigned_kind TEXT NOT NULL CHECK(assigned_kind IN ('entity','concept','source','overview','authored')),
            rule TEXT NOT NULL CHECK(rule IN ('overview_title','authored','source','concept')),
            migrated_at TEXT NOT NULL
        )",
    ),
    (
        92,
        "CREATE TABLE IF NOT EXISTS entity_page_migration_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            stage TEXT NOT NULL,
            cursor TEXT,
            epoch INTEGER NOT NULL DEFAULT 1,
            completed_at INTEGER,
            report_json TEXT
        )",
    ),
];

/// Every migration body below 145 is a frozen replay, and 107 and 111 read
/// tables 145 drops. A database at a version between their creating
/// migration and 145 always has them; one that does not (its version was
/// set back after 145 ran, as the migration tests do) gets them back empty
/// before the chain replays, the same way `ensure_legacy_entities_table`
/// stands `entities` back up. Migration 145 drops them again at the end.
pub(super) async fn ensure_dropped_tables_for_replay(
    conn: &libsql::Connection,
    version: i64,
) -> Result<(), WenlanError> {
    if version >= 145 {
        return Ok(());
    }
    for (created_by, ddl) in DROPPED_TABLE_DDL {
        if version >= *created_by {
            conn.execute(ddl, ())
                .await
                .map_err(|e| WenlanError::VectorDb(format!("restore retired table: {e}")))?;
        }
    }
    if version >= 81 {
        // Migration 111 bumps the epoch on this one row, as 81 left it.
        conn.execute(
            "INSERT OR IGNORE INTO edges_migration_state (id, stage, epoch)
             VALUES (1, 'backfilled', 1)",
            (),
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("restore edges state row: {e}")))?;
    }
    Ok(())
}

pub fn table_class(name: &str) -> Option<TableClass> {
    TABLE_CLASSES
        .iter()
        .find(|(table, _)| *table == name)
        .map(|(_, class)| *class)
}

impl MemoryDB {
    pub(super) async fn migrate_145_drop_retired_tables(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(145, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ())
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m145 begin: {e}")))?;
        for table in DROPPED_TABLES {
            if let Err(error) = conn
                .execute(&format!("DROP TABLE IF EXISTS {table}"), ())
                .await
            {
                let _ = conn.execute("ROLLBACK", ()).await;
                return Err(WenlanError::VectorDb(format!("m145 drop {table}: {error}")));
            }
        }
        if let Err(error) = conn.execute("PRAGMA user_version = 145", ()).await {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(WenlanError::VectorDb(format!("m145 stamp: {error}")));
        }
        commit_or_rollback(&conn)
            .await
            .map_err(|e| WenlanError::VectorDb(format!("m145 commit: {e}")))?;
        log::info!(
            "[migration] Migration 145 applied: dropped retired tables {}",
            DROPPED_TABLES.join(", ")
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "table_classes_test.rs"]
mod tests;
