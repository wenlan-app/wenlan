//! One-time fold of topic types written before the type gate.
//!
//! New topics have their type folded onto `entity_type_vocabulary` when they
//! are created (`resolve_or_create_entity`, through
//! `kg::entity_admission::admit`). Topics created before that gate can still
//! carry a type outside it, such as `concepts`. This pass folds the ones whose
//! type is a plain case or plural variant of exactly one canonical type, and
//! leaves every other type as it is: those are counted by lint
//! (`KgEntitiesOffVocabularyType`) for a person to decide.

use super::{MemoryDB, WenlanError};

/// The app-metadata key that records the pass ran, with what it folded.
pub(crate) const FOLD_ENTITY_TYPE_VARIANTS_KEY: &str = "kg_fold_entity_type_variants_v1";

impl MemoryDB {
    /// Fold, once per database, every topic whose type is a case or plural
    /// variant of exactly one canonical type (`concepts` to `concept`). Each
    /// fold is recorded in `vocab_heal_ledger` by [`MemoryDB::fold_entity_type`],
    /// so it can be undone. Returns the topics folded; zero once the pass has
    /// run.
    pub async fn fold_entity_type_variants_once(&self) -> Result<u64, WenlanError> {
        if self
            .get_app_metadata(FOLD_ENTITY_TYPE_VARIANTS_KEY)
            .await?
            .is_some()
        {
            return Ok(0);
        }
        let canonicals = self.entity_canonicals().await?;
        let folds = self.entity_type_variant_folds(&canonicals).await?;
        let mut record = Vec::new();
        let mut total = 0u64;
        for (from, to) in folds {
            let folded = self.fold_entity_type(&from, &to).await? as u64;
            total += folded;
            record.push(serde_json::json!({"from": from, "to": to, "topics": folded}));
        }
        if total > 0 {
            log::info!(
                "[kg] folded {total} topic types written before the type gate: {}",
                serde_json::Value::Array(record.clone())
            );
        }
        self.set_app_metadata(
            FOLD_ENTITY_TYPE_VARIANTS_KEY,
            &serde_json::json!({"folded": record}).to_string(),
        )
        .await?;
        Ok(total)
    }

    /// Each type outside the vocabulary that a case or plural fold maps to
    /// exactly one canonical type, with that type. Any other type is left
    /// out: an alias such as `platform` to `technology` is a judgment, not a
    /// spelling.
    async fn entity_type_variant_folds(
        &self,
        canonicals: &[String],
    ) -> Result<Vec<(String, String)>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT DISTINCT entity_type FROM pages
                  WHERE kind = 'entity' AND entity_type IS NOT NULL
                    AND entity_type NOT IN (SELECT canonical FROM entity_type_vocabulary)
                  ORDER BY entity_type",
                (),
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("entity type variants: {e}")))?;
        let mut folds = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| WenlanError::VectorDb(format!("entity type variants: {e}")))?
        {
            let from: String = row.get(0).unwrap_or_default();
            if let Some(to) = crate::vocab::safe_transform::safe_transform(&from, canonicals) {
                folds.push((from, to));
            }
        }
        Ok(folds)
    }
}

#[cfg(test)]
mod tests {
    use super::FOLD_ENTITY_TYPE_VARIANTS_KEY;
    use crate::db::tests::test_db;
    use crate::db::TestEntity;
    use crate::lint::context::{CancellationToken, LintClock};
    use crate::lint::runner::LintRunner;
    use wenlan_types::lint::{LintMetricCode, LintMetricValue, LintQuery};

    async fn types(db: &crate::db::MemoryDB) -> Vec<(String, String)> {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT epm.entity_id, p.entity_type FROM entity_page_map epm
                   JOIN pages p ON p.id = epm.page_id ORDER BY epm.entity_id",
                (),
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((row.get(0).unwrap(), row.get(1).unwrap()));
        }
        out
    }

    async fn off_vocabulary_metric(db: &crate::db::MemoryDB) -> u64 {
        let report = LintRunner::new(LintClock::fixed(), CancellationToken::new())
            .run(
                db,
                &LintQuery {
                    profile: None,
                    space: None,
                },
                None,
                false,
            )
            .await
            .unwrap();
        report
            .checks()
            .iter()
            .find(|check| check.check_id() == "entities.partition_inventory")
            .expect("the partition inventory ran")
            .metrics()
            .iter()
            .find_map(|metric| {
                (metric.code() == LintMetricCode::KgEntitiesOffVocabularyType).then(|| match metric
                    .value()
                {
                    LintMetricValue::Count { value } => *value,
                    other => panic!("not a count: {other:?}"),
                })
            })
            .expect("the off-vocabulary count is reported")
    }

    async fn seed(db: &crate::db::MemoryDB) {
        for (id, name, entity_type) in [
            ("ent-a", "Data", "concepts"),
            ("ent-b", "Search", "Concept"),
            ("ent-c", "Reddit", "platform"),
            ("ent-d", "Rust", "technology"),
        ] {
            db.test_seed_entity_shadow_page(TestEntity::new(id, name, entity_type))
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn plural_and_case_variants_fold_once_and_the_rest_are_left_for_lint() {
        let (db, _tmp) = test_db().await;
        seed(&db).await;
        assert_eq!(off_vocabulary_metric(&db).await, 3);

        assert_eq!(db.fold_entity_type_variants_once().await.unwrap(), 2);
        assert_eq!(
            types(&db).await,
            [
                ("ent-a".to_string(), "concept".to_string()),
                ("ent-b".to_string(), "concept".to_string()),
                ("ent-c".to_string(), "platform".to_string()),
                ("ent-d".to_string(), "technology".to_string()),
            ],
            "only spelling variants fold; an alias like platform is a judgment"
        );
        assert_eq!(
            off_vocabulary_metric(&db).await,
            1,
            "platform is still counted"
        );

        let record = db
            .get_app_metadata(FOLD_ENTITY_TYPE_VARIANTS_KEY)
            .await
            .unwrap()
            .expect("the pass records what it folded");
        assert!(record.contains("concepts"), "{record}");

        // A type written later under an old rule is not folded again: the
        // pass is one-time, and lint keeps counting it.
        db.test_seed_entity_shadow_page(TestEntity::new("ent-e", "Later", "concepts"))
            .await
            .unwrap();
        assert_eq!(db.fold_entity_type_variants_once().await.unwrap(), 0);
        assert_eq!(off_vocabulary_metric(&db).await, 2);
    }

    #[tokio::test]
    async fn every_fold_is_recorded_in_the_vocabulary_ledger() {
        let (db, _tmp) = test_db().await;
        seed(&db).await;
        db.fold_entity_type_variants_once().await.unwrap();
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT old_value, new_value FROM vocab_heal_ledger
                  WHERE kind = 'entity' ORDER BY old_value",
                (),
            )
            .await
            .unwrap();
        let mut ledger = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            ledger.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
        }
        assert_eq!(
            ledger,
            [
                ("Concept".to_string(), "concept".to_string()),
                ("concepts".to_string(), "concept".to_string()),
            ]
        );
    }

    /// An archived topic is folded along with an active one, and the count
    /// and the ledger name both; a type found only on archived topics is
    /// folded too instead of being skipped while the pass reads as done.
    #[tokio::test]
    async fn archived_topics_are_folded_counted_and_ledgered() {
        let (db, _tmp) = test_db().await;
        for (id, name, entity_type) in [
            ("ent-a", "Data", "concepts"),
            ("ent-b", "Search", "concepts"),
            ("ent-c", "Index", "Concept"),
        ] {
            db.test_seed_entity_shadow_page(TestEntity::new(id, name, entity_type))
                .await
                .unwrap();
        }
        {
            let conn = db.conn.lock().await;
            conn.execute(
                "UPDATE pages SET status = 'archived' WHERE id IN \
                   (SELECT page_id FROM entity_page_map WHERE entity_id IN ('ent-b', 'ent-c'))",
                (),
            )
            .await
            .unwrap();
        }
        assert_eq!(db.fold_entity_type_variants_once().await.unwrap(), 3);
        assert_eq!(
            types(&db).await,
            [
                ("ent-a".to_string(), "concept".to_string()),
                ("ent-b".to_string(), "concept".to_string()),
                ("ent-c".to_string(), "concept".to_string()),
            ]
        );
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT old_value, pre_image FROM vocab_heal_ledger \
                  WHERE kind = 'entity' ORDER BY old_value",
                (),
            )
            .await
            .unwrap();
        let mut ledger = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            ledger.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
        }
        assert_eq!(ledger.len(), 2, "{ledger:?}");
        assert_eq!(ledger[0].0, "Concept");
        assert!(ledger[0].1.contains("ent-c"), "{ledger:?}");
        assert_eq!(ledger[1].0, "concepts");
        assert!(
            ledger[1].1.contains("ent-a") && ledger[1].1.contains("ent-b"),
            "{ledger:?}"
        );
    }
}
