// SPDX-License-Identifier: Apache-2.0
//! The host's writer for versions, proposals, and decisions from team sync.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use libsql::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use wenlan_types::team::{MemberSyncRefusal, MemberSyncRefusalReason as Reason};

use super::{scoped_pages::shared_page_predicate, MemberPageTarget, MemoryDB};
use crate::export::knowledge::MemberSyncProjection;
use crate::pages::Page;
use crate::read_scope::ReadScope;
use crate::shared_space::{version, writer_edited_by, MemberViewReceipt, OWN_DEVICE_PERSON};
use crate::WenlanError;
use version::PageVersion;

#[cfg(test)]
#[path = "member_sync_test.rs"]
mod tests;

pub(crate) struct MemberSyncWriter<'a> {
    pub db: &'a MemoryDB,
    pub space: &'a str,
    pub member_id: &'a str,
    pub settings_path: &'a Path,
    pub knowledge_path: Option<&'a Path>,
}

/// The parent page and its conflict copy are committed together.
pub(crate) enum MemberSyncConflict<'a> {
    Merge {
        title: &'a str,
        body: &'a str,
    },
    Copy {
        id: &'a str,
        version: &'a PageVersion,
        name: &'a str,
    },
    Notice,
}

pub(crate) enum MemberSyncOperation<'a> {
    Create,
    Edit {
        preceding: &'a [&'a PageVersion],
        conflict: Option<MemberSyncConflict<'a>>,
    },
    Stage,
    Decide {
        proposal: &'a PageVersion,
        card: Option<(&'a str, Option<&'a str>)>,
    },
}

pub(crate) struct MemberSyncInput<'a> {
    pub change: &'a PageVersion,
    pub page: Option<&'a Page>,
    pub history: &'a [PageVersion],
    pub operation: MemberSyncOperation<'a>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Receipt {
    Applied {
        page: String,
        stay: String,
        card: Option<String>,
    },
    Refused(MemberSyncRefusal),
}

pub(crate) enum MemberSyncOutcome {
    Applied {
        version: i64,
        wrote: bool,
        card: Option<String>,
    },
    Refused,
}

#[derive(Debug, thiserror::Error)]
enum CheckError {
    #[error("member sync refused: {0:?}")]
    Refused(Reason),
    #[error(transparent)]
    Storage(#[from] WenlanError),
}

impl From<Reason> for CheckError {
    fn from(reason: Reason) -> Self {
        Self::Refused(reason)
    }
}

impl From<libsql::Error> for CheckError {
    fn from(error: libsql::Error) -> Self {
        Self::Storage(storage(error))
    }
}

fn storage(error: libsql::Error) -> WenlanError {
    WenlanError::VectorDb(format!("member sync: {error}"))
}

fn space_prefix(space_id: &str) -> String {
    format!(
        "shared_space.member_sync:{}:",
        version::sha256_hex(space_id.as_bytes())
    )
}

fn receipt_prefix(space_id: &str, member: &str) -> String {
    format!(
        "{}{}:",
        space_prefix(space_id),
        version::sha256_hex(member.as_bytes())
    )
}

async fn space_id(conn: &Connection, space: &str) -> Result<Option<String>, WenlanError> {
    conn.query("SELECT id FROM spaces WHERE name=?1", [space])
        .await
        .map_err(storage)?
        .next()
        .await
        .map_err(storage)?
        .map(|row| row.get(0).map_err(storage))
        .transpose()
}

async fn read_value(conn: &Connection, key: &str) -> Result<Option<String>, WenlanError> {
    conn.query("SELECT value FROM app_metadata WHERE key=?1", [key])
        .await
        .map_err(storage)?
        .next()
        .await
        .map_err(storage)?
        .map(|row| row.get(0).map_err(storage))
        .transpose()
}

fn receipt_key(space_id: &str, change: &PageVersion) -> String {
    format!(
        "{}{}",
        receipt_prefix(space_id, &change.meta.author.member),
        change.sha
    )
}

async fn record(conn: &Connection, key: &str, receipt: &Receipt) -> Result<(), WenlanError> {
    conn.execute(
        "INSERT INTO app_metadata(key,value) VALUES (?1,?2)
          ON CONFLICT(key) DO UPDATE SET value=excluded.value
            WHERE ?3=1 AND json_extract(app_metadata.value,'$.Applied') IS NOT NULL",
        params![
            key,
            serde_json::to_string(receipt)?,
            i64::from(matches!(receipt, Receipt::Applied { .. }))
        ],
    )
    .await
    .map_err(storage)?;
    Ok(())
}

impl MemberSyncWriter<'_> {
    /// Ingests one version (with its intervening history and conflict result),
    /// proposal, or decision. Only the refusal receipt is written on refusal.
    pub(crate) async fn write(
        &self,
        input: MemberSyncInput<'_>,
    ) -> Result<MemberSyncOutcome, WenlanError> {
        let mut authors = HashSet::from([input.change.meta.author.member.as_str()]);
        match &input.operation {
            MemberSyncOperation::Edit {
                preceding,
                conflict,
            } => {
                authors.extend(
                    preceding
                        .iter()
                        .map(|version| version.meta.author.member.as_str()),
                );
                if let Some(MemberSyncConflict::Copy { version, .. }) = conflict {
                    authors.insert(&version.meta.author.member);
                }
            }
            MemberSyncOperation::Decide { proposal, .. } => {
                authors.insert(&proposal.meta.author.member);
            }
            _ => {}
        }
        authors.remove(self.member_id);
        let team_path = crate::rulesync_team::team_path(self.settings_path);
        let _authority = if authors.is_empty() {
            None
        } else {
            let authors: Vec<_> = authors.into_iter().map(str::to_string).collect();
            let space = self.space.to_string();
            let verified = tokio::task::spawn_blocking(move || {
                let lock = crate::rulesync_team::authority_lock(&team_path)?;
                let team = crate::team::read_team(&team_path)?;
                let allowed = authors.iter().all(|author| {
                    team.members.iter().any(|member| {
                        &member.id == author
                            && member.access(&space) == Some(wenlan_types::team::TeamAccess::Write)
                    })
                });
                Ok::<_, WenlanError>(allowed.then_some(lock))
            })
            .await
            .map_err(|error| WenlanError::Generic(format!("member sync authority: {error}")))?;
            match verified {
                Ok(Some(lock)) => Some(lock),
                Ok(None) => return self.refuse(input.change, Reason::PageUnavailable).await,
                Err(error) => {
                    log::warn!("[shared_space] member sync authority unavailable: {error}");
                    return self.refuse(input.change, Reason::Unverifiable).await;
                }
            }
        };
        let mut projection = match self.knowledge_path {
            Some(root) => match MemberSyncProjection::begin(root, self.db) {
                Ok(session) => Some(session),
                Err(error) => {
                    log::warn!("[shared_space] member sync projection unavailable: {error}");
                    return self.refuse(input.change, Reason::OutsideSpaceFolder).await;
                }
            },
            None => None,
        };
        let conn = self.db.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
            .map_err(storage)?;
        let Some(space_id) = space_id(&tx, self.space).await? else {
            return Ok(MemberSyncOutcome::Refused);
        };
        let key = receipt_key(&space_id, input.change);
        if let Some(value) = read_value(&tx, &key).await? {
            match serde_json::from_str::<Receipt>(&value)? {
                Receipt::Refused(_) => return Ok(MemberSyncOutcome::Refused),
                Receipt::Applied { page, stay, card } => {
                    let scope = ReadScope::Space(self.space.to_string());
                    if !MemoryDB::member_page_access_on(
                        &tx,
                        &page,
                        &scope,
                        MemberPageTarget::Published,
                    )
                    .await?
                    {
                        return Ok(MemberSyncOutcome::Refused);
                    }
                    let Some(current) = MemoryDB::get_page_on(&tx, &page, false).await? else {
                        return Ok(MemberSyncOutcome::Refused);
                    };
                    if current_stay(&tx, &current, &space_id)
                        .await
                        .map_err(|error| WenlanError::Generic(error.to_string()))?
                        != stay
                    {
                        return Ok(MemberSyncOutcome::Refused);
                    }
                    if !matches!(input.operation, MemberSyncOperation::Stage) {
                        return Ok(applied(&current, false, card));
                    }
                }
            }
        }

        match self
            .write_checked(&tx, &space_id, projection.as_mut(), &input)
            .await
        {
            Ok((outcome, stay)) => {
                let MemberSyncOutcome::Applied { card, .. } = &outcome else {
                    unreachable!("a checked write returns refusal through CheckError")
                };
                record(
                    &tx,
                    &key,
                    &Receipt::Applied {
                        page: input.change.page.id.clone().unwrap_or_default(),
                        stay,
                        card: card.clone(),
                    },
                )
                .await?;
                tx.commit().await.map_err(storage)?;
                if let Some(projection) = projection {
                    projection.keep();
                }
                Ok(outcome)
            }
            Err(error) => {
                tx.rollback().await.map_err(storage)?;
                drop(projection);
                let reason = match error {
                    CheckError::Refused(reason) => reason,
                    CheckError::Storage(error) => {
                        log::warn!("[shared_space] member sync write refused: {error}");
                        Reason::Unverifiable
                    }
                };
                record(&conn, &key, &refusal(input.change, reason)).await?;
                Ok(MemberSyncOutcome::Refused)
            }
        }
    }

    pub(crate) async fn refuse(
        &self,
        change: &PageVersion,
        reason: Reason,
    ) -> Result<MemberSyncOutcome, WenlanError> {
        let conn = self.db.conn.lock().await;
        if let Some(id) = space_id(&conn, self.space).await? {
            record(&conn, &receipt_key(&id, change), &refusal(change, reason)).await?;
        }
        Ok(MemberSyncOutcome::Refused)
    }

    pub(crate) async fn terminal_changes(&self) -> Result<HashSet<String>, WenlanError> {
        let conn = self.db.conn.lock().await;
        let Some(id) = space_id(&conn, self.space).await? else {
            return Ok(HashSet::new());
        };
        let mut rows = conn
            .query(
                "SELECT value FROM app_metadata WHERE key GLOB ?1
             AND CASE WHEN json_valid(value) THEN json_type(value,'$.Refused')='object' ELSE 0 END",
                [format!("{}*", space_prefix(&id))],
            )
            .await
            .map_err(storage)?;
        let mut refused = HashSet::new();
        while let Some(row) = rows.next().await.map_err(storage)? {
            let value = row.get::<String>(0).map_err(storage)?;
            if let Receipt::Refused(receipt) = serde_json::from_str(&value)? {
                refused.insert(receipt.change);
            }
        }
        Ok(refused)
    }

    /// Keep a hosted review export in the same continuous stay as its
    /// caller's card or page snapshot. A move cannot interleave with the
    /// transport file publication.
    pub(crate) async fn publish_current_review(
        &self,
        page: &Page,
        stay: &str,
        folder: &Path,
        member: &str,
        kind: &str,
        text: &str,
    ) -> Result<Option<String>, WenlanError> {
        let conn = self.db.conn.lock().await;
        let scope = ReadScope::Space(self.space.to_owned());
        let Some(space_id) = space_id(&conn, self.space).await? else {
            return Ok(None);
        };
        if !MemoryDB::member_page_access_on(&conn, &page.id, &scope, MemberPageTarget::Published)
            .await?
        {
            return Ok(None);
        }
        let Some(current) = MemoryDB::get_page_on(&conn, &page.id, true).await? else {
            return Ok(None);
        };
        if current.version != page.version
            || current.status != page.status
            || current.review_required != page.review_required
            || current.title != page.title
            || current.content != page.content
            || current_stay(&conn, &current, &space_id)
                .await
                .map_err(|error| WenlanError::Generic(error.to_string()))?
                != stay
        {
            return Ok(None);
        }
        let (sha, _) = version::write_file(folder, member, kind, &page.id, text)?;
        Ok(Some(sha))
    }

    async fn write_checked(
        &self,
        conn: &Connection,
        space_id: &str,
        mut projection: Option<&mut MemberSyncProjection>,
        input: &MemberSyncInput<'_>,
    ) -> Result<(MemberSyncOutcome, String), CheckError> {
        let change = input.change;
        let id = change
            .page
            .id
            .as_deref()
            .filter(|id| version::is_safe_id(id) && !crate::rulesync_team::is_master_page(id))
            .ok_or(Reason::PageUnavailable)?;
        if input.page.is_none() && MemoryDB::shared_page_was_deleted_on(conn, id).await? {
            return Err(Reason::PageUnavailable.into());
        }
        check_cutover(conn, projection.is_some()).await?;
        let scope = ReadScope::Space(self.space.to_string());
        let shared = visible_pages(conn, self.space, false).await?;
        let current = match input.page {
            Some(expected) => {
                if !MemoryDB::member_page_access_on(conn, id, &scope, MemberPageTarget::Published)
                    .await?
                {
                    return Err(Reason::PageUnavailable.into());
                }
                let page = MemoryDB::get_page_on(conn, id, true)
                    .await?
                    .ok_or(Reason::PageUnavailable)?;
                if page.id != expected.id
                    || page.version != expected.version
                    || page.review_required != expected.review_required
                    || page.title != expected.title
                    || page.content != expected.content
                    || page.status != expected.status
                {
                    return Err(Reason::PageChanged.into());
                }
                Some(page)
            }
            None if matches!(input.operation, MemberSyncOperation::Create) => {
                if exists(conn, id).await? {
                    return Err(Reason::PageUnavailable.into());
                }
                None
            }
            None => return Err(Reason::PageUnavailable.into()),
        };
        let stay = match &current {
            Some(page) => Some(current_stay(conn, page, space_id).await?),
            None => None,
        };
        self.check_ancestry(conn, space_id, id, stay.as_deref(), &shared, input)
            .await?;
        let mut visible = visible_pages(conn, self.space, true).await?;

        let initial = initial_folder(conn, id).await?;
        let folder = if let Some(projection) = projection.as_deref_mut() {
            let spaces = MemoryDB::space_names_on(conn).await?;
            let folder = projection
                .space_folder(self.space, spaces, self.settings_path)
                .map_err(projection_refusal)?;
            projection
                .check(id, current.as_ref(), &initial, &folder)
                .map_err(projection_refusal)?;
            Some(folder)
        } else {
            None
        };

        let outcome = match (&input.operation, current) {
            (MemberSyncOperation::Create, None) => {
                if change.meta.archived {
                    return Err(Reason::InvalidAncestry.into());
                }
                let title =
                    available_title(&visible, id, &change.page.title, &change.meta.author.member)?;
                self.insert_page(conn, id, &title, change).await?;
                if !MemoryDB::member_page_access_on(conn, id, &scope, MemberPageTarget::Published)
                    .await?
                {
                    return Err(Reason::PageUnavailable.into());
                }
                let page = read_page(conn, id).await?;
                if let (Some(projection), Some(folder)) = (projection.as_deref_mut(), &folder) {
                    projection
                        .project(&page, folder, &initial)
                        .map_err(projection_refusal)?;
                }
                visible.insert(id.to_string(), title);
                refresh_links(conn, &page, &visible).await?;
                return Ok((
                    applied(&page, true, None),
                    current_stay(conn, &page, space_id).await?,
                ));
            }
            (
                MemberSyncOperation::Edit {
                    preceding,
                    conflict,
                },
                Some(mut page),
            ) => {
                let original = page.clone();
                for version in preceding.iter().copied().chain(std::iter::once(change)) {
                    let edited_by = writer_edited_by(&version.meta.author, self.member_id);
                    let title = if page.status == "archived" && version.meta.archived {
                        page.title.clone()
                    } else {
                        available_title(
                            &visible,
                            id,
                            &version.page.title,
                            &version.meta.author.member,
                        )?
                    };
                    let status = if version.meta.archived {
                        "archived"
                    } else {
                        "active"
                    };
                    page = apply_page(
                        conn,
                        page,
                        &title,
                        &version.page.body,
                        status,
                        version.meta.locked,
                        &edited_by,
                    )
                    .await?;
                }
                match conflict {
                    Some(MemberSyncConflict::Copy {
                        id: copy_id,
                        version,
                        name,
                    }) => {
                        if exists(conn, copy_id).await?
                            || MemoryDB::shared_page_was_deleted_on(conn, copy_id).await?
                        {
                            return Err(Reason::PageUnavailable.into());
                        }
                        let title = super::super::shared_space::free_name(
                            &title_keys(&visible, copy_id),
                            &version.page.title,
                            name,
                        );
                        if let (Some(projection), Some(folder)) =
                            (projection.as_deref_mut(), &folder)
                        {
                            projection
                                .check(copy_id, None, "", folder)
                                .map_err(projection_refusal)?;
                        }
                        self.insert_page(conn, copy_id, &title, version).await?;
                        if !MemoryDB::member_page_access_on(
                            conn,
                            copy_id,
                            &scope,
                            MemberPageTarget::Published,
                        )
                        .await?
                        {
                            return Err(Reason::PageUnavailable.into());
                        }
                        let copy = read_page(conn, copy_id).await?;
                        if let (Some(projection), Some(folder)) =
                            (projection.as_deref_mut(), &folder)
                        {
                            projection
                                .project(&copy, folder, "")
                                .map_err(projection_refusal)?;
                        }
                        refresh_links(conn, &copy, &visible).await?;
                        mark_concurrent(conn, &page, original.version).await?;
                    }
                    Some(MemberSyncConflict::Merge { title, body }) => {
                        let title =
                            available_title(&visible, id, title, &change.meta.author.member)?;
                        let locked = page.review_required;
                        page = apply_page(
                            conn,
                            page,
                            &title,
                            body,
                            "active",
                            locked,
                            OWN_DEVICE_PERSON,
                        )
                        .await?;
                    }
                    Some(MemberSyncConflict::Notice) => {
                        mark_concurrent(conn, &page, original.version).await?
                    }
                    None => {}
                }
                let prose_changed = page.version != original.version;
                if prose_changed {
                    if let (Some(projection), Some(folder)) = (projection, &folder) {
                        projection
                            .project(&page, folder, &initial)
                            .map_err(projection_refusal)?;
                    }
                    if page.status == "active" {
                        visible.insert(id.to_string(), page.title.clone());
                        refresh_links(conn, &page, &visible).await?;
                    }
                }
                applied(
                    &page,
                    prose_changed || page.review_required != original.review_required,
                    None,
                )
            }
            (MemberSyncOperation::Stage, Some(page)) => {
                if page.status != "active" || !page.review_required {
                    return Err(Reason::ReviewUnavailable.into());
                }
                let card = stage_card(
                    conn,
                    &page,
                    change,
                    stay.as_deref().ok_or(Reason::InvalidAncestry)?,
                    self.member_id,
                )
                .await?;
                applied(&page, true, Some(card))
            }
            (MemberSyncOperation::Decide { proposal, card }, Some(page)) => {
                if let Some((card_id, card_stay)) = card.as_ref() {
                    if card_stay.as_deref() != stay.as_deref() {
                        return Err(Reason::ReviewUnavailable.into());
                    }
                    close_card(
                        conn,
                        &page,
                        card_id,
                        &proposal.sha,
                        self.space,
                        stay.as_deref().ok_or(Reason::InvalidAncestry)?,
                    )
                    .await?;
                }
                applied(&page, card.is_some(), None)
            }
            _ => return Err(Reason::Unverifiable.into()),
        };
        Ok((outcome, stay.ok_or(Reason::InvalidAncestry)?))
    }

    async fn check_ancestry(
        &self,
        conn: &Connection,
        space_id: &str,
        id: &str,
        stay: Option<&str>,
        visible: &HashMap<String, String>,
        input: &MemberSyncInput<'_>,
    ) -> Result<(), CheckError> {
        let valid = |version: &PageVersion| {
            version.page.id.as_deref() == Some(id) && version.meta.stay.as_deref() == stay
        };
        let page_version =
            |version: &PageVersion| version::file_kind(&version.meta) == version::PAGES_DIR;
        let mut hashes = HashSet::new();
        if let Some(stay) = stay {
            let key = format!("shared_space.member_view_receipts:{}", self.space);
            if let Some(value) = read_value(conn, &key).await? {
                let receipts: HashMap<String, MemberViewReceipt> =
                    serde_json::from_str(&value).map_err(WenlanError::from)?;
                if let Some(receipt) = receipts.get(id).filter(|receipt| receipt.stay == stay) {
                    hashes.extend(
                        input
                            .history
                            .iter()
                            .filter(|version| {
                                valid(version)
                                    && page_version(version)
                                    && receipt.hashes.contains(&version.sha)
                            })
                            .map(|version| version.sha.clone()),
                    );
                }
            }
            // This pass can publish its host root before it stores the
            // manifest. The held hash is a valid root only in this exact stay.
            let key = format!("shared_space.held:{}", self.space);
            if let Some(value) = read_value(conn, &key).await? {
                let held: serde_json::Value =
                    serde_json::from_str(&value).map_err(WenlanError::from)?;
                if let Some(root) = held.get(id).and_then(|entry| {
                    entry["version"].as_i64()?;
                    entry["sha"].as_str()
                }) {
                    if input.history.iter().any(|version| {
                        version.sha == root && valid(version) && page_version(version)
                    }) {
                        hashes.insert(root.to_string());
                    }
                }
            }
        }
        let mut pending = Vec::new();
        for version in input
            .history
            .iter()
            .filter(|version| valid(version) && page_version(version))
        {
            match read_value(conn, &receipt_key(space_id, version)).await? {
                Some(value) => {
                    match serde_json::from_str::<Receipt>(&value).map_err(WenlanError::from)? {
                        Receipt::Refused(_) => {
                            hashes.remove(&version.sha);
                        }
                        Receipt::Applied {
                            page,
                            stay: recorded,
                            ..
                        } => {
                            if page == id && Some(recorded.as_str()) == stay {
                                hashes.insert(version.sha.clone());
                            }
                        }
                    }
                }
                None => pending.push(version),
            }
        }

        // Version edges are checked in their original direction. All
        // parents, including merge parents, must already be anchored.
        let new_author = &input.change.meta.author.member;
        loop {
            let before = hashes.len();
            for node in &pending {
                if stay.is_none() && &node.meta.author.member != new_author {
                    continue;
                }
                if rooted(node, &hashes, stay.is_none()) {
                    hashes.insert(node.sha.clone());
                }
            }
            if hashes.len() == before {
                break;
            }
        }
        let change = input.change;
        let member_snapshot =
            |version: &PageVersion| crate::member_view::MemberSnapshot::matches(version, visible);
        let verified = valid(change)
            && member_snapshot(change)
            && match &input.operation {
                MemberSyncOperation::Create | MemberSyncOperation::Edit { .. } => {
                    page_version(change) && hashes.contains(&change.sha)
                }
                MemberSyncOperation::Stage => {
                    version::file_kind(&change.meta) == version::PROPOSALS_DIR
                        && rooted(change, &hashes, false)
                }
                MemberSyncOperation::Decide { proposal, .. } => {
                    version::file_kind(&change.meta) == version::DECISIONS_DIR
                        && valid(proposal)
                        && member_snapshot(proposal)
                        && version::file_kind(&proposal.meta) == version::PROPOSALS_DIR
                        && rooted(proposal, &hashes, false)
                        && rooted(change, &hashes, true)
                        && change
                            .meta
                            .decides
                            .as_ref()
                            .is_some_and(|decision| decision.proposal == proposal.sha)
                }
            };
        let usable = |version: &PageVersion| {
            valid(version)
                && page_version(version)
                && member_snapshot(version)
                && hashes.contains(&version.sha)
        };
        let preceding_is_verified = match &input.operation {
            MemberSyncOperation::Edit {
                preceding,
                conflict,
            } => {
                preceding.iter().copied().all(usable)
                    && match conflict {
                        Some(MemberSyncConflict::Copy { id, version, .. }) => {
                            version::is_safe_id(id)
                                && !crate::rulesync_team::is_master_page(id)
                                && usable(version)
                        }
                        _ => true,
                    }
            }
            _ => true,
        };
        if !verified || !preceding_is_verified {
            return Err(Reason::InvalidAncestry.into());
        }
        Ok(())
    }
}

fn rooted(version: &PageVersion, hashes: &HashSet<String>, allow_root: bool) -> bool {
    version
        .meta
        .parent
        .as_ref()
        .map_or(allow_root, |parent| hashes.contains(parent))
        && version
            .meta
            .merged
            .iter()
            .all(|parent| hashes.contains(parent))
}

fn projection_refusal(error: WenlanError) -> CheckError {
    log::warn!("[shared_space] member sync projection refused: {error}");
    Reason::OutsideSpaceFolder.into()
}

fn applied(page: &Page, wrote: bool, card: Option<String>) -> MemberSyncOutcome {
    MemberSyncOutcome::Applied {
        version: page.version,
        wrote,
        card,
    }
}

fn refusal(change: &PageVersion, reason: Reason) -> Receipt {
    Receipt::Refused(MemberSyncRefusal {
        page_id: change.page.id.clone().unwrap_or_default(),
        change: change.sha.clone(),
        reason,
    })
}

impl MemberSyncWriter<'_> {
    async fn insert_page(
        &self,
        conn: &Connection,
        id: &str,
        title: &str,
        change: &PageVersion,
    ) -> Result<(), CheckError> {
        let body = crate::export::provenance::validate_canonical_page_content(&change.page.body)
            .map_err(|_| Reason::Unverifiable)?;
        let now = chrono::Utc::now();
        let edited_by = writer_edited_by(&change.meta.author, self.member_id);
        let changelog = super::append_changelog_entry(
            "[]",
            serde_json::json!({
                "version": 1, "at": now.timestamp(), "edited_by": edited_by,
            }),
            20,
        )?;
        let kind = crate::pages::page_kind_for(title, "authored", "active");
        conn.execute(
            "INSERT INTO pages (
                id,title,content,space,workspace,source_memory_ids,version,status,
                created_at,last_compiled,last_modified,creation_kind,review_status,kind,
                incarnation,changelog,review_required
             ) VALUES (?1,?2,?3,?4,?4,'[]',1,'active',?5,?5,?5,'authored',
                       'confirmed',?6,lower(hex(randomblob(16))),?7,?8)",
            params![
                id,
                title,
                body,
                self.space,
                now.to_rfc3339(),
                kind,
                changelog,
                i64::from(change.meta.locked)
            ],
        )
        .await?;
        append_history(conn, id, &edited_by, now.timestamp()).await
    }
}

impl MemoryDB {
    /// Refusals for the authenticated member in the requested stable Space.
    pub async fn member_sync_refusals(
        &self,
        space: &str,
        member: &str,
    ) -> Result<Vec<MemberSyncRefusal>, WenlanError> {
        let conn = self.conn.lock().await;
        let Some(id) = space_id(&conn, space).await? else {
            return Ok(Vec::new());
        };
        let pattern = format!("{}*", receipt_prefix(&id, member));
        let mut rows = conn
            .query(
                "SELECT value FROM app_metadata WHERE key GLOB ?1 ORDER BY key",
                [pattern],
            )
            .await
            .map_err(storage)?;
        let mut refused = Vec::new();
        while let Some(row) = rows.next().await.map_err(storage)? {
            if let Receipt::Refused(receipt) =
                serde_json::from_str(&row.get::<String>(0).map_err(storage)?)?
            {
                refused.push(receipt);
            }
        }
        Ok(refused)
    }
}

async fn check_cutover(conn: &Connection, projects: bool) -> Result<(), CheckError> {
    if let Some(raw) = read_value(conn, super::TRUTH_CUTOVER_FENCE_KEY).await? {
        let fence = super::CutoverFence::parse(raw.trim()).ok_or(Reason::Unverifiable)?;
        if fence.phase == super::CutoverPhase::Preparing {
            return Err(Reason::Unverifiable.into());
        }
    }
    if projects
        && read_value(conn, super::truth_exposure::TRUTH_CUTOVER_GENERATION_KEY)
            .await?
            .is_some_and(|raw| raw.trim().parse::<i64>() != Ok(0))
    {
        // The ordinary projection gate uses current truth permits. When that
        // gate is active, this writer cannot safely publish a mixed write.
        return Err(Reason::Unverifiable.into());
    }
    Ok(())
}

async fn read_page(conn: &Connection, id: &str) -> Result<Page, CheckError> {
    MemoryDB::get_page_on(conn, id, true)
        .await?
        .ok_or(Reason::PageUnavailable.into())
}

async fn exists(conn: &Connection, id: &str) -> Result<bool, libsql::Error> {
    Ok(conn
        .query("SELECT 1 FROM pages WHERE id=?1", [id])
        .await?
        .next()
        .await?
        .is_some())
}

async fn current_stay(
    conn: &Connection,
    page: &Page,
    space_id: &str,
) -> Result<String, CheckError> {
    let mut rows = conn.query(
        "SELECT incarnation,
                COALESCE((SELECT MAX(version) FROM page_history WHERE page_id=p.id AND edited_by='space_move'),0)
         FROM pages p WHERE id=?1",
        [page.id.as_str()],
    ).await?;
    let row = rows.next().await?.ok_or(Reason::PageUnavailable)?;
    let incarnation: String = row.get(0)?;
    Ok(version::stay_token(&incarnation, space_id, row.get(1)?))
}

async fn initial_folder(conn: &Connection, id: &str) -> Result<String, libsql::Error> {
    conn.query(
        "SELECT folder_path FROM page_draft_create_requests WHERE page_id=?1",
        [id],
    )
    .await?
    .next()
    .await?
    .map_or(Ok(String::new()), |row| row.get(0))
}

async fn visible_pages(
    conn: &Connection,
    space: &str,
    active: bool,
) -> Result<HashMap<String, String>, libsql::Error> {
    let shared = shared_page_predicate("p.");
    let mut rows = conn.query(
        &format!("SELECT id,title FROM pages p WHERE workspace=?1 AND (?2=0 OR status='active') AND {shared}"),
        params![space, i64::from(active)],
    ).await?;
    let mut visible = HashMap::new();
    while let Some(row) = rows.next().await? {
        visible.insert(row.get(0)?, row.get(1)?);
    }
    Ok(visible)
}

fn title_keys(visible: &HashMap<String, String>, id: &str) -> HashSet<String> {
    visible
        .iter()
        .filter(|(page, _)| page.as_str() != id)
        .map(|(_, title)| MemoryDB::page_title_key(title))
        .collect()
}

fn available_title(
    visible: &HashMap<String, String>,
    id: &str,
    wanted: &str,
    name: &str,
) -> Result<String, CheckError> {
    let wanted =
        super::page_rename::validate_page_title(wanted).map_err(|_| Reason::Unverifiable)?;
    let taken = title_keys(visible, id);
    if taken.contains(&MemoryDB::page_title_key(&wanted)) {
        Ok(crate::shared_space::free_name(&taken, &wanted, name))
    } else {
        Ok(wanted)
    }
}

fn checked_body(current: &Page, body: &str) -> Result<String, CheckError> {
    let body = crate::export::provenance::validate_canonical_page_content(body)
        .map_err(|_| Reason::Unverifiable)?;
    if current.content == body || current.citations.is_empty() {
        return Ok(body.to_string());
    }
    let projected = crate::citations::strip_member_citation_markers(body);
    let (restored, citations) = crate::citations::restore_unchanged_member_sentence_markers(
        &current.content,
        &projected,
        &current.citations,
    )
    .map_err(|_| Reason::CitationsWouldChange)?
    .ok_or(Reason::CitationsWouldChange)?;
    if serde_json::to_value(&citations).map_err(WenlanError::from)?
        != serde_json::to_value(&current.citations).map_err(WenlanError::from)?
    {
        return Err(Reason::CitationsWouldChange.into());
    }
    Ok(restored)
}

async fn apply_page(
    conn: &Connection,
    page: Page,
    title: &str,
    body: &str,
    status: &str,
    locked: bool,
    edited_by: &str,
) -> Result<Page, CheckError> {
    let body = checked_body(&page, body)?;
    if body == page.content && title == page.title && status == page.status {
        if locked != page.review_required {
            conn.execute(
                "UPDATE pages SET review_required=?2 WHERE id=?1",
                params![page.id.as_str(), i64::from(locked)],
            )
            .await?;
        }
        return read_page(conn, &page.id).await;
    }

    let raw_citations: Option<String> = conn
        .query(
            "SELECT citations FROM pages WHERE id=?1",
            [page.id.as_str()],
        )
        .await?
        .next()
        .await?
        .ok_or(Reason::PageUnavailable)?
        .get::<Option<String>>(0)?;
    if let Some(raw) = raw_citations {
        // Refuse unreadable or differently mapped records. The UPDATE keeps
        // the stored map byte-for-byte, so it must describe the same citations.
        let records: Vec<wenlan_types::pages::PageCitation> =
            serde_json::from_str(&raw).map_err(|_| Reason::CitationsWouldChange)?;
        if serde_json::to_value(&records).map_err(WenlanError::from)?
            != serde_json::to_value(&page.citations).map_err(WenlanError::from)?
        {
            return Err(Reason::CitationsWouldChange.into());
        }
        let restored = Page {
            citations: records,
            ..page.clone()
        };
        if checked_body(&restored, &body)? != body {
            return Err(Reason::CitationsWouldChange.into());
        }
    } else if !page.citations.is_empty() {
        // Lifecycle-only citation maps are version-bound. Keep the page
        // unchanged until the host can materialize the map.
        return Err(Reason::CitationsWouldChange.into());
    }

    let now = chrono::Utc::now();
    let prose_changed = body != page.content;
    let changed_by = match (page.status.as_str(), status) {
        ("active", "archived") => "archive",
        ("archived", "active") => "restore",
        _ => edited_by,
    };
    let changelog = super::append_changelog_entry(
        page.changelog.as_deref().unwrap_or("[]"),
        serde_json::json!({
            "version": page.version + 1, "edited_by": changed_by, "at": now.timestamp(),
        }),
        20,
    )?;
    let kind = crate::pages::page_kind_for(title, &page.creation_kind, status);
    conn.execute(
        "UPDATE pages SET title=?2,content=?3,status=?4,kind=?5,review_required=?6,
                          version=version+1,last_modified=?7,changelog=?8,
                          embedding=CASE WHEN ?9 OR title<>?2 THEN NULL ELSE embedding END,
                          last_compiled=CASE WHEN ?9 THEN ?7 ELSE last_compiled END,
                          refresh_blocked_reason=CASE WHEN ?9 THEN NULL ELSE refresh_blocked_reason END,
                          source_revision=COALESCE(source_revision,0)+?9
          WHERE id=?1",
        params![page.id.as_str(),title,body,status,kind,i64::from(locked),now.to_rfc3339(),changelog,i64::from(prose_changed)],
    ).await?;
    // Same transaction: re-check every note that relies on this page.
    MemoryDB::append_page_history(conn, &page.id, changed_by, now.timestamp()).await?;
    read_page(conn, &page.id).await
}

async fn append_history(
    conn: &Connection,
    id: &str,
    edited_by: &str,
    at: i64,
) -> Result<(), CheckError> {
    MemoryDB::append_page_snapshot_on(conn, id, edited_by, at, false).await?;
    Ok(())
}

async fn mark_concurrent(conn: &Connection, page: &Page, version: i64) -> Result<(), CheckError> {
    let mut entries: Vec<serde_json::Value> =
        serde_json::from_str(page.changelog.as_deref().unwrap_or("[]"))
            .map_err(WenlanError::from)?;
    if let Some(entry) = entries
        .iter_mut()
        .rev()
        .find(|entry| entry["version"].as_i64() == Some(page.version))
    {
        entry["concurrent_with"] = serde_json::json!(version);
        conn.execute(
            "UPDATE pages SET changelog=?2 WHERE id=?1",
            params![
                page.id.as_str(),
                serde_json::to_string(&entries).map_err(WenlanError::from)?
            ],
        )
        .await?;
    }
    Ok(())
}

async fn refresh_links(
    conn: &Connection,
    page: &Page,
    visible: &HashMap<String, String>,
) -> Result<(), CheckError> {
    let matches = |label: &str| {
        let key = MemoryDB::page_title_key(label);
        let mut targets = visible
            .iter()
            .filter(|(_, title)| MemoryDB::page_title_key(title) == key);
        let (id, _) = targets.next()?;
        targets.next().is_none().then(|| id.clone())
    };
    let links = crate::synthesis::wikilinks::extract_wikilinks(&page.content)
        .into_iter()
        .map(|label| crate::synthesis::wikilinks::Wikilink {
            target_page_id: matches(&label),
            label,
        })
        .collect::<Vec<_>>();
    MemoryDB::replace_page_links_on(conn, &page.id, &links, true).await?;
    Ok(())
}

async fn stage_card(
    conn: &Connection,
    page: &Page,
    proposal: &PageVersion,
    stay: &str,
    local_member: &str,
) -> Result<String, CheckError> {
    let (incarnation, source_revision): (String, i64) = {
        let mut rows = conn
            .query(
                "SELECT incarnation,COALESCE(source_revision,0) FROM pages WHERE id=?1",
                [page.id.as_str()],
            )
            .await?;
        let row = rows.next().await?.ok_or(Reason::PageUnavailable)?;
        (row.get(0)?, row.get(1)?)
    };
    let edited_by = writer_edited_by(&proposal.meta.author, local_member);
    let id = format!("mem_{}", uuid::Uuid::new_v4().simple());
    let now = chrono::Utc::now().timestamp();
    let content = checked_body(page, &proposal.page.body)?;
    let stored_body = super::revision_cards::stored_card_body(&content, now);
    if stored_body != content && !page.citations.is_empty() {
        return Err(Reason::CitationsWouldChange.into());
    }
    let structured = serde_json::json!({
        "page_incarnation": incarnation,
        "revision_kind": "page_write",
        "target_kind": "page",
        "revises_page": page.id,
        "page_version": page.version,
        "source_revision": source_revision,
        "edited_by": edited_by,
        "review_reason": "locked",
        "preserve_source_links": true,
        "citations": page.citations,
        "citation_body_digest": crate::pdf_citations::body_digest(&content),
        "proposal_digest": crate::provenance::revision_content_digest(&stored_body),
        "member_sync_stay": stay,
        "member_sync_proposal": proposal.sha,
    });
    let title: String = format!("Revision: {}", page.title)
        .chars()
        .take(80)
        .collect();
    let source_agent = proposal.meta.author.agent.as_deref();
    let space = page.workspace.as_deref().ok_or(Reason::PageUnavailable)?;
    let mut previous = conn
        .query(
            "SELECT id,content FROM memories
          WHERE source='memory' AND pending_revision=1 AND chunk_index=0
            AND supersedes=?1 AND space=?2 AND json_valid(structured_fields)
            AND json_extract(structured_fields,'$.member_sync_stay')=?3
            AND json_extract(structured_fields,'$.member_sync_proposal')=?4",
            params![page.id.as_str(), space, stay, proposal.sha.as_str()],
        )
        .await?;
    if let Some(row) = previous.next().await? {
        let previous_id: String = row.get(0)?;
        let previous_body: String = row.get(1)?;
        if previous_body != stored_body || previous.next().await?.is_some() {
            return Err(Reason::ReviewUnavailable.into());
        }
        drop(previous);
        conn.execute(
            "UPDATE memories SET structured_fields=?2 WHERE id=?1",
            params![previous_id.as_str(), structured.to_string()],
        )
        .await?;
        return Ok(previous_id);
    }
    drop(previous);
    let written = conn
        .execute(
            "INSERT INTO memories(
            id,source,source_id,content,source_text,title,chunk_index,last_modified,created_at,
            chunk_type,memory_type,space,source_agent,confirmed,pending_revision,supersedes,
            origin_class,word_count,structured_fields)
         SELECT ?1,'memory',?1,?2,?2,?3,0,?4,?4,'text','fact',?5,?6,0,1,?7,?8,?9,?10
          WHERE NOT EXISTS (
              SELECT 1 FROM memories m
               WHERE m.source='memory' AND m.pending_revision=1 AND m.chunk_index=0
                 AND m.supersedes=?7 AND m.space=?5)",
            params![
                id.as_str(),
                stored_body,
                title,
                now,
                space,
                source_agent,
                page.id.as_str(),
                crate::origin::classify_origin(source_agent).as_str(),
                content.split_whitespace().count() as i64,
                structured.to_string()
            ],
        )
        .await?;
    if written != 1 {
        return Err(Reason::ReviewUnavailable.into());
    }
    Ok(id)
}

async fn close_card(
    conn: &Connection,
    page: &Page,
    card_id: &str,
    proposal: &str,
    space: &str,
    stay: &str,
) -> Result<(), CheckError> {
    let closed = conn.execute(
        "UPDATE memories SET pending_revision=0,supersede_mode='evicted'
          WHERE source='memory' AND source_id=?1 AND supersedes=?2 AND space=?3
            AND pending_revision=1 AND json_valid(structured_fields)
            AND json_extract(structured_fields,'$.revision_kind')='page_write'
            AND json_extract(structured_fields,'$.research_write') IS NULL
            AND json_extract(structured_fields,'$.page_incarnation')=(SELECT incarnation FROM pages WHERE id=?2)
            AND ((json_extract(structured_fields,'$.member_sync_proposal') IS NULL
                   AND json_extract(structured_fields,'$.member_sync_stay') IS NULL)
              OR (json_extract(structured_fields,'$.member_sync_proposal')=?4
                   AND json_extract(structured_fields,'$.member_sync_stay')=?5))",
        params![card_id, page.id.as_str(), space, proposal, stay],
    ).await?;
    if closed == 0 {
        return Err(Reason::ReviewUnavailable.into());
    }
    Ok(())
}
