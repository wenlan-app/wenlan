// SPDX-License-Identifier: Apache-2.0
//! A thing's map of points: the recap and the mind map on its page read the
//! same nodes.
//!
//! Migration 152 adds four tables:
//! - `thing_maps`: one row per thing whose map was ever written, with its
//!   `revision` (a thing with no row is an empty map at revision 0).
//! - `thing_map_nodes`: the points. `parent_id` is `NULL` only for the root;
//!   `kind` is an open set; `state` is open, decided or dropped; `by` is who
//!   wrote the point last (`you`, `ai` or `local`); `sources` is JSON.
//! - `thing_map_edges`: links beyond parent and child, such as an answer.
//! - `thing_map_proposals`: changes the AI or Wenlan's model asked for on
//!   points the person wrote (UX-WHO-2), waiting for the person.
//!
//! A request applies its ops in order, all or none, against the revision the
//! caller read. The map is small, so it is read whole, changed in memory and
//! written back in one transaction.

use super::{commit_or_rollback, MemoryDB};
use crate::WenlanError;
use std::collections::HashSet;
use wenlan_types::{
    ThingMapAuthor, ThingMapEdge, ThingMapNode, ThingMapOp, ThingMapProposal, ThingMapResponse,
    ThingMapSource, ThingMapState,
};

const THING_MAP_DDL: &str = "
CREATE TABLE IF NOT EXISTS thing_maps (
    thing_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS thing_map_nodes (
    thing_id TEXT NOT NULL,
    id TEXT NOT NULL,
    parent_id TEXT,
    rank REAL NOT NULL,
    kind TEXT NOT NULL,
    state TEXT CHECK (state IN ('open', 'decided', 'dropped')),
    label TEXT NOT NULL,
    by TEXT NOT NULL CHECK (by IN ('you', 'ai', 'local')),
    sources TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (thing_id, id)
);
CREATE TABLE IF NOT EXISTS thing_map_edges (
    thing_id TEXT NOT NULL,
    from_id TEXT NOT NULL,
    to_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (thing_id, from_id, to_id, kind)
);
CREATE TABLE IF NOT EXISTS thing_map_proposals (
    thing_id TEXT NOT NULL,
    id TEXT NOT NULL,
    by TEXT NOT NULL CHECK (by IN ('ai', 'local')),
    change TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (thing_id, id)
);
";

/// Most points one map holds.
pub const MAX_MAP_NODES: usize = 500;
/// Most ops one request carries.
pub const MAX_MAP_OPS: usize = 200;
/// Longest point label, in characters.
pub const MAX_LABEL_CHARS: usize = 500;
/// Longest `kind`, in characters.
const MAX_KIND_CHARS: usize = 40;
/// Most sources one point cites.
const MAX_SOURCES: usize = 20;

fn db_err(context: &str) -> impl Fn(libsql::Error) -> WenlanError + '_ {
    move |e| WenlanError::VectorDb(format!("{context}: {e}"))
}

fn invalid(message: impl Into<String>) -> WenlanError {
    WenlanError::Validation(format!("thing_map: {}", message.into()))
}

fn author_text(by: ThingMapAuthor) -> &'static str {
    match by {
        ThingMapAuthor::You => "you",
        ThingMapAuthor::Ai => "ai",
        ThingMapAuthor::Local => "local",
    }
}

fn author_of(text: &str) -> Result<ThingMapAuthor, WenlanError> {
    match text {
        "you" => Ok(ThingMapAuthor::You),
        "ai" => Ok(ThingMapAuthor::Ai),
        "local" => Ok(ThingMapAuthor::Local),
        other => Err(WenlanError::Generic(format!("thing map author {other}"))),
    }
}

fn state_text(state: ThingMapState) -> &'static str {
    match state {
        ThingMapState::Open => "open",
        ThingMapState::Decided => "decided",
        ThingMapState::Dropped => "dropped",
    }
}

fn state_of(text: &str) -> Result<ThingMapState, WenlanError> {
    match text {
        "open" => Ok(ThingMapState::Open),
        "decided" => Ok(ThingMapState::Decided),
        "dropped" => Ok(ThingMapState::Dropped),
        other => Err(WenlanError::Generic(format!("thing map state {other}"))),
    }
}

/// A map as stored, with each point's times.
#[derive(Debug, Clone, Default)]
struct Stored {
    revision: i64,
    nodes: Vec<(ThingMapNode, i64, i64)>,
    edges: Vec<ThingMapEdge>,
    proposals: Vec<ThingMapProposal>,
}

impl Stored {
    fn response(&self, thing_id: &str) -> ThingMapResponse {
        let mut nodes: Vec<ThingMapNode> = self.nodes.iter().map(|(n, _, _)| n.clone()).collect();
        nodes.sort_by(|a, b| {
            a.parent_id
                .cmp(&b.parent_id)
                .then(a.rank.total_cmp(&b.rank))
                .then(a.id.cmp(&b.id))
        });
        let mut edges = self.edges.clone();
        edges.sort_by(|a, b| (&a.from, &a.to, &a.kind).cmp(&(&b.from, &b.to, &b.kind)));
        ThingMapResponse {
            thing_id: thing_id.to_string(),
            revision: self.revision,
            nodes,
            edges,
            proposals: self.proposals.clone(),
        }
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|(n, _, _)| n.id == id)
    }

    fn node(&self, id: &str) -> Result<&ThingMapNode, WenlanError> {
        self.index(id)
            .map(|i| &self.nodes[i].0)
            .ok_or_else(|| invalid(format!("no point {id}")))
    }

    /// `id` and every point under it.
    fn subtree(&self, id: &str) -> HashSet<String> {
        let mut found: HashSet<String> = HashSet::from([id.to_string()]);
        loop {
            let before = found.len();
            for (node, _, _) in &self.nodes {
                if node
                    .parent_id
                    .as_ref()
                    .is_some_and(|parent| found.contains(parent))
                {
                    found.insert(node.id.clone());
                }
            }
            if found.len() == before {
                return found;
            }
        }
    }

    fn next_rank(&self, parent_id: Option<&str>) -> f64 {
        self.nodes
            .iter()
            .filter(|(n, _, _)| n.parent_id.as_deref() == parent_id)
            .map(|(n, _, _)| n.rank)
            .fold(None, |best: Option<f64>, rank| {
                Some(best.map_or(rank, |b| b.max(rank)))
            })
            .map_or(0.0, |rank| rank + 1.0)
    }

    /// Whether this change, by a machine, touches a point the person wrote,
    /// and so waits for them (UX-WHO-2).
    fn needs_the_person(&self, op: &ThingMapOp) -> bool {
        let persons = |id: &str| {
            self.index(id)
                .is_some_and(|i| self.nodes[i].0.by == ThingMapAuthor::You)
        };
        match op {
            ThingMapOp::Edit { id, .. } | ThingMapOp::Move { id, .. } => persons(id),
            ThingMapOp::Remove { id } => self.subtree(id).iter().any(|id| persons(id)),
            _ => false,
        }
    }

    /// Apply one change. `by` is who makes it.
    fn apply(&mut self, op: &ThingMapOp, by: ThingMapAuthor, now: i64) -> Result<(), WenlanError> {
        match op {
            ThingMapOp::Add {
                id,
                parent_id,
                rank,
                kind,
                state,
                label,
                sources,
            } => {
                let id = match id {
                    Some(id) => {
                        check_id(id)?;
                        if self.index(id).is_some() {
                            return Err(invalid(format!("point {id} already exists")));
                        }
                        id.clone()
                    }
                    None => new_node_id(),
                };
                match parent_id {
                    Some(parent) => {
                        self.node(parent)?;
                    }
                    None if !self.nodes.is_empty() => {
                        return Err(invalid("only the first point has no parent"));
                    }
                    None => {}
                }
                if self.nodes.len() >= MAX_MAP_NODES {
                    return Err(invalid(format!(
                        "a map holds at most {MAX_MAP_NODES} points"
                    )));
                }
                check_kind(kind)?;
                check_label(label)?;
                check_sources(sources)?;
                let rank = match rank {
                    Some(rank) => check_rank(*rank)?,
                    None => self.next_rank(parent_id.as_deref()),
                };
                self.nodes.push((
                    ThingMapNode {
                        id,
                        parent_id: parent_id.clone(),
                        rank,
                        kind: kind.clone(),
                        state: *state,
                        label: label.trim().to_string(),
                        by,
                        sources: sources.clone(),
                    },
                    now,
                    now,
                ));
            }
            ThingMapOp::Edit {
                id,
                kind,
                state,
                label,
                sources,
            } => {
                let i = self
                    .index(id)
                    .ok_or_else(|| invalid(format!("no point {id}")))?;
                if let Some(kind) = kind {
                    check_kind(kind)?;
                }
                if let Some(label) = label {
                    check_label(label)?;
                }
                if let Some(sources) = sources {
                    check_sources(sources)?;
                }
                let (node, _, updated) = &mut self.nodes[i];
                if let Some(kind) = kind {
                    node.kind = kind.clone();
                }
                if state.is_some() {
                    node.state = *state;
                }
                if let Some(label) = label {
                    node.label = label.trim().to_string();
                }
                if let Some(sources) = sources {
                    node.sources = sources.clone();
                }
                node.by = by;
                *updated = now;
            }
            ThingMapOp::Move {
                id,
                parent_id,
                rank,
            } => {
                let i = self
                    .index(id)
                    .ok_or_else(|| invalid(format!("no point {id}")))?;
                self.node(parent_id)?;
                if self.subtree(id).contains(parent_id) {
                    return Err(invalid(format!("{id} cannot go under itself")));
                }
                let rank = match rank {
                    Some(rank) => check_rank(*rank)?,
                    None => self.next_rank(Some(parent_id)),
                };
                let (node, _, updated) = &mut self.nodes[i];
                node.parent_id = Some(parent_id.clone());
                node.rank = rank;
                node.by = by;
                *updated = now;
            }
            ThingMapOp::Remove { id } => {
                self.node(id)?;
                let gone = self.subtree(id);
                self.nodes.retain(|(n, _, _)| !gone.contains(&n.id));
                self.edges
                    .retain(|e| !gone.contains(&e.from) && !gone.contains(&e.to));
            }
            ThingMapOp::Link { from, to, kind } => {
                self.node(from)?;
                self.node(to)?;
                if from == to {
                    return Err(invalid("a point cannot link to itself"));
                }
                check_kind(kind)?;
                let edge = ThingMapEdge {
                    from: from.clone(),
                    to: to.clone(),
                    kind: kind.clone(),
                };
                if !self.edges.contains(&edge) {
                    self.edges.push(edge);
                }
            }
            ThingMapOp::Unlink { from, to, kind } => {
                self.edges
                    .retain(|e| !(e.from == *from && e.to == *to && e.kind == *kind));
            }
            ThingMapOp::Accept { .. } | ThingMapOp::Reject { .. } => {
                return Err(invalid("a proposal is answered on its own"));
            }
        }
        Ok(())
    }
}

fn new_node_id() -> String {
    format!("tmn_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

fn new_proposal_id() -> String {
    format!("tmp_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

fn check_id(id: &str) -> Result<(), WenlanError> {
    let fine = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if fine {
        Ok(())
    } else {
        Err(invalid(format!(
            "a point id is 1 to 64 letters, digits, _ or -: {id}"
        )))
    }
}

fn check_kind(kind: &str) -> Result<(), WenlanError> {
    let fine = !kind.is_empty()
        && kind.chars().count() <= MAX_KIND_CHARS
        && kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if fine {
        Ok(())
    } else {
        Err(invalid(format!(
            "a kind is 1 to {MAX_KIND_CHARS} lowercase letters, digits or _: {kind}"
        )))
    }
}

fn check_label(label: &str) -> Result<(), WenlanError> {
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_LABEL_CHARS {
        return Err(invalid(format!(
            "a label is 1 to {MAX_LABEL_CHARS} characters"
        )));
    }
    Ok(())
}

fn check_rank(rank: f64) -> Result<f64, WenlanError> {
    if rank.is_finite() {
        Ok(rank)
    } else {
        Err(invalid("rank must be a finite number"))
    }
}

fn check_sources(sources: &[ThingMapSource]) -> Result<(), WenlanError> {
    if sources.len() > MAX_SOURCES {
        return Err(invalid(format!(
            "a point cites at most {MAX_SOURCES} sources"
        )));
    }
    for source in sources {
        if source.session_id.trim().is_empty() {
            return Err(invalid("a source names its conversation"));
        }
        let negative = source.from.is_some_and(|n| n < 0) || source.to.is_some_and(|n| n < 0);
        let backwards = matches!((source.from, source.to), (Some(from), Some(to)) if from > to);
        if negative || backwards {
            return Err(invalid("a source's from and to are positions, from first"));
        }
    }
    Ok(())
}

impl MemoryDB {
    /// Migration 152: the thing map tables. They are `IF NOT EXISTS`, so a
    /// crash before the version bump replays cleanly; they start empty.
    pub(super) async fn migrate_152_thing_map(
        &self,
        prior_version: i64,
    ) -> Result<(), WenlanError> {
        self.backup_before_migration(152, prior_version).await?;
        let conn = self.conn.lock().await;
        conn.execute_batch(THING_MAP_DDL)
            .await
            .map_err(db_err("m152 thing map DDL"))?;
        conn.execute("PRAGMA user_version = 152", ())
            .await
            .map_err(db_err("m152 bump"))?;
        log::info!("[migration] Migration 152 applied: thing maps");
        Ok(())
    }

    /// The thing's map. A thing nothing wrote a map for answers an empty one
    /// at revision 0. The caller checks the thing exists and is in scope.
    pub async fn get_thing_map(&self, thing_id: &str) -> Result<ThingMapResponse, WenlanError> {
        let conn = self.conn.lock().await;
        Ok(read_map(&conn, thing_id).await?.response(thing_id))
    }

    /// Apply `ops` in order, all or none, as `by`. A change by the AI or
    /// Wenlan's model to a point the person wrote waits as a proposal
    /// instead (UX-WHO-2); `Accept` and `Reject` are the person's alone.
    /// Returns the map and how many ops became proposals. 409 when
    /// `expected_revision` is not the current one.
    pub async fn apply_thing_map_ops(
        &self,
        thing_id: &str,
        by: ThingMapAuthor,
        expected_revision: i64,
        ops: &[ThingMapOp],
    ) -> Result<(ThingMapResponse, usize), WenlanError> {
        if ops.is_empty() {
            return Err(invalid("no ops"));
        }
        if ops.len() > MAX_MAP_OPS {
            return Err(invalid(format!("at most {MAX_MAP_OPS} ops at once")));
        }
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let mut map = read_map(&conn, thing_id).await?;
        if map.revision != expected_revision {
            return Err(WenlanError::Conflict(format!(
                "thing_map_revision: the map is at revision {}, not {expected_revision}",
                map.revision
            )));
        }
        let mut proposed = 0;
        for op in ops {
            match op {
                ThingMapOp::Accept { proposal_id } | ThingMapOp::Reject { proposal_id } => {
                    if by != ThingMapAuthor::You {
                        return Err(invalid("only the person answers a proposal"));
                    }
                    let i = map
                        .proposals
                        .iter()
                        .position(|p| p.id == *proposal_id)
                        .ok_or_else(|| invalid(format!("no proposal {proposal_id}")))?;
                    let proposal = map.proposals.remove(i);
                    if matches!(op, ThingMapOp::Accept { .. }) {
                        // It lands as the machine's change it was.
                        map.apply(&proposal.change, proposal.by, now)?;
                    }
                }
                _ if by != ThingMapAuthor::You && map.needs_the_person(op) => {
                    // Check it would apply now, so a proposal is never one
                    // that cannot land.
                    map.clone().apply(op, by, now)?;
                    map.proposals.push(ThingMapProposal {
                        id: new_proposal_id(),
                        by,
                        change: op.clone(),
                        created_at: now,
                    });
                    proposed += 1;
                }
                _ => map.apply(op, by, now)?,
            }
        }
        map.revision += 1;
        write_map(&conn, thing_id, &map, now).await?;
        Ok((map.response(thing_id), proposed))
    }
}

async fn read_map(conn: &libsql::Connection, thing_id: &str) -> Result<Stored, WenlanError> {
    let mut map = Stored::default();
    let mut rows = conn
        .query(
            "SELECT revision FROM thing_maps WHERE thing_id = ?1",
            libsql::params![thing_id],
        )
        .await
        .map_err(db_err("thing map revision"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(db_err("thing map revision row"))?
    {
        map.revision = row.get(0).map_err(db_err("thing map revision value"))?;
    }
    let mut rows = conn
        .query(
            "SELECT id, parent_id, rank, kind, state, label, by, sources, created_at, updated_at
               FROM thing_map_nodes WHERE thing_id = ?1",
            libsql::params![thing_id],
        )
        .await
        .map_err(db_err("thing map nodes"))?;
    while let Some(row) = rows.next().await.map_err(db_err("thing map node row"))? {
        let state: Option<String> = row.get(4).map_err(db_err("thing map node state"))?;
        let by: String = row.get(6).map_err(db_err("thing map node by"))?;
        let sources: String = row.get(7).map_err(db_err("thing map node sources"))?;
        map.nodes.push((
            ThingMapNode {
                id: row.get(0).map_err(db_err("thing map node id"))?,
                parent_id: row.get(1).map_err(db_err("thing map node parent"))?,
                rank: row.get(2).map_err(db_err("thing map node rank"))?,
                kind: row.get(3).map_err(db_err("thing map node kind"))?,
                state: state.as_deref().map(state_of).transpose()?,
                label: row.get(5).map_err(db_err("thing map node label"))?,
                by: author_of(&by)?,
                sources: serde_json::from_str(&sources)?,
            },
            row.get(8).map_err(db_err("thing map node created"))?,
            row.get(9).map_err(db_err("thing map node updated"))?,
        ));
    }
    let mut rows = conn
        .query(
            "SELECT from_id, to_id, kind FROM thing_map_edges WHERE thing_id = ?1",
            libsql::params![thing_id],
        )
        .await
        .map_err(db_err("thing map edges"))?;
    while let Some(row) = rows.next().await.map_err(db_err("thing map edge row"))? {
        map.edges.push(ThingMapEdge {
            from: row.get(0).map_err(db_err("thing map edge from"))?,
            to: row.get(1).map_err(db_err("thing map edge to"))?,
            kind: row.get(2).map_err(db_err("thing map edge kind"))?,
        });
    }
    let mut rows = conn
        .query(
            "SELECT id, by, change, created_at FROM thing_map_proposals
              WHERE thing_id = ?1 ORDER BY created_at, id",
            libsql::params![thing_id],
        )
        .await
        .map_err(db_err("thing map proposals"))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(db_err("thing map proposal row"))?
    {
        let by: String = row.get(1).map_err(db_err("thing map proposal by"))?;
        let change: String = row.get(2).map_err(db_err("thing map proposal change"))?;
        map.proposals.push(ThingMapProposal {
            id: row.get(0).map_err(db_err("thing map proposal id"))?,
            by: author_of(&by)?,
            change: serde_json::from_str(&change)?,
            created_at: row.get(3).map_err(db_err("thing map proposal at"))?,
        });
    }
    Ok(map)
}

async fn write_map(
    conn: &libsql::Connection,
    thing_id: &str,
    map: &Stored,
    now: i64,
) -> Result<(), WenlanError> {
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(db_err("thing map begin"))?;
    let written = write_rows(conn, thing_id, map, now).await;
    if let Err(error) = written {
        let _ = conn.execute("ROLLBACK", ()).await;
        return Err(error);
    }
    commit_or_rollback(conn)
        .await
        .map_err(db_err("thing map commit"))?;
    Ok(())
}

async fn write_rows(
    conn: &libsql::Connection,
    thing_id: &str,
    map: &Stored,
    now: i64,
) -> Result<(), WenlanError> {
    for table in ["thing_map_nodes", "thing_map_edges", "thing_map_proposals"] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE thing_id = ?1"),
            libsql::params![thing_id],
        )
        .await
        .map_err(db_err("thing map clear"))?;
    }
    conn.execute(
        "INSERT INTO thing_maps (thing_id, revision, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(thing_id) DO UPDATE SET revision = excluded.revision,
                                             updated_at = excluded.updated_at",
        libsql::params![thing_id, map.revision, now],
    )
    .await
    .map_err(db_err("thing map revision write"))?;
    for (node, created, updated) in &map.nodes {
        conn.execute(
            "INSERT INTO thing_map_nodes
               (thing_id, id, parent_id, rank, kind, state, label, by, sources,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            libsql::params![
                thing_id,
                node.id.as_str(),
                node.parent_id.as_deref(),
                node.rank,
                node.kind.as_str(),
                node.state.map(state_text),
                node.label.as_str(),
                author_text(node.by),
                serde_json::to_string(&node.sources)?,
                *created,
                *updated
            ],
        )
        .await
        .map_err(db_err("thing map node write"))?;
    }
    for edge in &map.edges {
        conn.execute(
            "INSERT INTO thing_map_edges (thing_id, from_id, to_id, kind)
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![
                thing_id,
                edge.from.as_str(),
                edge.to.as_str(),
                edge.kind.as_str()
            ],
        )
        .await
        .map_err(db_err("thing map edge write"))?;
    }
    for proposal in &map.proposals {
        conn.execute(
            "INSERT INTO thing_map_proposals (thing_id, id, by, change, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                thing_id,
                proposal.id.as_str(),
                author_text(proposal.by),
                serde_json::to_string(&proposal.change)?,
                proposal.created_at
            ],
        )
        .await
        .map_err(db_err("thing map proposal write"))?;
    }
    Ok(())
}
