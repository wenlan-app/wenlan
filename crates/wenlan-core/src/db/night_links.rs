// SPDX-License-Identifier: Apache-2.0
//! Connecting a night's points to the big map (docs/ux/spec.md, UX-CHAT-5).
//!
//! A night's model reads a thing's conversations once. In that same call it
//! names the topic each point is about, from a short list made here. A point
//! with no topic that fits stays under its thing; nothing forces it into a
//! broad topic. Links go through the same hub rule and helpers as other links.
//!
//! What a night changed is one list in its `thing.dream` entry, so the morning
//! line, the replay and undo read one record. Points kept before nights named
//! topics are connected once, by hand, through the same path; that entry is
//! marked `backfill`, is not a night, and has its own undo.

use super::things::history;
use super::MemoryDB;
use crate::conversation_intake::{self as intake, FoundConversation, PriorPoint};
use crate::read_scope::ReadScope;
use crate::WenlanError;
use std::collections::{HashMap, HashSet};
use wenlan_types::ThingActor;

/// Most topics one thing's model is offered.
pub const MAX_NIGHT_TOPICS: usize = 25;
/// Most of those found by name in what the person typed.
const MAX_NAMED_TOPICS: usize = 10;
/// How many nearest topics are looked at, before leaving out what cannot be
/// offered.
const NEAREST_TOPICS: usize = 40;
/// Set once the points kept before nights named topics were connected. Its
/// value says how many links that made.
const BACKFILL_MARKER: &str = "night_links_backfill_v1";

/// One change a night made to the big map. `New`, `Link` and `Tidy` are
/// written; the record can hold the others when nights start making them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NightChange {
    /// A point was written.
    New { memory: String },
    /// A point was connected to a topic.
    Link { memory: String, entity: String },
    /// A note was brought up to date, from one revision to another.
    Update {
        page: String,
        before: i64,
        after: i64,
    },
    /// A point took the place of an older one, which stays in history.
    Replace { old: String, new: String },
    /// A topic or note was folded into another.
    Merge { from: String, into: String },
    /// A point left the map.
    Remove { memory: String },
    /// Memories an AI wrote were tidied (UX-GRAPH-7): `old` are hidden,
    /// their `supersede_mode` before kept in `before` to put back on undo;
    /// `new` stands for them. `wrote` is set when the night wrote `new`, so
    /// undo forgets it.
    Tidy {
        how: wenlan_types::NightTidyKind,
        old: Vec<String>,
        #[serde(default)]
        before: Vec<Option<String>>,
        new: String,
        #[serde(default)]
        wrote: bool,
        /// What it changed on the thing's map.
        #[serde(
            default,
            skip_serializing_if = "super::thing_map::DreamMapChanges::is_empty"
        )]
        map: super::thing_map::DreamMapChanges,
        /// Rows an `old` row had hidden as their superseder, hidden too so
        /// they do not come back once it no longer stands, with their own
        /// mode before, for undo.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also_before: Vec<Option<String>>,
    },
}

/// A link a night made that was not there before, so undo takes back only
/// this and leaves what other work linked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NightLink {
    pub memory: String,
    pub entity: String,
}

/// A topic a model may choose for a thing's points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NightTopic {
    pub entity_id: String,
    pub name: String,
    /// `None` when the topic is unfiled.
    pub space: Option<String>,
}

impl NightTopic {
    /// Whether a point in `space` may connect to this topic. A link never
    /// crosses Spaces; an unfiled topic can take a point from anywhere.
    pub fn fits(&self, space: Option<&str>) -> bool {
        self.space.is_none() || self.space.as_deref() == space
    }
}

/// A point kept before nights named topics, connected to nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnlinkedPoint {
    pub id: String,
    pub text: String,
    pub space: Option<String>,
}

impl MemoryDB {
    /// The topics a thing's model is offered: the topics its earlier points
    /// connect to, then specific names found in what the person typed, then
    /// the nearest by meaning. Hubs are never offered, and neither are topics
    /// of another Space or names of code. With no embedding model loaded, the
    /// list holds only the first two.
    pub async fn night_topics(
        &self,
        title: &str,
        conversations: &[FoundConversation],
        prior: &[PriorPoint],
        spaces: &[String],
    ) -> Result<Vec<NightTopic>, WenlanError> {
        let all = self.offerable_topics(spaces).await?;
        let raw = intake::night_said(title, conversations);
        let lower = raw.to_lowercase();
        let mut named: Vec<(&NightTopic, usize)> = all
            .values()
            .filter_map(|topic| {
                intake::specific_name_hits(&topic.name, &raw, &lower).map(|hits| (topic, hits))
            })
            .collect();
        named.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.0.name.chars().count().cmp(&a.0.name.chars().count()))
                .then_with(|| a.0.name.cmp(&b.0.name))
        });

        let mut order: Vec<String> = self.topics_of_points(prior).await?;
        order.extend(
            named
                .into_iter()
                .take(MAX_NAMED_TOPICS)
                .map(|(topic, _)| topic.entity_id.clone()),
        );
        // A search that fails costs the list its nearest topics, not the night.
        let query = intake::night_topic_query(title, conversations, prior);
        match self.search_entities_by_vector(&query, NEAREST_TOPICS).await {
            Ok(nearest) => order.extend(nearest.into_iter().map(|found| found.entity.id)),
            Err(error) => log::warn!("[things] finding topics near {title:?}: {error}"),
        }

        let mut seen = HashSet::new();
        let order: Vec<String> = order
            .into_iter()
            .filter(|id| all.contains_key(id) && seen.insert(id.clone()))
            .collect();
        let ids: Vec<&str> = order.iter().map(String::as_str).collect();
        let hubs = self.hub_entity_ids(&ids).await?;
        Ok(order
            .iter()
            .filter(|id| !hubs.contains(*id))
            .filter_map(|id| all.get(id).cloned())
            .take(MAX_NIGHT_TOPICS)
            .collect())
    }

    /// Every active topic with a name that can be offered, in these Spaces or
    /// unfiled, by entity id.
    async fn offerable_topics(
        &self,
        spaces: &[String],
    ) -> Result<HashMap<String, NightTopic>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT epm.entity_id, p.title, p.space
                   FROM pages p
                   JOIN entity_page_map epm ON epm.page_id = p.id
                  WHERE p.kind = 'entity' AND p.status = 'active'",
                (),
            )
            .await
            .map_err(db_err("night topics"))?;
        let mut topics = HashMap::new();
        while let Some(row) = rows.next().await.map_err(db_err("night topic row"))? {
            let name = row.get::<String>(1).unwrap_or_default();
            if !intake::topic_name_offered(&name) {
                continue;
            }
            let space = crate::space_context::normalize_unfiled_space(
                row.get::<Option<String>>(2).unwrap_or(None),
            );
            if space.as_ref().is_some_and(|s| !spaces.contains(s)) {
                continue;
            }
            let entity_id = row.get::<String>(0).unwrap_or_default();
            topics.insert(
                entity_id.clone(),
                NightTopic {
                    entity_id,
                    name,
                    space,
                },
            );
        }
        Ok(topics)
    }

    /// The topics earlier points already connect to.
    async fn topics_of_points(&self, points: &[PriorPoint]) -> Result<Vec<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut ids = Vec::new();
        for point in points {
            let mut rows = conn
                .query(
                    "SELECT entity_id FROM memory_entities WHERE memory_id = ?1 ORDER BY rowid",
                    libsql::params![point.id.as_str()],
                )
                .await
                .map_err(db_err("night point topics"))?;
            while let Some(row) = rows.next().await.map_err(db_err("night point topic row"))? {
                ids.push(row.get::<String>(0).unwrap_or_default());
            }
        }
        Ok(ids)
    }

    /// Connect a point to a topic, unless the topic has become a hub or the
    /// link is already there. Returns the link when this made it.
    pub async fn link_night_point(
        &self,
        memory: &str,
        entity: &str,
    ) -> Result<Option<NightLink>, WenlanError> {
        if !self.hub_entity_ids(&[entity]).await?.is_empty() {
            return Ok(None);
        }
        let current = {
            let conn = self.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT 1 FROM memory_entities WHERE memory_id = ?1 AND entity_id = ?2",
                    libsql::params![memory, entity],
                )
                .await
                .map_err(db_err("night link exists"))?;
            if rows
                .next()
                .await
                .map_err(db_err("night link exists row"))?
                .is_some()
            {
                return Ok(None);
            }
            drop(rows);
            let mut rows = conn
                .query(
                    "SELECT entity_id FROM memories WHERE source_id = ?1 LIMIT 1",
                    libsql::params![memory],
                )
                .await
                .map_err(db_err("night link memory"))?;
            match rows.next().await.map_err(db_err("night link memory row"))? {
                Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
                None => return Ok(None),
            }
        };
        if current.is_none() {
            self.update_memory_entity_id(memory, entity).await?;
        } else {
            self.link_memory_entities(memory, &[entity]).await?;
        }
        Ok(Some(NightLink {
            memory: memory.to_string(),
            entity: entity.to_string(),
        }))
    }

    /// A thing's standing night points that connect to nothing.
    pub async fn unlinked_night_points(
        &self,
        thing_id: &str,
    ) -> Result<Vec<UnlinkedPoint>, WenlanError> {
        let points = self.dream_points(thing_id).await?;
        let conn = self.conn.lock().await;
        let mut unlinked = Vec::new();
        for point in points {
            let mut rows = conn
                .query(
                    "SELECT 1 FROM memory_entities WHERE memory_id = ?1 LIMIT 1",
                    libsql::params![point.id.as_str()],
                )
                .await
                .map_err(db_err("unlinked night point"))?;
            if rows
                .next()
                .await
                .map_err(db_err("unlinked night point row"))?
                .is_some()
            {
                continue;
            }
            drop(rows);
            let mut rows = conn
                .query(
                    "SELECT space FROM memories WHERE source_id = ?1 LIMIT 1",
                    libsql::params![point.id.as_str()],
                )
                .await
                .map_err(db_err("unlinked night point space"))?;
            let space = match rows
                .next()
                .await
                .map_err(db_err("unlinked night point space row"))?
            {
                Some(row) => crate::space_context::normalize_unfiled_space(
                    row.get::<Option<String>>(0).unwrap_or(None),
                ),
                None => None,
            };
            unlinked.push(UnlinkedPoint {
                id: point.id,
                text: point.text,
                space,
            });
        }
        Ok(unlinked)
    }

    /// Keep the links the one-time connecting made for a thing, so they can be
    /// taken back. This entry is not a night: it writes no point, and the
    /// thing's page and its night undo pass over it.
    pub async fn record_night_links_backfill(
        &self,
        thing_id: &str,
        links: &[NightLink],
    ) -> Result<(), WenlanError> {
        if links.is_empty() {
            return Ok(());
        }
        let changes: Vec<NightChange> = links.iter().map(NightChange::from).collect();
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("night links record begin"))?;
        history(
            &tx,
            now,
            ThingActor::Ai,
            "thing.dream",
            thing_id,
            None,
            serde_json::json!({ "changes": changes, "backfill": true }),
        )
        .await?;
        tx.commit()
            .await
            .map_err(db_err("night links record commit"))?;
        Ok(())
    }

    /// Whether old points were already connected, and how many links it made.
    pub async fn night_links_backfilled(&self) -> Result<Option<String>, WenlanError> {
        self.get_app_metadata(BACKFILL_MARKER).await
    }

    /// Note that the one-time connecting finished.
    pub async fn mark_night_links_backfilled(&self, linked: usize) -> Result<(), WenlanError> {
        self.set_app_metadata(BACKFILL_MARKER, &linked.to_string())
            .await
    }

    /// The person takes back the one-time connecting: every link it recorded
    /// is removed, and it can be run again. Returns how many links it removed.
    /// Links other work made stay.
    pub async fn undo_night_links_backfill(&self) -> Result<usize, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT h.seq, h.thing_id, h.data FROM thing_history h
                  WHERE h.op = 'thing.dream'
                    AND json_extract(h.data, '$.backfill') = 1
                    AND NOT EXISTS (SELECT 1 FROM thing_history u
                                     WHERE u.thing_id = h.thing_id
                                       AND u.op = 'thing.dream_undo'
                                       AND json_extract(u.data, '$.seq') = h.seq)
                  ORDER BY h.seq",
                (),
            )
            .await
            .map_err(db_err("night links backfills"))?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().await.map_err(db_err("night links backfill"))? {
            entries.push((
                row.get::<i64>(0).unwrap_or(0),
                row.get::<String>(1).unwrap_or_default(),
                row.get::<String>(2).unwrap_or_default(),
            ));
        }
        drop(rows);
        let now = chrono::Utc::now().timestamp();
        let tx = conn
            .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
            .await
            .map_err(db_err("night links undo begin"))?;
        let mut removed = 0;
        for (seq, thing_id, data) in entries {
            let changes = serde_json::from_str::<serde_json::Value>(&data)
                .ok()
                .and_then(|entry| entry.get("changes").cloned())
                .and_then(|changes| serde_json::from_value::<Vec<NightChange>>(changes).ok())
                .unwrap_or_default();
            removed += remove_night_links(&tx, &changes).await?;
            history(
                &tx,
                now,
                ThingActor::Person,
                "thing.dream_undo",
                &thing_id,
                None,
                serde_json::json!({ "seq": seq }),
            )
            .await?;
        }
        tx.execute(
            "DELETE FROM app_metadata WHERE key = ?1",
            libsql::params![BACKFILL_MARKER],
        )
        .await
        .map_err(db_err("night links undo marker"))?;
        tx.commit()
            .await
            .map_err(db_err("night links undo commit"))?;
        Ok(removed)
    }

    /// Every thing that may have night points, for the one-time connecting.
    pub async fn things_with_nights(&self) -> Result<Vec<(String, String)>, WenlanError> {
        Ok(self
            .list_things(&ReadScope::Global, None)
            .await?
            .into_iter()
            .filter(|thing| thing.dreamed_at.is_some())
            .map(|thing| (thing.id, thing.title.unwrap_or_default()))
            .collect())
    }
}

impl From<&NightLink> for NightChange {
    fn from(link: &NightLink) -> Self {
        NightChange::Link {
            memory: link.memory.clone(),
            entity: link.entity.clone(),
        }
    }
}

/// Remove the links in a record. A point whose main topic was one of them
/// falls back to the next one it still has, or to none. Returns how many were
/// there to remove.
pub(super) async fn remove_night_links(
    tx: &libsql::Transaction,
    changes: &[NightChange],
) -> Result<usize, WenlanError> {
    let mut removed = 0;
    for change in changes {
        let NightChange::Link { memory, entity } = change else {
            continue;
        };
        removed += tx
            .execute(
                "DELETE FROM memory_entities WHERE memory_id = ?1 AND entity_id = ?2",
                libsql::params![memory.as_str(), entity.as_str()],
            )
            .await
            .map_err(db_err("night link remove"))? as usize;
        tx.execute(
            "UPDATE memories
                SET entity_id = (SELECT me.entity_id FROM memory_entities me
                                  WHERE me.memory_id = ?1 ORDER BY me.rowid LIMIT 1)
              WHERE source_id = ?1 AND entity_id = ?2",
            libsql::params![memory.as_str(), entity.as_str()],
        )
        .await
        .map_err(db_err("night link main topic"))?;
    }
    Ok(removed)
}

fn db_err(what: &'static str) -> impl Fn(libsql::Error) -> WenlanError {
    move |e| WenlanError::VectorDb(format!("{what}: {e}"))
}
