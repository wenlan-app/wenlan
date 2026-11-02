//! S0-162: renaming a space closes over every space-keyed grouping row.
//!
//! `update_space` cascades over `memories`, `entities`, and `pages`/`workspace`
//! and historically stopped there, while the M4 community substrate keys on
//! the same mutable string. A stale name is not
//! self-healing: the scheduler claims dirty spaces with
//! `SELECT space FROM space_graph_state WHERE dirty = 1` and no join to a live
//! `spaces` row (`db/community_grouping_state.rs:84`), so an orphaned row can be
//! selected indefinitely.
//!
//! The postcondition is S0-162's, stated over rows rather than over statements:
//! after a rename, no grouping row names a space that does not resolve.
//! Every row keyed by the old name is either updated to the new name or retired.
//!
//! One space-keyed table is deliberately excluded and the exclusion is the scope
//! line: `page_draft_create_requests` (`db.rs:7783`). The old name it keeps is
//! not stale data, it is replay history, and its immutability is the table's
//! purpose — migration 76 says so at `db.rs:7771`, the create path matches a
//! replay against the stored request before validating the space against the
//! live row (`db/page_drafts.rs:246`), and
//! `create_replay_returns_the_server_mutated_scope_after_space_rename` locks it
//! down. Rewriting those rows would break delayed-create replay.

use crate::WenlanError;

/// The closure, in **dependency order**, each table paired with whether `space`
/// sits inside its PRIMARY KEY.
///
/// The order is load-bearing and is not alphabetical. `community_members` and
/// `page_community_assignments` both carry a BEFORE UPDATE fence that re-checks
/// the row's community against `communities.space` (`db.rs:11204`, `db.rs:10978`),
/// so `communities` has to be carrying the new name before either is rewritten
/// or the fence aborts the rename. `edges` carries the matching AFTER UPDATE
/// fence against its endpoint's space (`db.rs:8932`), which is why the caller
/// invokes this only after the memories/entities/pages cascade has run.
///
/// A `true` in the second field means a row already keyed to the destination
/// name would collide on the PRIMARY KEY. Re-derivable community control rows
/// are retired before the rewrite. This is not defensive: the ORDINARY rename
/// hits it. The pre-existing `pages` cascade fires
/// `m4_page_community_page_invalidate`, which does
/// `INSERT OR IGNORE INTO community_route_space_inputs (NEW.space, 0)`
/// (`db.rs:11014`) — so by the time this runs, a row under the destination name
/// already exists, created by this very rename.
///
/// Retiring it and moving the old row over is also the right direction rather
/// than merely the convenient one: the trigger's row starts at generation 0
/// while the old-name row carries the accumulated routing generation, and
/// keeping the fresh row would walk that counter backwards.
///
/// Rows that predate the rename are orphans by construction. `spaces.name` is
/// UNIQUE, so a rename to a live space fails before the cascade runs and any
/// surviving community row under that name names no live space.
const CLOSURE: &[(&str, bool)] = &[
    ("communities", false),
    ("community_members", true),
    ("page_community_assignments", false),
    ("page_community_route_inputs", false),
    ("community_route_space_inputs", true),
    ("space_graph_state", true),
    ("grouping_leases", true),
    // `community_reader_parity` is NOT here, and its absence is a correction to
    // S0-162 rather than an omission. The artifact's closed enumeration lists it
    // as the eleventh table and cites its DDL, but that DDL (`db.rs:10926`) is
    // undone by the later "retire local-only community parity model" migration,
    // which does `DROP TABLE IF EXISTS community_reader_parity` (`db.rs:11659`)
    // and replaces the per-space table with the global singleton
    // `community_parity_input_state` — which has no `space` column at all. The
    // table does not exist in a migrated database, so closing over it raises
    // `no such table`. Ten of the eleven are real; this one is a citation to a
    // statement a later migration takes back.
    ("community_reader_space_proof", true),
    ("community_publication_receipts", true),
    ("edges", false),
];

/// Rewrite every space-keyed row from `old_name` to `new_name` inside the
/// caller's transaction. The caller must already have renamed the `spaces` row
/// and cascaded `memories`, `entities`, and `pages` (see the `edges` fence
/// above).
pub(super) async fn cascade_space_rename(
    tx: &libsql::Transaction,
    old_name: &str,
    new_name: &str,
) -> Result<(), WenlanError> {
    // Table names come from `CLOSURE`, never from a caller; the space names are
    // parameterized.
    for (table, space_in_primary_key) in CLOSURE {
        if !space_in_primary_key {
            continue;
        }
        tx.execute(
            &format!("DELETE FROM {table} WHERE space = ?1"),
            libsql::params![new_name],
        )
        .await
        .map_err(|e| {
            WenlanError::VectorDb(format!("update_space retire orphan {table} rows: {e}"))
        })?;
    }
    for (table, _) in CLOSURE {
        tx.execute(
            &format!("UPDATE {table} SET space = ?1 WHERE space = ?2"),
            libsql::params![new_name, old_name],
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("update_space cascade {table}: {e}")))?;
    }
    cascade_agent_sessions(tx, old_name, new_name).await
}

/// `agent_sessions.space` is the Space of a session's first write, and
/// `session_reads.space` the Space a read was scoped to. They follow a rename
/// or merge like the rows they describe, outside `CLOSURE` because they hold
/// no grouping state.
async fn cascade_agent_sessions(
    tx: &libsql::Transaction,
    old_name: &str,
    new_name: &str,
) -> Result<(), WenlanError> {
    tx.execute(
        "UPDATE agent_sessions SET space = ?1 WHERE space = ?2",
        libsql::params![new_name, old_name],
    )
    .await
    .map_err(|e| WenlanError::VectorDb(format!("space cascade agent_sessions: {e}")))?;
    tx.execute(
        "UPDATE session_reads SET space = ?1 WHERE space = ?2",
        libsql::params![new_name, old_name],
    )
    .await
    .map(|_| ())
    .map_err(|e| WenlanError::VectorDb(format!("space cascade session_reads: {e}")))
}

/// Fold every space-keyed row from `old_name` into the LIVE space
/// `target_name` inside the caller's transaction, after the memories/pages
/// cascade (see the `edges` fence above). This is a merge, not a rename: the
/// destination has rows of its own that must survive, so nothing under
/// `target_name` is retired. Over the same `CLOSURE`, per table: `edges` are
/// re-keyed to the target; `communities` are retired the way
/// `finalize_community_grouping` retires a superseded generation; the
/// re-derivable community control rows are dropped. Finally the target is
/// marked dirty so grouping re-runs over the merged input.
pub(super) async fn cascade_space_merge(
    tx: &libsql::Transaction,
    old_name: &str,
    target_name: &str,
) -> Result<(), WenlanError> {
    // `edges` goes first, out of CLOSURE order: `m4_grouping_edge_update`
    // (db.rs `m4_grouping_edge_update`) re-materialises the `space_graph_state`
    // row of OLD.space when a grounded edge changes space, so that row can only
    // be retired once the edge rewrite is done.
    tx.execute(
        "UPDATE edges SET space = ?1 WHERE space = ?2",
        libsql::params![target_name, old_name],
    )
    .await
    .map_err(|e| WenlanError::VectorDb(format!("space merge cascade edges: {e}")))?;

    let now = chrono::Utc::now().timestamp();
    for (table, _) in CLOSURE {
        match *table {
            "edges" => {}
            "communities" => {
                tx.execute(
                    "UPDATE communities SET retired_at = ?2, updated_at = ?2 \
                     WHERE space = ?1 AND retired_at IS NULL",
                    libsql::params![old_name, now],
                )
                .await
                .map_err(|e| {
                    WenlanError::VectorDb(format!("space merge retire communities: {e}"))
                })?;
            }
            // Everything else in CLOSURE is re-derivable control state
            // (memberships, route inputs, leases, receipts): dropped, and
            // rebuilt by the target's next grouping pass.
            table => {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE space = ?1"),
                    libsql::params![old_name],
                )
                .await
                .map_err(|e| WenlanError::VectorDb(format!("space merge retire {table}: {e}")))?;
            }
        }
    }

    // Same shape as `m4_grouping_entity_update`'s destination upsert, so a
    // merge that moved no entity page still invalidates the target's grouping.
    tx.execute(
        "INSERT INTO space_graph_state
             (space, graph_generation, grouping_generation, published_generation, dirty)
         VALUES (?1, 0, 1, NULL, 1)
         ON CONFLICT(space) DO UPDATE SET
             grouping_generation = grouping_generation + 1,
             dirty = 1",
        libsql::params![target_name],
    )
    .await
    .map_err(|e| WenlanError::VectorDb(format!("space merge dirty target: {e}")))?;
    cascade_agent_sessions(tx, old_name, target_name).await
}

/// Every table the closure covers, for the teeth.
#[cfg(test)]
pub(super) fn closed_tables() -> Vec<&'static str> {
    CLOSURE.iter().map(|(table, _)| *table).collect()
}
