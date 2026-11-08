// SPDX-License-Identifier: Apache-2.0
//! Reweave phase: re-link existing entities across memories using updated graph state.

use crate::db::MemoryDB;
use crate::error::WenlanError;

/// Reweave entity links: find memories with no entity_id and try to match them
/// against existing entities via vector similarity.
pub async fn reweave_entity_links(
    db: &MemoryDB,
    limit: usize,
    entity_link_distance: f64,
) -> Result<usize, WenlanError> {
    match db.unfile_old_hub_links_once().await {
        Ok(0) => {}
        Ok(removed) => log::info!(
            "[refinery] reweave: removed {removed} old link(s) to hub topics from memories with a specific topic"
        ),
        Err(e) => log::warn!("[refinery] reweave: old hub-link cleanup skipped: {e}"),
    }
    let unlinked = db.get_unlinked_memories(limit).await?;
    let mut linked = 0usize;
    for (source_id, content) in &unlinked {
        let entities = db.search_entities_by_vector(content, 3).await?;
        let candidates: Vec<&str> = entities.iter().map(|e| e.entity.id.as_str()).collect();
        let hubs = db.hub_entity_ids(&candidates).await?;
        for entity in &entities {
            // Same rule as `auto_link_entity`: closeness to a hub topic is
            // not a reason to file a memory under it.
            if hubs.contains(&entity.entity.id) {
                continue;
            }
            if entity.distance < entity_link_distance as f32 {
                db.update_memory_entity_id_as(
                    source_id,
                    &entity.entity.id,
                    crate::db::topic_filing::writer::REWEAVE,
                )
                .await?;
                linked += 1;
                break;
            }
        }
    }
    if linked > 0 {
        log::info!("[refinery] reweave: linked {} memories to entities", linked);
    }
    Ok(linked)
}
