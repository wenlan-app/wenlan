// SPDX-License-Identifier: Apache-2.0
//! Archiving and deleting a page, for every surface (HTTP, MCP through it).
//!
//! An archive can be undone (`restore_page`); a delete cannot. So:
//! - an AI tool (any caller that is not the person in the app) may archive
//!   only a page that holds none of the person's work (UX-WHO-2);
//! - only the person may delete a page for good;
//! - both write an `agent_activity` row in the same transaction.

use crate::db::{MemoryDB, PageRemovalAudit};
use crate::error::WenlanError;
use crate::pages::Page;
use crate::read_scope::ReadScope;
use wenlan_types::responses::PageRemovalErrorCode;

/// Who asks to archive or delete a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRemover<'a> {
    /// The person, with their own gesture in the app.
    Person,
    /// Anything else, named by its agent name (`unknown` when it sent none).
    Ai(&'a str),
}

impl PageRemover<'_> {
    /// The name the `agent_activity` row carries.
    pub fn audit_name(&self) -> &str {
        match self {
            Self::Person => "person",
            Self::Ai(agent) => agent,
        }
    }
}

#[derive(Debug)]
pub enum PageRemovalError {
    /// No such page in the caller's Space: a page outside it answers as a
    /// missing one would.
    NotFound,
    Refused(PageRemovalErrorCode),
    Failed(WenlanError),
}

impl From<WenlanError> for PageRemovalError {
    fn from(error: WenlanError) -> Self {
        Self::Failed(error)
    }
}

/// The English text for a refusal; clients switch on the code.
pub fn page_removal_refusal_message(code: PageRemovalErrorCode) -> &'static str {
    match code {
        PageRemovalErrorCode::PageIsThePersons => {
            "page_is_the_persons: this page holds the person's own writing or is \
             locked for review, so only the person can archive or delete it"
        }
        PageRemovalErrorCode::PermanentDeleteNeedsPerson => {
            "permanent_delete_needs_person: only the person, in the app, can \
             delete a page for good; archive it instead, which can be undone"
        }
    }
}

/// Does `page` hold the person's work, so that an AI may not remove it?
///
/// True for a review-locked page, a human-owned one (`user_edited` or
/// `authored`), a page made from an original the person brought in (an
/// imported chat), a page with any version the person wrote (`manual_edit`,
/// `fs_edit`, an accepted revision, ...), and a page with any sentence
/// `authorship::is_protected` keeps from a machine.
///
/// An original an AI tool kept by itself in a live session is the AI's own
/// work, so the AI may archive it (it can be restored) unless the person
/// edited it since.
pub async fn page_holds_persons_work(db: &MemoryDB, page: &Page) -> Result<bool, WenlanError> {
    let guarded = super::page_is_human_owned(page) || page.review_required;
    if guarded {
        return Ok(true);
    }
    // An AI-kept original's sentences read as `Original`, which keeps them
    // from being rewritten (UX-ART-3); that guards edits, not archiving.
    let mut kept_by_ai = false;
    if page.creation_kind == "original" {
        kept_by_ai = db
            .get_original(&page.id)
            .await?
            .is_some_and(|record| record.kept_in_live_session());
        if !kept_by_ai {
            return Ok(true);
        }
    }
    let history = db.list_page_history(&page.id, 200).await?;
    if history.iter().any(|entry| {
        crate::authorship::version_author(&entry.edited_by, "")
            == crate::authorship::VersionAuthor::Person
    }) {
        return Ok(true);
    }
    Ok(crate::sentences::page_authorship(db, page)
        .await?
        .iter()
        .any(|row| {
            if kept_by_ai {
                row.author == wenlan_types::authorship::SentenceAuthor::Person
            } else {
                crate::authorship::is_protected(row.author, guarded)
            }
        }))
}

/// The page `id` as the caller may see it. `Ok(None)` when no page has the
/// id; a page outside `scope` is [`PageRemovalError::NotFound`].
async fn page_in_scope(
    db: &MemoryDB,
    id: &str,
    scope: &ReadScope,
) -> Result<Option<Page>, PageRemovalError> {
    match db.get_page(id).await? {
        None => Ok(None),
        Some(page) if scope.matches(page.space.as_deref()) => Ok(Some(page)),
        Some(_) => Err(PageRemovalError::NotFound),
    }
}

/// Archive page `id` for `remover`, recording it. Returns the version the
/// archive produced, or `None` when the page is missing or not active, which
/// changes and records nothing.
///
/// An AI's archive is fenced on the version it was checked at, so a person's
/// edit that lands in between keeps the page active.
pub async fn archive_page_as(
    db: &MemoryDB,
    id: &str,
    remover: PageRemover<'_>,
    scope: &ReadScope,
) -> Result<Option<i64>, PageRemovalError> {
    let Some(page) = page_in_scope(db, id, scope).await? else {
        return Ok(None);
    };
    if page.status != "active" {
        return Ok(None);
    }
    let expected_version = match remover {
        PageRemover::Person => None,
        PageRemover::Ai(_) => {
            if page_holds_persons_work(db, &page).await? {
                log::warn!(
                    "[page_removal] refused archive of page {id} by {}: page holds the person's work",
                    remover.audit_name()
                );
                return Err(PageRemovalError::Refused(
                    PageRemovalErrorCode::PageIsThePersons,
                ));
            }
            Some(page.version)
        }
    };
    let audit = PageRemovalAudit {
        agent: remover.audit_name(),
        title: &page.title,
    };
    let version = db
        .archive_page_recorded(id, expected_version, Some(audit))
        .await?;
    if version.is_none() && expected_version.is_some() {
        log::warn!(
            "[page_removal] archive of page {id} by {} not applied: page changed while it was being archived",
            remover.audit_name()
        );
        return Err(PageRemovalError::Failed(WenlanError::Validation(format!(
            "page_changed: page {id} changed while it was being archived; read it again"
        ))));
    }
    if let Some(version) = version {
        log::info!(
            "[page_removal] archived page {id} by {} (v{version})",
            remover.audit_name()
        );
    }
    Ok(version)
}

/// Delete page `id` for good, with its history, recording it. Only the
/// person may; a missing page is a no-op.
pub async fn delete_page_permanently(
    db: &MemoryDB,
    id: &str,
    remover: PageRemover<'_>,
    scope: &ReadScope,
) -> Result<(), PageRemovalError> {
    if remover != PageRemover::Person {
        log::warn!(
            "[page_removal] refused permanent delete of page {id} by {}: only the person may",
            remover.audit_name()
        );
        return Err(PageRemovalError::Refused(
            PageRemovalErrorCode::PermanentDeleteNeedsPerson,
        ));
    }
    let Some(page) = page_in_scope(db, id, scope).await? else {
        return Ok(());
    };
    let audit = PageRemovalAudit {
        agent: remover.audit_name(),
        title: &page.title,
    };
    db.delete_page_recorded(id, Some(audit)).await?;
    log::info!(
        "[page_removal] deleted page {id} permanently by {}",
        remover.audit_name()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use crate::test_log::LogCapture;

    async fn page(db: &MemoryDB, id: &str, kind: &str) {
        db.insert_page_with_kind(
            id,
            "Removal",
            None,
            "Kestrels hover before they stoop.",
            None,
            None,
            &[],
            "2026-01-01T00:00:00Z",
            kind,
            "confirmed",
            None,
            None,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn archives_and_deletes_are_logged_with_who_asked() {
        let (db, _dir) = test_db().await;
        page(&db, "rm1", "distilled").await;
        page(&db, "rm2", "distilled").await;
        let logs = LogCapture::start();
        let version = archive_page_as(&db, "rm1", PageRemover::Ai("codex"), &ReadScope::Global)
            .await
            .unwrap()
            .expect("archived");
        delete_page_permanently(&db, "rm2", PageRemover::Person, &ReadScope::Global)
            .await
            .unwrap();
        let archived = logs.matching(
            "INFO",
            &["archived page rm1", "by codex", &format!("v{version}")],
        );
        assert_eq!(archived.len(), 1, "{:?}", logs.lines());
        let deleted = logs.matching("INFO", &["deleted page rm2", "by person"]);
        assert_eq!(deleted.len(), 1, "{:?}", logs.lines());
    }

    #[tokio::test]
    async fn refused_removals_are_logged_as_warnings() {
        let (db, _dir) = test_db().await;
        page(&db, "rm3", "authored").await;
        let logs = LogCapture::start();
        let archive =
            archive_page_as(&db, "rm3", PageRemover::Ai("codex"), &ReadScope::Global).await;
        assert!(matches!(archive, Err(PageRemovalError::Refused(_))));
        let delete =
            delete_page_permanently(&db, "rm3", PageRemover::Ai("codex"), &ReadScope::Global).await;
        assert!(matches!(delete, Err(PageRemovalError::Refused(_))));
        assert_eq!(
            logs.matching("WARN", &["refused archive of page rm3", "by codex"])
                .len(),
            1,
            "{:?}",
            logs.lines()
        );
        assert_eq!(
            logs.matching(
                "WARN",
                &["refused permanent delete of page rm3", "by codex"]
            )
            .len(),
            1,
            "{:?}",
            logs.lines()
        );
    }

    async fn original(db: &MemoryDB, id: &str, conversation_url: &str) {
        page(db, id, "original").await;
        db.insert_original(&crate::db::originals::OriginalRecord {
            page_id: id.to_string(),
            vendor: "claude-code".to_string(),
            conversation_id: format!("conv-{id}"),
            artifact_id: format!("art-{id}"),
            kind: "svg".to_string(),
            title: Some("Flow".to_string()),
            file_path: format!("originals/claude-code/conv-{id}/art.svg"),
            conversation_url: conversation_url.to_string(),
            written_at: Some(0),
            imported_at: 0,
            replaces: None,
        })
        .await
        .unwrap();
    }

    /// An original an AI tool kept by itself in a live session, the way the
    /// keep route makes it. Returns its page id.
    async fn kept(db: &MemoryDB, root: &std::path::Path, content: &str) -> String {
        use crate::chat_import::originals::{
            keep_session_original, SessionOriginal, SessionOriginalFormat,
        };
        let session = wenlan_types::sessions::SessionRef::new("claude-code", "drawing").unwrap();
        let (kept, created) = keep_session_original(
            db,
            root,
            None,
            SessionOriginal {
                session: Some(&session),
                title: "Release flow",
                content,
                format: SessionOriginalFormat::Svg,
                space: wenlan_types::WriteSpaceTarget::Uncategorized,
                workspace: None,
                source_memory_ids: Vec::new(),
                replaces: None,
            },
        )
        .await
        .unwrap();
        assert!(created);
        kept.page_id
    }

    async fn person_edits(db: &MemoryDB, id: &str) {
        let wrote = crate::post_write::page_write(
            db,
            crate::post_write::PageWrite::Update {
                page_id: id,
                req: wenlan_types::requests::UpdatePageRequest {
                    content: "Kestrels hover before they stoop. I drew this one.".to_string(),
                    source_memory_ids: vec![],
                    expected_version: None,
                    caller_id: None,
                    operation_id: None,
                },
                edited_by: "manual_edit",
                require_stale: false,
                expected_source_revision: None,
                knowledge_path: None,
                citations: None,
            },
        )
        .await
        .unwrap();
        assert!(wrote.wrote, "{:?}", wrote.warnings);
    }

    #[tokio::test]
    async fn an_ai_may_archive_an_original_it_kept_in_a_live_session() {
        let (db, dir) = test_db().await;
        let kept1 = kept(
            &db,
            dir.path(),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>",
        )
        .await;
        let archived = archive_page_as(
            &db,
            &kept1,
            PageRemover::Ai("claude-code"),
            &ReadScope::Global,
        )
        .await
        .unwrap();
        assert!(archived.is_some());
        // Archiving is the most an AI may do: deleting for good stays the person's.
        let kept2 = kept(
            &db,
            dir.path(),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle/></svg>",
        )
        .await;
        let delete = delete_page_permanently(
            &db,
            &kept2,
            PageRemover::Ai("claude-code"),
            &ReadScope::Global,
        )
        .await;
        assert!(matches!(
            delete,
            Err(PageRemovalError::Refused(
                PageRemovalErrorCode::PermanentDeleteNeedsPerson
            ))
        ));
    }

    #[tokio::test]
    async fn an_imported_original_stays_the_persons() {
        let (db, _dir) = test_db().await;
        original(&db, "imp1", "https://claude.ai/chat/abc").await;
        let archive = archive_page_as(
            &db,
            "imp1",
            PageRemover::Ai("claude-code"),
            &ReadScope::Global,
        )
        .await;
        assert!(matches!(
            archive,
            Err(PageRemovalError::Refused(
                PageRemovalErrorCode::PageIsThePersons
            ))
        ));
    }

    #[tokio::test]
    async fn an_original_page_without_its_record_stays_the_persons() {
        let (db, _dir) = test_db().await;
        page(&db, "bare1", "original").await;
        let archive = archive_page_as(
            &db,
            "bare1",
            PageRemover::Ai("claude-code"),
            &ReadScope::Global,
        )
        .await;
        assert!(matches!(
            archive,
            Err(PageRemovalError::Refused(
                PageRemovalErrorCode::PageIsThePersons
            ))
        ));
    }

    #[tokio::test]
    async fn an_ai_kept_original_the_person_edited_stays_theirs() {
        let (db, dir) = test_db().await;
        let kept3 = kept(
            &db,
            dir.path(),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>",
        )
        .await;
        person_edits(&db, &kept3).await;
        let archive = archive_page_as(
            &db,
            &kept3,
            PageRemover::Ai("claude-code"),
            &ReadScope::Global,
        )
        .await;
        assert!(matches!(
            archive,
            Err(PageRemovalError::Refused(
                PageRemovalErrorCode::PageIsThePersons
            ))
        ));
    }
}
