// SPDX-License-Identifier: Apache-2.0
//! A row stored without a vector (big import) and embedded later by the
//! background catch-up must end with the vector the write path would have
//! given it: same `[space]` prefix, same retrieval cue, and no prefix for a
//! row filed under no Space (whose column holds the Unfiled sentinel).

use super::tests::test_db;
use super::MemoryDB;
use crate::sources::RawDocument;

fn doc(source_id: &str, space: Option<&str>, cue: Option<&str>) -> RawDocument {
    RawDocument {
        source: "memory".to_string(),
        source_id: source_id.to_string(),
        title: "Delta grid checklist".to_string(),
        content: "The delta grid checklist lists every survey marker along the river \
                  bend, which ones were reset last spring, and who checks them next."
            .to_string(),
        last_modified: 1_760_000_000,
        space: space.map(str::to_string),
        retrieval_cue: cue.map(str::to_string),
        ..Default::default()
    }
}

async fn vector(db: &MemoryDB, source_id: &str) -> Option<Vec<f32>> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT CASE WHEN embedding IS NULL THEN NULL \
                    ELSE vector_extract(embedding) END \
             FROM memories WHERE source = 'memory' AND source_id = ?1",
            libsql::params![source_id],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("stored row");
    let text: Option<String> = row.get(0).unwrap();
    text.map(|text| serde_json::from_str(&text).expect("vector text"))
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm(a) * norm(b))
}

#[tokio::test]
async fn a_row_embedded_later_gets_the_vector_the_write_path_gives() {
    let (db, _dir) = test_db().await;
    let cases = [
        ("spaced", Some("Cartography"), None),
        (
            "cued",
            Some("Cartography"),
            Some("survey markers by the river"),
        ),
        ("unfiled", None, None),
    ];
    for (name, space, cue) in cases {
        db.upsert_documents(vec![doc(&format!("{name}-now"), space, cue)])
            .await
            .unwrap();
        db.upsert_documents_without_embeddings(vec![doc(&format!("{name}-later"), space, cue)])
            .await
            .unwrap();
        assert!(
            vector(&db, &format!("{name}-later")).await.is_none(),
            "{name}: stored without a vector"
        );
    }

    db.recover_null_memory_embeddings(&crate::events::NoopEmitter)
        .await
        .unwrap();

    for (name, _, _) in cases {
        let now = vector(&db, &format!("{name}-now")).await.expect("embedded");
        let later = vector(&db, &format!("{name}-later"))
            .await
            .expect("embedded by the catch-up");
        let similarity = cosine(&now, &later);
        assert!(
            similarity > 0.9999,
            "{name}: catch-up embedded different text (cosine {similarity})"
        );
    }
}
