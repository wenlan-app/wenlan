use crate::db::{EmbeddingBacklog, MemoryDB};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) fn summary_eligible_predicate(alias: &str) -> String {
    let minimum = crate::refinery::summary::min_bucket_members();
    let durable_gate = crate::db::community_reader_durable_gate_sql(
        crate::db::COMMUNITY_SUMMARY_ELIGIBILITY_CONSUMER,
    );
    // `supersede_mode='archive'` is superseder behaviour, and the store path
    // stamps it on every decision -- see `crate::db::not_self_archived`. Testing
    // it as row state kept every decision out of summary eligibility while
    // `load_summary_buckets` (which this predicate must agree with) counted the
    // same rows.
    let live = crate::db::not_self_archived(alias);
    let peer_live = crate::db::not_self_archived("peer");
    // The producer this predicate must agree with, `load_summary_buckets`,
    // additionally requires a member to have no pending revision and not be
    // hidden by a 'hide'-mode superseder (db.rs: `COALESCE(m.pending_revision,
    // 0) = 0` / `distillation_not_superseded("m")`, itself
    // `not_hidden_by_superseder(&ReadScope::Global, alias)` -- a correlated
    // `NOT EXISTS`). Both the peer count and the owner's own row must apply
    // the same two filters, or a community can be marked eligible around a
    // member the producer will never place in its bucket. Reusing
    // `not_hidden_by_superseder` directly here tripped
    // `summary_eligibility_query_plan_is_not_correlated` (each `NOT EXISTS`
    // re-correlates on its own `{alias}`), so the exclusion is expressed as
    // an uncorrelated `NOT IN` against the (once-computed) set of active
    // 'hide'-mode superseders instead -- same predicate, no per-row subquery.
    let not_superseded = |alias: &str| {
        format!(
            "{alias}.source_id NOT IN (
                 SELECT superseder.supersedes FROM memories superseder
                  WHERE superseder.supersedes IS NOT NULL
                    AND superseder.pending_revision = 0
                    AND superseder.source = 'memory'
                    AND superseder.supersede_mode = 'hide'
             )"
        )
    };
    let owner_not_superseded = not_superseded(alias);
    let peer_not_superseded = not_superseded("peer");
    // G6 Stage 1.5b Part 3: reads `community_id` off the entity's `kind='entity'`
    // shadow page via `entity_page_map`/`pages` rather than `entities` directly
    // (unconditional hard cutover, same program contract as `load_summary_buckets`
    // above) -- the column is mirrored 1:1 off `entities.community_id` by
    // `insert_entity_shadow_page`/`update_entity_shadow_page`.
    let legacy = format!(
        "{alias}.entity_id IN (
             SELECT owner_epm.entity_id
               FROM entity_page_map owner_epm
               JOIN pages owner_p
                 ON owner_p.id=owner_epm.page_id
                AND owner_p.kind='entity' AND owner_p.status='active'
             JOIN (
                 SELECT peer_p.community_id
                   FROM memories peer
                   JOIN entity_page_map peer_epm ON peer.entity_id=peer_epm.entity_id
                   JOIN pages peer_p
                     ON peer_p.id=peer_epm.page_id
                    AND peer_p.kind='entity' AND peer_p.status='active'
                  WHERE peer.source='memory' AND peer.chunk_index=0
                    AND peer.is_recap=0 AND {peer_live}
                    AND COALESCE(peer.pending_revision, 0) = 0
                    AND {peer_not_superseded}
                    AND peer.source_id NOT LIKE 'merged_%'
                    AND peer.source_id NOT LIKE 'recap_%'
                    AND peer.embedding IS NOT NULL
                    AND peer_p.community_id IS NOT NULL
                  GROUP BY peer_p.community_id
                 HAVING COUNT(*) >= {minimum}
             ) eligible ON eligible.community_id=owner_p.community_id
         )"
    );
    let durable = format!(
        "{alias}.entity_id IN (
             SELECT owner.node_id FROM community_members owner
             JOIN space_graph_state owner_state
               ON owner_state.space=owner.space
              AND owner.published_generation=owner_state.published_generation
             JOIN (
                 SELECT peer_member.space, peer_member.community_id
                   FROM memories peer
                   JOIN community_members peer_member
                     ON peer.entity_id=peer_member.node_id
                    AND peer.space=peer_member.space
                   JOIN space_graph_state peer_state
                     ON peer_state.space=peer_member.space
                    AND peer_member.published_generation=peer_state.published_generation
                   JOIN communities peer_community
                     ON peer_community.community_id=peer_member.community_id
                    AND peer_community.space=peer_member.space
                    AND peer_community.retired_at IS NULL
                  WHERE peer.source='memory' AND peer.chunk_index=0
                    AND peer.is_recap=0 AND {peer_live}
                    AND COALESCE(peer.pending_revision, 0) = 0
                    AND {peer_not_superseded}
                    AND peer.source_id NOT LIKE 'merged_%'
                    AND peer.source_id NOT LIKE 'recap_%'
                    AND peer.embedding IS NOT NULL
                  GROUP BY peer_member.space, peer_member.community_id
                 HAVING COUNT(*) >= {minimum}
             ) eligible
               ON eligible.space=owner.space
              AND eligible.community_id=owner.community_id
         )"
    );
    format!(
        "{alias}.is_recap=0
         AND {live}
         AND COALESCE({alias}.pending_revision, 0) = 0
         AND {owner_not_superseded}
         AND {alias}.source_id NOT LIKE 'merged_%'
         AND {alias}.source_id NOT LIKE 'recap_%'
         AND {alias}.embedding IS NOT NULL
         AND CASE WHEN ({durable_gate}) THEN ({durable}) ELSE ({legacy}) END"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DerivedArtifact {
    Episode,
    Fact,
    Summary,
}

impl DerivedArtifact {
    const fn index(self) -> usize {
        match self {
            Self::Episode => 0,
            Self::Fact => 1,
            Self::Summary => 2,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct DerivedArtifactState {
    active: [AtomicU32; 3],
    generation: AtomicU64,
    /// Memory rows were stored without a vector on purpose (a big import)
    /// and the background catch-up has not finished embedding them.
    embedding_backlog: AtomicBool,
    /// Live counts behind that flag, kept here so `GET /api/activity` never
    /// has to count the rows without a vector (a scan of the whole table)
    /// on every poll. One lock for both numbers so a reader never sees one
    /// updated without the other.
    embedding_progress: Mutex<EmbeddingBacklog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DerivedArtifactSample {
    active: [u32; 3],
    generation: u64,
    embedding_backlog: bool,
}

impl DerivedArtifactState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn begin(self: &Arc<Self>, artifact: DerivedArtifact) -> DerivedArtifactGuard {
        self.active[artifact.index()].fetch_add(1, Ordering::AcqRel);
        DerivedArtifactGuard {
            state: Arc::clone(self),
            artifact,
        }
    }

    pub(crate) fn sample(&self) -> DerivedArtifactSample {
        DerivedArtifactSample {
            active: std::array::from_fn(|index| self.active[index].load(Ordering::Acquire)),
            generation: self.generation.load(Ordering::Acquire),
            embedding_backlog: self.embedding_backlog.load(Ordering::Acquire),
        }
    }

    fn set_embedding_backlog(&self, pending: bool) {
        if self.embedding_backlog.swap(pending, Ordering::AcqRel) != pending {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn embedding_progress(&self) -> std::sync::MutexGuard<'_, EmbeddingBacklog> {
        // The counts are plain numbers: a panic elsewhere cannot leave them
        // half written, so a poisoned lock is still safe to read.
        self.embedding_progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A write stored `rows` rows without a vector. They join the current
    /// catch-up: both `remaining` and `total` grow, so `done` stays put.
    fn add_embedding_backlog(&self, rows: u64) {
        if rows == 0 {
            return;
        }
        {
            let mut progress = self.embedding_progress();
            progress.remaining = progress.remaining.saturating_add(rows);
            progress.total = progress.total.saturating_add(rows);
        }
        self.set_embedding_backlog(true);
    }

    /// The catch-up embedded `rows` rows.
    fn embedded(&self, rows: u64) {
        let mut progress = self.embedding_progress();
        progress.remaining = progress.remaining.saturating_sub(rows);
    }

    /// Re-anchor `remaining` on a real count of rows without a vector. The
    /// running counts can drift from it (a note re-synced or deleted while
    /// waiting, a write counted twice around the count), and the count wins.
    /// `total` never shrinks and `done` never goes back: `total` grows just
    /// enough to keep what is already done, so the UI never shows done
    /// moving backwards or above total.
    fn begin_embedding_catch_up(&self, waiting: u64) {
        if waiting == 0 {
            self.end_embedding_catch_up();
            return;
        }
        {
            let mut progress = self.embedding_progress();
            let done = progress.done();
            progress.remaining = waiting;
            progress.total = progress.total.max(done.saturating_add(waiting));
        }
        self.set_embedding_backlog(true);
    }

    fn end_embedding_catch_up(&self) {
        *self.embedding_progress() = EmbeddingBacklog::default();
        self.set_embedding_backlog(false);
    }

    fn embedding_backlog_progress(&self) -> EmbeddingBacklog {
        *self.embedding_progress()
    }
}

impl DerivedArtifactSample {
    pub(crate) const fn is_active(self, artifact: DerivedArtifact) -> bool {
        self.active[artifact.index()] > 0
    }

    pub(crate) const fn embedding_backlog(self) -> bool {
        self.embedding_backlog
    }
}

pub(crate) struct DerivedArtifactGuard {
    state: Arc<DerivedArtifactState>,
    artifact: DerivedArtifact,
}

impl Drop for DerivedArtifactGuard {
    fn drop(&mut self) {
        self.state.active[self.artifact.index()].fetch_sub(1, Ordering::AcqRel);
        self.state.generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl MemoryDB {
    pub(crate) fn begin_derived_artifact_write(
        &self,
        artifact: DerivedArtifact,
    ) -> DerivedArtifactGuard {
        self.derived_artifact_state.begin(artifact)
    }

    pub(crate) fn derived_artifact_sample(&self) -> DerivedArtifactSample {
        self.derived_artifact_state.sample()
    }

    /// A big import stored `rows` memory rows without a vector on purpose.
    /// While rows wait for the background catch-up the memory lint counts a
    /// missing vector as pending, not as damage, and Activity reports them.
    pub(crate) fn add_embedding_backlog(&self, rows: usize) {
        self.derived_artifact_state
            .add_embedding_backlog(u64::try_from(rows).unwrap_or(u64::MAX));
    }

    /// The background catch-up embedded `rows` rows.
    pub(crate) fn note_backlog_rows_embedded(&self, rows: usize) {
        self.derived_artifact_state
            .embedded(u64::try_from(rows).unwrap_or(u64::MAX));
    }

    /// A catch-up run (or pass) counted `waiting` rows without a vector:
    /// mark the backlog and re-anchor its counts on that real number. Called
    /// once per run, never per poll, since the count scans the whole table.
    /// Zero ends the backlog.
    pub fn begin_embedding_catch_up(&self, waiting: usize) {
        self.derived_artifact_state
            .begin_embedding_catch_up(u64::try_from(waiting).unwrap_or(u64::MAX));
    }

    /// The catch-up run ended (nothing left, nothing more it can embed, or a
    /// failed batch): clear the flag and both counts. Rows still without a
    /// vector are counted again by the next run.
    pub fn end_embedding_catch_up(&self) {
        self.derived_artifact_state.end_embedding_catch_up();
    }

    /// Rows still waiting for a vector and the size of the current catch-up,
    /// from the live counters: no database read. All zero when no catch-up
    /// is running.
    pub fn embedding_backlog(&self) -> EmbeddingBacklog {
        self.derived_artifact_state.embedding_backlog_progress()
    }
}

#[cfg(test)]
mod embedding_backlog_tests {
    use super::DerivedArtifactState;
    use crate::db::tests::test_db;
    use crate::db::EmbeddingBacklog;
    use crate::sources::RawDocument;

    fn progress(state: &DerivedArtifactState) -> (u64, u64, u64, bool) {
        let backlog = state.embedding_backlog_progress();
        (
            backlog.remaining,
            backlog.total,
            backlog.done(),
            state.sample().embedding_backlog(),
        )
    }

    #[test]
    fn deferred_rows_add_and_embedded_rows_subtract() {
        let state = DerivedArtifactState::default();
        state.add_embedding_backlog(100);
        assert_eq!(progress(&state), (100, 100, 0, true));
        state.embedded(30);
        assert_eq!(progress(&state), (70, 100, 30, true));
        // A second import mid catch-up joins it: done stays where it was.
        state.add_embedding_backlog(50);
        assert_eq!(progress(&state), (120, 150, 30, true));
        // Subtracting more than is left never wraps.
        state.embedded(500);
        assert_eq!(progress(&state), (0, 150, 150, true));
        state.end_embedding_catch_up();
        assert_eq!(progress(&state), (0, 0, 0, false));
    }

    /// A run re-anchors on its real count; total never shrinks and done
    /// never moves backwards or past total, whichever way the count drifts.
    #[test]
    fn a_run_count_re_anchors_without_moving_done_back() {
        let state = DerivedArtifactState::default();
        // Boot with leftover rows and no counters: the count is the total.
        state.begin_embedding_catch_up(400);
        assert_eq!(progress(&state), (400, 400, 0, true));
        state.embedded(100);
        assert_eq!(progress(&state), (300, 400, 100, true));
        // Fewer than expected (rows deleted while waiting): done jumps ahead.
        state.begin_embedding_catch_up(250);
        assert_eq!(progress(&state), (250, 400, 150, true));
        // More than expected (a write counted late): total grows, done holds.
        state.begin_embedding_catch_up(320);
        assert_eq!(progress(&state), (320, 470, 150, true));
        // A zero count ends the backlog.
        state.begin_embedding_catch_up(0);
        assert_eq!(progress(&state), (0, 0, 0, false));
    }

    #[test]
    fn backlog_flag_changes_bump_the_generation_once() {
        let state = DerivedArtifactState::default();
        let before = state.sample();
        state.add_embedding_backlog(0);
        assert_eq!(state.sample(), before, "nothing written, nothing changes");
        state.add_embedding_backlog(2);
        state.add_embedding_backlog(3);
        let during = state.sample();
        assert!(during.embedding_backlog());
        state.embedded(5);
        assert_eq!(state.sample(), during, "counts alone never bump it");
        state.end_embedding_catch_up();
        assert!(!state.sample().embedding_backlog());
    }

    fn note(source_id: &str) -> RawDocument {
        RawDocument {
            source: "memory".to_string(),
            source_id: source_id.to_string(),
            title: "note".to_string(),
            content: "a vault note stored first and embedded in the background".to_string(),
            last_modified: 1,
            ..RawDocument::default()
        }
    }

    /// The deferred upsert adds exactly the rows it wrote (no model needed).
    #[tokio::test]
    async fn deferred_upsert_adds_the_rows_it_wrote() {
        let (db, _tmp) = test_db().await;
        assert_eq!(db.embedding_backlog(), EmbeddingBacklog::default());
        let first = db
            .upsert_documents_without_embeddings(vec![note("vault::a.md::0")])
            .await
            .unwrap();
        let second = db
            .upsert_documents_without_embeddings(vec![note("vault::b.md::0")])
            .await
            .unwrap();
        let written = (first + second) as u64;
        assert!(written >= 2);
        assert_eq!(
            db.embedding_backlog(),
            EmbeddingBacklog {
                remaining: written,
                total: written,
            }
        );
        // The count the activity route reads comes from the same counters.
        let counts = db.activity_counts().await.unwrap();
        assert_eq!(counts.embedding_backlog.remaining, written);
        db.end_embedding_catch_up();
        assert_eq!(db.embedding_backlog(), EmbeddingBacklog::default());
    }

    /// Each recovery batch subtracts the rows it embedded. Needs the
    /// embedding model.
    #[tokio::test]
    async fn a_recovery_batch_subtracts_the_rows_it_embedded() {
        let (db, _tmp) = test_db().await;
        let written = db
            .upsert_documents_without_embeddings(vec![
                note("vault::a.md::0"),
                note("vault::b.md::0"),
            ])
            .await
            .unwrap();
        let waiting = db.count_null_memory_embeddings().await.unwrap();
        db.begin_embedding_catch_up(waiting);
        assert_eq!(db.embedding_backlog().remaining, waiting as u64);
        let batch = db
            .recover_null_memory_embeddings_batch(None, crate::db::NULL_EMBEDDING_RECOVERY_BATCH)
            .await
            .unwrap();
        assert_eq!(batch.recovered, written);
        let backlog = db.embedding_backlog();
        assert_eq!(backlog.remaining, (waiting - batch.recovered) as u64);
        assert_eq!(backlog.done(), batch.recovered as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::summary_eligible_predicate;
    use crate::db::tests::test_db;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn summary_eligibility_query_plan_is_not_correlated() {
        let (db, _tmp) = test_db().await;
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT m.source_id FROM memories m
              WHERE m.source='memory' AND ({})",
            summary_eligible_predicate("m")
        );
        let conn = db.test_primary_session().await;
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let mut details = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            details.push(row.get::<String>(3).unwrap());
        }
        assert!(
            details.iter().all(|detail| !detail.contains("CORRELATED")),
            "summary eligibility must be computed once, query plan: {details:?}"
        );
    }

    #[tokio::test]
    async fn summary_eligibility_requires_a_qualifying_community_and_candidate() {
        let (db, _tmp) = test_db().await;
        // G6 Stage 3: `summary_eligible_predicate`'s legacy branch reads
        // `community_id` off the entity's shadow page, and migration 123
        // dropped `entities`, so the shadow IS the seed.
        for (entity_id, community_id) in [
            ("large-a", 1),
            ("large-b", 1),
            ("large-c", 1),
            ("small-a", 2),
            ("small-b", 2),
        ] {
            db.test_seed_entity_shadow_page(
                crate::db::TestEntity::new(entity_id, entity_id, "concept")
                    .community_id(community_id),
            )
            .await
            .unwrap();
        }
        let conn = db.test_primary_session().await;
        let vector = format!(
            "[{}]",
            std::iter::repeat_n("0", 768).collect::<Vec<_>>().join(",")
        );
        conn.execute(
            "INSERT INTO memories
               (id,content,source,source_id,title,chunk_index,last_modified,chunk_type,
                stability,supersede_mode,embedding,entity_id,is_recap)
             VALUES
               ('large-a','a','memory','large-a','a',0,1,'text','new','hide',vector32(?1),'large-a',0),
               ('large-b','b','memory','large-b','b',0,1,'text','new','hide',vector32(?1),'large-b',0),
               ('large-c','c','memory','large-c','c',0,1,'text','new','hide',vector32(?1),'large-c',0),
               ('small-a','a','memory','small-a','a',0,1,'text','new','hide',vector32(?1),'small-a',0),
               ('small-b','b','memory','small-b','b',0,1,'text','new','hide',vector32(?1),'small-b',0),
               ('recap-large','r','memory','recap-large','r',0,1,'text','new','hide',vector32(?1),'large-a',1);",
            libsql::params![vector],
        )
        .await
        .unwrap();
        let sql = format!(
            "SELECT m.source_id FROM memories m
              WHERE m.source='memory' AND ({}) ORDER BY m.source_id",
            summary_eligible_predicate("m")
        );
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let mut eligible = BTreeSet::new();
        while let Some(row) = rows.next().await.unwrap() {
            eligible.insert(row.get::<String>(0).unwrap());
        }
        assert_eq!(
            eligible,
            BTreeSet::from([
                "large-a".to_string(),
                "large-b".to_string(),
                "large-c".to_string(),
            ])
        );
    }

    /// A peer the producer, `load_summary_buckets`, would exclude (a pending
    /// revision or a 'hide'-mode superseder) must also drop out of this
    /// predicate's peer count, or a community can be marked eligible around a
    /// member the producer never places in its bucket -- see the module
    /// comment above `summary_eligible_predicate`.
    #[tokio::test]
    async fn summary_eligibility_excludes_a_pending_revision_peer() {
        let (db, _tmp) = test_db().await;
        for entity_id in ["large-a", "large-b", "large-c"] {
            db.test_seed_entity_shadow_page(
                crate::db::TestEntity::new(entity_id, entity_id, "concept").community_id(1),
            )
            .await
            .unwrap();
        }
        let conn = db.test_primary_session().await;
        let vector = format!(
            "[{}]",
            std::iter::repeat_n("0", 768).collect::<Vec<_>>().join(",")
        );
        conn.execute(
            "INSERT INTO memories
               (id,content,source,source_id,title,chunk_index,last_modified,chunk_type,
                stability,supersede_mode,embedding,entity_id,is_recap,pending_revision)
             VALUES
               ('large-a','a','memory','large-a','a',0,1,'text','new','hide',vector32(?1),'large-a',0,0),
               ('large-b','b','memory','large-b','b',0,1,'text','new','hide',vector32(?1),'large-b',0,0),
               ('large-c','c','memory','large-c','c',0,1,'text','new','hide',vector32(?1),'large-c',0,1);",
            libsql::params![vector],
        )
        .await
        .unwrap();
        let sql = format!(
            "SELECT m.source_id FROM memories m
              WHERE m.source='memory' AND ({}) ORDER BY m.source_id",
            summary_eligible_predicate("m")
        );
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let mut eligible = BTreeSet::new();
        while let Some(row) = rows.next().await.unwrap() {
            eligible.insert(row.get::<String>(0).unwrap());
        }
        assert_eq!(
            eligible,
            BTreeSet::new(),
            "a pending-revision peer must drop the community below min_bucket_members, \
             matching load_summary_buckets"
        );
    }

    #[tokio::test]
    async fn summary_eligibility_excludes_a_hide_superseded_peer() {
        let (db, _tmp) = test_db().await;
        for entity_id in ["large-a", "large-b", "large-c"] {
            db.test_seed_entity_shadow_page(
                crate::db::TestEntity::new(entity_id, entity_id, "concept").community_id(1),
            )
            .await
            .unwrap();
        }
        let conn = db.test_primary_session().await;
        let vector = format!(
            "[{}]",
            std::iter::repeat_n("0", 768).collect::<Vec<_>>().join(",")
        );
        conn.execute(
            "INSERT INTO memories
               (id,content,source,source_id,title,chunk_index,last_modified,chunk_type,
                stability,supersede_mode,embedding,entity_id,is_recap,pending_revision)
             VALUES
               ('large-a','a','memory','large-a','a',0,1,'text','new','hide',vector32(?1),'large-a',0,0),
               ('large-b','b','memory','large-b','b',0,1,'text','new','hide',vector32(?1),'large-b',0,0),
               ('large-c','c','memory','large-c','c',0,1,'text','new','hide',vector32(?1),'large-c',0,0);",
            libsql::params![vector],
        )
        .await
        .unwrap();
        // A 'hide'-mode superseder replacing large-c must drop it from the
        // peer count exactly as `not_hidden_by_superseder` drops it from
        // `load_summary_buckets`. No `entity_id` here: the superseder itself
        // is not a community member, only the fact that it supersedes one.
        conn.execute(
            "INSERT INTO memories
               (id,content,source,source_id,title,chunk_index,last_modified,chunk_type,
                stability,supersede_mode,supersedes,is_recap,pending_revision)
             VALUES
               ('large-c-v2','c2','memory','large-c-v2','c2',0,2,'text','new','hide','large-c',
                0,0);",
            (),
        )
        .await
        .unwrap();
        let sql = format!(
            "SELECT m.source_id FROM memories m
              WHERE m.source='memory' AND ({}) ORDER BY m.source_id",
            summary_eligible_predicate("m")
        );
        let mut rows = conn.query(&sql, ()).await.unwrap();
        let mut eligible = BTreeSet::new();
        while let Some(row) = rows.next().await.unwrap() {
            eligible.insert(row.get::<String>(0).unwrap());
        }
        assert_eq!(
            eligible,
            BTreeSet::new(),
            "a hide-superseded peer must drop the community below min_bucket_members, \
             matching load_summary_buckets"
        );
    }
}
