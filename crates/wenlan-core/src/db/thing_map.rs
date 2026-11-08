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

/// Most of a map's points not kept yet that one night's reading sees.
pub const MAX_DREAM_MAP_POINTS: usize = 60;

/// A point a night kept, for its thing's map (UX-CHAT-5): the map and
/// long-term memory stay one point.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamMapWrite {
    /// The memory the night wrote.
    pub memory_id: String,
    /// The conversation it came from.
    pub session_id: String,
    pub text: String,
    pub decided: bool,
    /// The map point it is, when it was one already.
    pub node: Option<String>,
    /// Whether it replaces what that point said: a kept point no longer
    /// holds.
    pub replaces: bool,
}

/// A map point as a night's reading sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamMapPoint {
    pub id: String,
    pub label: String,
    pub state: Option<ThingMapState>,
}

/// What a night's reading needs from a thing's map: the points no night kept
/// yet, in reading order, and which point each kept memory is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DreamMapView {
    pub unkept: Vec<DreamMapPoint>,
    pub by_memory: std::collections::HashMap<String, String>,
}

/// What a night changed on a map, kept with the night so taking it back
/// puts the map back too.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DreamMapChanges {
    /// Points the night added, in the order it added them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Points it changed, as they were before and after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<DreamNodeChange>,
    /// Proposals it left on the person's points.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<String>,
}

impl DreamMapChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.proposals.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DreamNodeChange {
    pub before: ThingMapNode,
    pub after: ThingMapNode,
}

/// The memory a point is, if a night kept it.
fn memory_of(node: &ThingMapNode) -> Option<&str> {
    node.sources.iter().find_map(|s| s.memory_id.as_deref())
}

/// A point's sources with `memory_id` as the one memory it is, cited on the
/// conversation it came from.
fn sources_with_memory(
    sources: &[ThingMapSource],
    session_id: &str,
    memory_id: &str,
) -> Vec<ThingMapSource> {
    let mut out: Vec<ThingMapSource> = sources
        .iter()
        .cloned()
        .map(|mut s| {
            s.memory_id = None;
            s
        })
        .collect();
    match out.iter_mut().find(|s| s.session_id == session_id) {
        Some(source) => source.memory_id = Some(memory_id.to_string()),
        None => {
            if out.len() >= MAX_SOURCES {
                out.pop();
            }
            out.push(ThingMapSource {
                session_id: session_id.to_string(),
                from: None,
                to: None,
                memory_id: Some(memory_id.to_string()),
            });
        }
    }
    out
}

fn cut_label(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_LABEL_CHARS {
        text.to_string()
    } else {
        text.chars().take(MAX_LABEL_CHARS - 1).collect::<String>() + "…"
    }
}

impl MemoryDB {
    /// The map as a night's reading of the thing sees it.
    pub async fn dream_map_view(&self, thing_id: &str) -> Result<DreamMapView, WenlanError> {
        let conn = self.conn.lock().await;
        let map = read_map(&conn, thing_id).await?;
        drop(conn);
        let mut view = DreamMapView::default();
        let mut nodes: Vec<&ThingMapNode> = map
            .nodes
            .iter()
            .map(|(n, _, _)| n)
            .filter(|n| n.parent_id.is_some() && n.state != Some(ThingMapState::Dropped))
            .collect();
        nodes.sort_by(|a, b| a.rank.total_cmp(&b.rank).then(a.id.cmp(&b.id)));
        for node in nodes {
            match memory_of(node) {
                Some(memory) => {
                    view.by_memory.insert(memory.to_string(), node.id.clone());
                }
                None if view.unkept.len() < MAX_DREAM_MAP_POINTS => {
                    view.unkept.push(DreamMapPoint {
                        id: node.id.clone(),
                        label: node.label.clone(),
                        state: node.state,
                    })
                }
                None => {}
            }
        }
        Ok(view)
    }

    /// Put a night's points on the thing's map (UX-CHAT-5): a point that was
    /// on it already now names its memory, and takes the night's state and,
    /// when it replaces what the point said, its words; any other point is
    /// added under the map's top point, which is made from `title` when the
    /// map was empty. A change to a point the person wrote waits as a
    /// proposal instead (UX-WHO-2); naming its memory does not, since it
    /// leaves the person's words alone. Returns what changed, so taking the
    /// night back can undo it.
    pub async fn dream_into_map(
        &self,
        thing_id: &str,
        title: &str,
        writes: &[DreamMapWrite],
    ) -> Result<DreamMapChanges, WenlanError> {
        let mut changes = DreamMapChanges::default();
        if writes.is_empty() {
            return Ok(changes);
        }
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let mut map = read_map(&conn, thing_id).await?;
        if map.nodes.is_empty() {
            let root = ThingMapNode {
                id: new_node_id(),
                parent_id: None,
                rank: 0.0,
                kind: "topic".into(),
                state: None,
                label: cut_label(if title.trim().is_empty() {
                    "…"
                } else {
                    title
                }),
                by: ThingMapAuthor::Local,
                sources: Vec::new(),
            };
            changes.added.push(root.id.clone());
            map.nodes.push((root, now, now));
        }
        let Some(root) = map
            .nodes
            .iter()
            .find(|(n, _, _)| n.parent_id.is_none())
            .map(|(n, _, _)| n.id.clone())
        else {
            return Ok(changes);
        };
        for write in writes {
            let state = if write.decided {
                ThingMapState::Decided
            } else {
                ThingMapState::Open
            };
            let label = cut_label(&write.text);
            let found = write.node.as_deref().and_then(|id| map.index(id));
            let Some(i) = found else {
                if map.nodes.len() >= MAX_MAP_NODES {
                    continue;
                }
                let node = ThingMapNode {
                    id: new_node_id(),
                    parent_id: Some(root.clone()),
                    rank: map.next_rank(Some(&root)),
                    kind: "point".into(),
                    state: Some(state),
                    label,
                    by: ThingMapAuthor::Local,
                    sources: sources_with_memory(&[], &write.session_id, &write.memory_id),
                };
                changes.added.push(node.id.clone());
                map.nodes.push((node, now, now));
                continue;
            };
            let before = map.nodes[i].0.clone();
            let new_label = (write.replaces && label != before.label).then_some(label);
            let new_state = (before.state != Some(state)).then_some(state);
            let mut after = before.clone();
            after.sources =
                sources_with_memory(&before.sources, &write.session_id, &write.memory_id);
            if new_label.is_some() || new_state.is_some() {
                if before.by == ThingMapAuthor::You {
                    let proposal = ThingMapProposal {
                        id: new_proposal_id(),
                        by: ThingMapAuthor::Local,
                        change: ThingMapOp::Edit {
                            id: before.id.clone(),
                            kind: None,
                            state: new_state,
                            label: new_label,
                            sources: None,
                        },
                        created_at: now,
                    };
                    changes.proposals.push(proposal.id.clone());
                    map.proposals.push(proposal);
                } else {
                    if let Some(label) = new_label {
                        after.label = label;
                    }
                    if let Some(state) = new_state {
                        after.state = Some(state);
                    }
                    after.by = ThingMapAuthor::Local;
                }
            }
            if after != before {
                map.nodes[i].0 = after.clone();
                map.nodes[i].2 = now;
                changes.changed.push(DreamNodeChange { before, after });
            }
        }
        if !changes.is_empty() {
            map.revision += 1;
            write_map(&conn, thing_id, &map, now).await?;
        }
        Ok(changes)
    }

    /// Take a night's changes off the thing's map: its proposals still
    /// waiting go, each point it changed and nobody changed since goes back
    /// to what it was, and each point it added that nobody touched since and
    /// that has nothing under it is removed. What someone changed since
    /// stays as they left it.
    pub async fn undo_dream_map(
        &self,
        thing_id: &str,
        changes: &DreamMapChanges,
    ) -> Result<(), WenlanError> {
        if changes.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("thing map begin"))?;
        undo_dream_map_in(&tx, thing_id, changes, now).await?;
        tx.commit().await.map_err(db_err("thing map commit"))?;
        Ok(())
    }
}

/// [`MemoryDB::undo_dream_map`] on a connection the caller holds, so a
/// night's Undo can take its map changes back in the same transaction as
/// the rest of the night. The caller owns the transaction.
pub(super) async fn undo_dream_map_in(
    conn: &libsql::Connection,
    thing_id: &str,
    changes: &DreamMapChanges,
    now: i64,
) -> Result<(), WenlanError> {
    {
        if changes.is_empty() {
            return Ok(());
        }
        let mut map = read_map(conn, thing_id).await?;
        let before = (map.nodes.len(), map.proposals.len());
        let mut touched = false;
        map.proposals.retain(|p| !changes.proposals.contains(&p.id));
        for change in changes.changed.iter().rev() {
            if let Some(i) = map.index(&change.after.id) {
                if map.nodes[i].0 == change.after {
                    map.nodes[i].0 = change.before.clone();
                    map.nodes[i].2 = now;
                    touched = true;
                }
            }
        }
        for id in changes.added.iter().rev() {
            let Some(i) = map.index(id) else { continue };
            let node = &map.nodes[i].0;
            let has_children = map
                .nodes
                .iter()
                .any(|(n, _, _)| n.parent_id.as_deref() == Some(id.as_str()));
            if node.by == ThingMapAuthor::Local && !has_children {
                map.nodes.remove(i);
                map.edges.retain(|e| e.from != *id && e.to != *id);
            }
        }
        if touched || before != (map.nodes.len(), map.proposals.len()) {
            map.revision += 1;
            write_rows(conn, thing_id, &map, now).await?;
        }
        Ok(())
    }
}

/// The points a proposal's change names.
fn proposal_points(change: &ThingMapOp) -> Vec<&str> {
    match change {
        ThingMapOp::Add { id, parent_id, .. } => id
            .iter()
            .chain(parent_id.iter())
            .map(String::as_str)
            .collect(),
        ThingMapOp::Edit { id, .. } | ThingMapOp::Remove { id } => vec![id.as_str()],
        ThingMapOp::Move { id, parent_id, .. } => vec![id.as_str(), parent_id.as_str()],
        ThingMapOp::Link { from, to, .. } | ThingMapOp::Unlink { from, to, .. } => {
            vec![from.as_str(), to.as_str()]
        }
        ThingMapOp::Accept { .. } | ThingMapOp::Reject { .. } => Vec::new(),
    }
}

/// The map's top point, made from the thing's title when the map is empty.
async fn root_of(
    conn: &libsql::Connection,
    thing_id: &str,
    map: &mut Stored,
    now: i64,
) -> Result<String, WenlanError> {
    if let Some((root, _, _)) = map.nodes.iter().find(|(n, _, _)| n.parent_id.is_none()) {
        return Ok(root.id.clone());
    }
    let mut rows = conn
        .query(
            "SELECT title FROM things WHERE id = ?1",
            libsql::params![thing_id],
        )
        .await
        .map_err(db_err("thing map title"))?;
    let title = match rows.next().await.map_err(db_err("thing map title row"))? {
        Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
        None => None,
    }
    .filter(|t| !t.trim().is_empty())
    .unwrap_or_else(|| "…".to_string());
    let root = ThingMapNode {
        id: new_node_id(),
        parent_id: None,
        rank: 0.0,
        kind: "topic".into(),
        state: None,
        label: cut_label(&title),
        by: ThingMapAuthor::Local,
        sources: Vec::new(),
    };
    let id = root.id.clone();
    map.nodes.push((root, now, now));
    Ok(id)
}

/// A conversation moved from `from` to `to` (UX-THING-1): the points that
/// came only from it go with it, each keeping its id, its memory and what
/// sits under it, and land under the top point of `to`'s map. A point under
/// one of them that came from elsewhere stays, under the nearest point that
/// stays. A link or proposal goes along when every point it names goes,
/// and is dropped when only some do. Runs inside the caller's transaction.
pub(super) async fn carry_session_points(
    conn: &libsql::Connection,
    from: &str,
    to: &str,
    session_id: &str,
    now: i64,
) -> Result<(), WenlanError> {
    let mut source = read_map(conn, from).await?;
    let moving: HashSet<String> = source
        .nodes
        .iter()
        .map(|(n, _, _)| n)
        .filter(|n| {
            n.parent_id.is_some()
                && !n.sources.is_empty()
                && n.sources.iter().all(|s| s.session_id == session_id)
        })
        .map(|n| n.id.clone())
        .collect();
    if moving.is_empty() {
        return Ok(());
    }
    let parents: std::collections::HashMap<String, Option<String>> = source
        .nodes
        .iter()
        .map(|(n, _, _)| (n.id.clone(), n.parent_id.clone()))
        .collect();
    let staying_above = |id: &str| -> Option<String> {
        let mut at = parents.get(id).cloned().flatten();
        while let Some(parent) = at {
            if !moving.contains(&parent) {
                return Some(parent);
            }
            at = parents.get(&parent).cloned().flatten();
        }
        None
    };
    let mut target = read_map(conn, to).await?;
    let root = root_of(conn, to, &mut target, now).await?;
    let (carried, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut source.nodes)
        .into_iter()
        .partition(|(n, _, _)| moving.contains(&n.id));
    source.nodes = kept;
    // A point left without its parent goes after the last point under the
    // nearest one that stays.
    let orphans: Vec<String> = source
        .nodes
        .iter()
        .filter(|(n, _, _)| n.parent_id.as_ref().is_some_and(|p| moving.contains(p)))
        .map(|(n, _, _)| n.id.clone())
        .collect();
    for id in orphans {
        let parent = staying_above(&id);
        let rank = source
            .nodes
            .iter()
            .filter(|(n, _, _)| n.id != id && n.parent_id == parent)
            .map(|(n, _, _)| n.rank)
            .fold(None, |best: Option<f64>, r| {
                Some(best.map_or(r, |b| b.max(r)))
            })
            .map_or(0.0, |r| r + 1.0);
        if let Some(i) = source.index(&id) {
            source.nodes[i].0.parent_id = parent;
            source.nodes[i].0.rank = rank;
            source.nodes[i].2 = now;
        }
    }
    let mut carried = carried;
    carried.sort_by(|a, b| a.0.rank.total_cmp(&b.0.rank).then(a.0.id.cmp(&b.0.id)));
    for (mut node, created, _) in carried {
        if !node.parent_id.as_ref().is_some_and(|p| moving.contains(p)) {
            node.parent_id = Some(root.clone());
            node.rank = target.next_rank(Some(&root));
        }
        target.nodes.push((node, created, now));
    }
    let (edges, other_edges): (Vec<_>, Vec<_>) = std::mem::take(&mut source.edges)
        .into_iter()
        .partition(|e| moving.contains(&e.from) || moving.contains(&e.to));
    source.edges = other_edges;
    target.edges.extend(
        edges
            .into_iter()
            .filter(|e| moving.contains(&e.from) && moving.contains(&e.to)),
    );
    let mut stay = Vec::new();
    for proposal in std::mem::take(&mut source.proposals) {
        let named = proposal_points(&proposal.change);
        let going = named.iter().filter(|id| moving.contains(**id)).count();
        if going == 0 {
            stay.push(proposal);
        } else if going == named.len() {
            target.proposals.push(proposal);
        }
    }
    source.proposals = stay;
    source.revision += 1;
    target.revision += 1;
    write_rows(conn, from, &source, now).await?;
    write_rows(conn, to, &target, now).await?;
    Ok(())
}

/// `other` merged into `id` (UX-THING-3): its whole map, top point and all,
/// goes under the top point of `id`'s map, with its links and proposals.
/// Runs inside the caller's transaction.
pub(super) async fn carry_whole_map(
    conn: &libsql::Connection,
    id: &str,
    other: &str,
    now: i64,
) -> Result<(), WenlanError> {
    let mut merged = read_map(conn, other).await?;
    if merged.nodes.is_empty() && merged.proposals.is_empty() {
        return Ok(());
    }
    let mut kept = read_map(conn, id).await?;
    let root = root_of(conn, id, &mut kept, now).await?;
    let rank = kept.next_rank(Some(&root));
    for (mut node, created, updated) in std::mem::take(&mut merged.nodes) {
        if node.parent_id.is_none() {
            node.parent_id = Some(root.clone());
            node.rank = rank;
        }
        kept.nodes.push((node, created, updated));
    }
    kept.edges.append(&mut merged.edges);
    kept.proposals.append(&mut merged.proposals);
    kept.revision += 1;
    merged.revision += 1;
    write_rows(conn, id, &kept, now).await?;
    write_rows(conn, other, &merged, now).await?;
    Ok(())
}
