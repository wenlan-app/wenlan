// SPDX-License-Identifier: Apache-2.0
//! Identity-bound positive import provenance and impact-only selection observations.
use super::{commit_or_rollback, MemoryDB};
use crate::WenlanError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use wenlan_types::zotero::{ZoteroDisconnectPreview, ZoteroRegistrationIdentity};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Membership {
    identity: ZoteroRegistrationIdentity,
    revision: u64,
    /// Only a fresh registration stamped before any imports proves complete provenance.
    #[serde(default)]
    impact_scope_known: bool,
    /// Only successful imports can enter this map. False means observed absent.
    imported: BTreeMap<String, (String, bool)>,
    pending_disconnect: bool,
    disconnected: bool,
}
fn sql(e: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("Zotero membership: {e}"))
}
fn key(id: &str) -> String {
    format!("zotero_membership:{id}")
}
async fn load(conn: &libsql::Connection, id: &str) -> Result<Option<Membership>, WenlanError> {
    let mut rows = conn
        .query("SELECT value FROM app_metadata WHERE key=?1", [key(id)])
        .await
        .map_err(sql)?;
    rows.next()
        .await
        .map_err(sql)?
        .map(|r| Ok(serde_json::from_str(&r.get::<String>(0).map_err(sql)?)?))
        .transpose()
}
async fn save(conn: &libsql::Connection, value: &Membership) -> Result<(), WenlanError> {
    conn.execute("INSERT INTO app_metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",libsql::params![key(&value.identity.registration_id),serde_json::to_string(value)?]).await.map_err(sql)?;
    Ok(())
}
fn checked(value: &Membership, identity: &ZoteroRegistrationIdentity) -> Result<(), WenlanError> {
    if &value.identity != identity || value.pending_disconnect || value.disconnected {
        return Err(WenlanError::Conflict(
            "Zotero registration identity changed or disconnect is pending; reconnect explicitly"
                .into(),
        ));
    }
    Ok(())
}
impl MemoryDB {
    /// New bindings never inherit raw sync receipts from an older registration.
    pub async fn bind_zotero_registration(
        &self,
        identity: &ZoteroRegistrationIdentity,
    ) -> Result<(), WenlanError> {
        self.bind_zotero_registration_with_scope(identity, true)
            .await
    }
    /// Existing config/receipt sets have an unknown historical import boundary.
    pub async fn bind_zotero_registration_baseline(
        &self,
        identity: &ZoteroRegistrationIdentity,
    ) -> Result<(), WenlanError> {
        self.bind_zotero_registration_with_scope(identity, false)
            .await
    }
    async fn bind_zotero_registration_with_scope(
        &self,
        identity: &ZoteroRegistrationIdentity,
        known: bool,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        if let Some(value) = load(&conn, &identity.registration_id).await? {
            return checked(&value, identity);
        }
        save(
            &conn,
            &Membership {
                identity: identity.clone(),
                revision: 0,
                impact_scope_known: known && !identity.server_id.is_empty(),
                imported: BTreeMap::new(),
                pending_disconnect: false,
                disconnected: false,
            },
        )
        .await
    }
    pub async fn observe_zotero_selection(
        &self,
        identity: &ZoteroRegistrationIdentity,
        listed: &BTreeSet<String>,
        successful_imports: &BTreeMap<String, String>,
    ) -> Result<(), WenlanError> {
        if identity.server_id.is_empty() {
            return Err(WenlanError::Validation("Legacy unknown Zotero identity cannot record successful membership; reconnect to a stable server".into()));
        }
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result=async {
            let mut value=load(&conn,&identity.registration_id).await?.ok_or_else(||WenlanError::Conflict("Zotero registration is not bound".into()))?;
            checked(&value,identity)?;
            let mut changed=false;
            for (item,(source,present)) in &mut value.imported {
                let observed=listed.contains(item);
                if *present && !observed {
                    super::zotero_scope_impacts::zotero_scope_impact_on(&conn,&identity.space_id,std::slice::from_ref(source),"Zotero item is no longer in the synchronized selection; retained evidence requires review").await?;
                }
                changed|=*present!=observed;
                *present=observed;
            }
            for (item,source) in successful_imports {
                if !listed.contains(item) {return Err(WenlanError::Validation("Imported Zotero item was not in the authoritative listing".into()));}
                let next=(source.clone(),true);
                changed|=value.imported.get(item)!=Some(&next);
                value.imported.insert(item.clone(),next);
            }
            if changed {value.revision+=1;save(&conn,&value).await?;}
            Ok(())
        }.await;
        match result {
            Ok(()) => commit_or_rollback(&conn).await.map(|_| ()).map_err(sql),
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }
    pub async fn zotero_disconnect_preview(
        &self,
        identity: &ZoteroRegistrationIdentity,
    ) -> Result<ZoteroDisconnectPreview, WenlanError> {
        let conn = self.conn.lock().await;
        let value = load(&conn, &identity.registration_id)
            .await?
            .unwrap_or_else(|| Membership {
                identity: identity.clone(),
                revision: 0,
                impact_scope_known: false,
                imported: BTreeMap::new(),
                pending_disconnect: false,
                disconnected: false,
            });
        if &value.identity != identity {
            return Err(WenlanError::Conflict(
                "Zotero registration identity changed".into(),
            ));
        }
        let sources: Vec<String> = value
            .imported
            .values()
            .map(|(source, _)| source.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let pages = super::zotero_scope_impacts::zotero_scope_preview_on(
            &conn,
            &identity.space_id,
            &sources,
        )
        .await?;
        let mut names = conn
            .query(
                "SELECT name FROM spaces WHERE id=?1",
                [identity.space_id.as_str()],
            )
            .await
            .map_err(sql)?;
        let space_name = names
            .next()
            .await
            .map_err(sql)?
            .ok_or_else(|| {
                WenlanError::NotFound("Zotero destination Space no longer exists".into())
            })?
            .get::<String>(0)
            .map_err(sql)?;
        drop(names);
        let identity_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(identity)?));
        // Opaque preview fence also changes when saved affected work changes.
        let mut fence = serde_json::to_vec(&(
            value.revision,
            value.impact_scope_known,
            &pages,
            &space_name,
        ))?;
        for page in &pages {
            let mut rows = conn
                .query(
                    "SELECT incarnation,version,COALESCE(source_revision,0) FROM pages WHERE id=?1",
                    [page.as_str()],
                )
                .await
                .map_err(sql)?;
            let row = rows.next().await.map_err(sql)?.ok_or_else(|| {
                WenlanError::Conflict("Affected Page changed during preview".into())
            })?;
            fence.extend(serde_json::to_vec(&(
                row.get::<String>(0).map_err(sql)?,
                row.get::<i64>(1).map_err(sql)?,
                row.get::<i64>(2).map_err(sql)?,
            ))?);
        }
        let hash = Sha256::digest(fence);
        let revision = u64::from_be_bytes(hash[..8].try_into().expect("eight digest bytes"))
            & ((1u64 << 53) - 1);
        Ok(ZoteroDisconnectPreview {
            impact_scope_known: value.impact_scope_known && !identity.server_id.is_empty(),
            identity_digest,
            space_name,
            identity: identity.clone(),
            revision,
            canonical_item_ids: sources,
            affected_page_ids: pages,
            pending_disconnect: value.pending_disconnect,
            disconnected: value.disconnected,
        })
    }
    /// Commit impact and recoverable intent BEFORE touching config. Retains all receipts/content.
    pub async fn begin_zotero_disconnect(
        &self,
        identity: &ZoteroRegistrationIdentity,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute("BEGIN", ()).await.map_err(sql)?;
        let result = async {
            let mut value = load(&conn, &identity.registration_id)
                .await?
                .ok_or_else(|| WenlanError::Conflict("Zotero registration is not bound".into()))?;
            if &value.identity != identity {
                return Err(WenlanError::Conflict(
                    "Zotero registration identity changed".into(),
                ));
            }
            if value.pending_disconnect || value.disconnected {
                return Ok(());
            }
            let sources: Vec<_> = value
                .imported
                .values()
                .map(|(source, _)| source.clone())
                .collect();
            super::zotero_scope_impacts::zotero_scope_impact_on(
                &conn,
                &identity.space_id,
                &sources,
                "Zotero connection was disabled; retained evidence requires review",
            )
            .await?;
            value.pending_disconnect = true;
            value.revision += 1;
            save(&conn, &value).await
        }
        .await;
        match result {
            Ok(()) => commit_or_rollback(&conn).await.map(|_| ()).map_err(sql),
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }
    pub async fn finish_zotero_disconnect(
        &self,
        identity: &ZoteroRegistrationIdentity,
    ) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        let mut value = load(&conn, &identity.registration_id)
            .await?
            .ok_or_else(|| WenlanError::Conflict("Missing Zotero disconnect intent".into()))?;
        if &value.identity != identity || (!value.pending_disconnect && !value.disconnected) {
            return Err(WenlanError::Conflict(
                "Zotero disconnect identity changed".into(),
            ));
        }
        if !value.disconnected {
            value.pending_disconnect = false;
            value.disconnected = true;
            value.revision += 1;
            save(&conn, &value).await?;
        }
        Ok(())
    }
    /// Durable disconnect intentions remain discoverable after config removal.
    pub async fn pending_zotero_disconnects(
        &self,
    ) -> Result<Vec<ZoteroRegistrationIdentity>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn.query("SELECT key,value FROM app_metadata WHERE substr(key,1,18)='zotero_membership:' ORDER BY key", ()).await.map_err(sql)?;
        let mut identities = Vec::new();
        while let Some(row) = rows.next().await.map_err(sql)? {
            let stored_key = row.get::<String>(0).map_err(sql)?;
            let value: Membership = serde_json::from_str(&row.get::<String>(1).map_err(sql)?)?;
            if stored_key != key(&value.identity.registration_id) {
                return Err(WenlanError::Conflict(
                    "Zotero membership identity does not match its registration key".into(),
                ));
            }
            if value.pending_disconnect && !value.disconnected {
                identities.push(value.identity);
            }
        }
        Ok(identities)
    }
    pub async fn zotero_known_registration(
        &self,
        id: &str,
    ) -> Result<Option<ZoteroRegistrationIdentity>, WenlanError> {
        let conn = self.conn.lock().await;
        Ok(load(&conn, id).await?.map(|value| value.identity))
    }
    pub async fn zotero_retained_registration(
        &self,
        id: &str,
    ) -> Result<Option<ZoteroRegistrationIdentity>, WenlanError> {
        let conn = self.conn.lock().await;
        Ok(load(&conn, id)
            .await?
            .filter(|v| v.pending_disconnect || v.disconnected)
            .map(|v| v.identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use wenlan_types::pages::PageCitation;
    async fn seed(db: &MemoryDB) -> ZoteroRegistrationIdentity {
        let space = db.create_space("Research", None, false).await.unwrap();
        let c = PageCitation {
            occurrence: 1,
            marker: 1,
            source_kind: "memory".into(),
            locator: "paper-a".into(),
            score: 0.8,
            status: "verified".into(),
            scope: "sentence".into(),
            cited_version: Some(1),
            cited_digest: Some("digest".into()),
            disproven: false,
            historical: false,
            pdf_target: None,
        };
        {
            let conn = db.conn.lock().await;
            conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('paper-a','file','paper-a','Paper','Evidence','Research',0,1,'text')",()).await.unwrap();
        }
        db.insert_page_with_kind(
            "review",
            "Human review",
            None,
            "Human conclusion [1].",
            None,
            Some("Research"),
            &["paper-a"],
            &chrono::Utc::now().to_rfc3339(),
            "authored",
            "confirmed",
            Some("Research"),
            Some(&serde_json::to_string(&vec![c]).unwrap()),
        )
        .await
        .unwrap();
        ZoteroRegistrationIdentity {
            registration_id: "A".into(),
            locator: "exact-selection-A".into(),
            server_id: "server".into(),
            space_id: space.id,
        }
    }
    #[tokio::test]
    async fn zotero_selection_absence_is_impact_only_deduplicated_and_survives_auto_recheck() {
        let (db, _temp) = test_db().await;
        let identity = seed(&db).await;
        let old_export = db
            .capture_page_export(
                "review",
                &crate::read_scope::ReadScope::Space("Research".into()),
            )
            .await
            .unwrap();
        let old_bytes = serde_json::to_vec(&old_export).unwrap();
        db.bind_zotero_registration(&identity).await.unwrap();
        let present = BTreeSet::from(["ITEM0001".into()]);
        let imported = BTreeMap::from([("ITEM0001".into(), "paper-a".into())]);
        db.observe_zotero_selection(&identity, &present, &imported)
            .await
            .unwrap();
        let mut b = identity.clone();
        b.registration_id = "B".into();
        b.locator = "B".into();
        db.bind_zotero_registration(&b).await.unwrap();
        db.observe_zotero_selection(&b, &present, &imported)
            .await
            .unwrap();
        db.observe_zotero_selection(&identity, &BTreeSet::new(), &BTreeMap::new())
            .await
            .unwrap();
        let once = db.zotero_disconnect_preview(&identity).await.unwrap();
        db.observe_zotero_selection(&identity, &BTreeSet::new(), &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(once, db.zotero_disconnect_preview(&identity).await.unwrap());
        assert_eq!(once.affected_page_ids, vec!["review"]);
        assert_eq!(
            db.source_lifecycle(&identity.space_id, "file", "paper-a")
                .await
                .unwrap()
                .record
                .revision,
            0,
            "scope loss cannot fabricate lifecycle decisions even if B remains present"
        );
        let page = db.get_page("review").await.unwrap().unwrap();
        assert_eq!(page.content, "Human conclusion [1].");
        assert_eq!(page.stale_reason.as_deref(), Some("source_updated"));
        {
            let conn = db.conn.lock().await;
            let json = serde_json::to_string(
                &page
                    .citations
                    .iter()
                    .cloned()
                    .map(|mut c| {
                        c.status = "verified".into();
                        c.score = 1.0;
                        c
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            conn.execute("UPDATE pages SET citations=?1,stale_reason=NULL,version=version+1 WHERE id='review'",[json]).await.unwrap();
        }
        let reread = db.get_page("review").await.unwrap().unwrap();
        assert_eq!(reread.stale_reason.as_deref(), Some("source_updated"));
        assert_eq!(
            reread.refresh_blocked_reason.as_deref(),
            Some(wenlan_types::zotero::ZOTERO_SELECTION_REVIEW_REQUIRED)
        );
        assert!(
            !reread.review_required,
            "selection loss cannot alter the researcher's user Review lock"
        );
        assert_eq!(reread.citations[0].status, "unevaluated");
        let export = db
            .capture_page_export(
                "review",
                &crate::read_scope::ReadScope::Space("Research".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            export.citations.as_ref().unwrap()[0].status,
            "unevaluated",
            "current export cannot certify raw producer citations while scope review is pending"
        );
        {
            let conn = db.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT value FROM app_metadata WHERE value LIKE ?1",
                    [format!("%{}%", old_export.id)],
                )
                .await
                .unwrap();
            let mut retained = false;
            while let Some(row) = rows.next().await.unwrap() {
                if let Ok(bundle) = serde_json::from_str::<
                    wenlan_types::page_export::PageExportBundle,
                >(&row.get::<String>(0).unwrap())
                {
                    if bundle.id == old_export.id {
                        assert_eq!(serde_json::to_vec(&bundle).unwrap(), old_bytes);
                        retained = true;
                    }
                }
            }
            assert!(retained, "old immutable export remains stored unchanged");
        }
        assert_eq!(
            db.zotero_disconnect_preview(&b)
                .await
                .unwrap()
                .canonical_item_ids,
            vec!["paper-a"]
        );
    }
    #[tokio::test]
    async fn zotero_stable_unbound_receipts_remain_unknown_after_baseline_sync_and_disconnect() {
        let (db, _temp) = test_db().await;
        let identity = seed(&db).await;
        db.upsert_sync_state(&identity.registration_id, "ITEM0001", 0, "legacy")
            .await
            .unwrap();
        let before = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert!(!before.impact_scope_known);
        assert!(before.canonical_item_ids.is_empty());
        db.bind_zotero_registration_baseline(&identity)
            .await
            .unwrap();
        db.observe_zotero_selection(
            &identity,
            &BTreeSet::from(["ITEM0001".into()]),
            &BTreeMap::from([("ITEM0001".into(), "paper-a".into())]),
        )
        .await
        .unwrap();
        assert!(
            !db.zotero_disconnect_preview(&identity)
                .await
                .unwrap()
                .impact_scope_known,
            "a current known subset cannot prove complete historical provenance"
        );
        db.begin_zotero_disconnect(&identity).await.unwrap();
        let pending = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert!(!pending.impact_scope_known);
        db.begin_zotero_disconnect(&identity).await.unwrap();
        assert_eq!(
            pending,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        db.finish_zotero_disconnect(&identity).await.unwrap();
        assert!(
            !db.zotero_disconnect_preview(&identity)
                .await
                .unwrap()
                .impact_scope_known
        );
        assert!(db
            .get_sync_state(&identity.registration_id, "ITEM0001")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn zotero_bound_identity_and_pending_disconnect_retain_receipts_and_prose() {
        let (db, _temp) = test_db().await;
        let identity = seed(&db).await;
        let fresh = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert!(fresh.canonical_item_ids.is_empty());
        assert_eq!(fresh.space_name, "Research");
        assert_eq!(fresh.identity_digest.len(), 64);
        db.bind_zotero_registration(&identity).await.unwrap();
        db.upsert_sync_state("A", "ITEM0001", 0, "content")
            .await
            .unwrap();
        // Legacy raw receipt is not proof of a positive bound import.
        db.observe_zotero_selection(&identity, &BTreeSet::new(), &BTreeMap::new())
            .await
            .unwrap();
        assert!(db
            .zotero_disconnect_preview(&identity)
            .await
            .unwrap()
            .canonical_item_ids
            .is_empty());
        let listed = BTreeSet::from(["ITEM0001".into()]);
        db.observe_zotero_selection(
            &identity,
            &listed,
            &BTreeMap::from([("ITEM0001".into(), "paper-a".into())]),
        )
        .await
        .unwrap();
        let mut wrong = identity.clone();
        wrong.locator = "another selection".into();
        assert!(db.bind_zotero_registration(&wrong).await.is_err());
        assert!(db
            .observe_zotero_selection(&wrong, &BTreeSet::new(), &BTreeMap::new())
            .await
            .is_err());
        db.begin_zotero_disconnect(&identity).await.unwrap();
        let pending = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert!(pending.pending_disconnect);
        db.begin_zotero_disconnect(&identity).await.unwrap();
        assert_eq!(
            pending,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        assert!(db
            .observe_zotero_selection(&identity, &listed, &BTreeMap::new())
            .await
            .is_err());
        assert!(db.get_sync_state("A", "ITEM0001").await.unwrap().is_some());
        assert_eq!(
            db.get_page("review").await.unwrap().unwrap().content,
            "Human conclusion [1]."
        );
        db.finish_zotero_disconnect(&identity).await.unwrap();
        let done = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert!(done.disconnected);
        db.finish_zotero_disconnect(&identity).await.unwrap();
        assert_eq!(done, db.zotero_disconnect_preview(&identity).await.unwrap());
        assert!(db.bind_zotero_registration(&identity).await.is_err());
    }
}
