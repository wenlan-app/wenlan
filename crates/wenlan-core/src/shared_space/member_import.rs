// SPDX-License-Identifier: Apache-2.0
//! Dispatch of hosted page changes. Member-supplied versions can only mutate
//! pages and cards through the transaction in db::member_sync.

use super::{second_page_id, version, Held, Pass, Together};
use crate::db::member_sync::{
    MemberSyncConflict, MemberSyncInput, MemberSyncOperation, MemberSyncOutcome, MemberSyncWriter,
};
use crate::pages::Page;
use crate::WenlanError;
use version::PageVersion;
use wenlan_types::shared_space::SharedSyncCounts;
use wenlan_types::team::MemberSyncRefusalReason;

impl Pass<'_> {
    pub(super) fn member_writer(&self) -> MemberSyncWriter<'_> {
        MemberSyncWriter {
            db: self.db,
            space: &self.space,
            member_id: &self.settings.member_id,
            settings_path: self.settings_path,
            knowledge_path: self.knowledge_path,
        }
    }

    pub(super) async fn refuse_hosted_changes(
        &self,
        versions: &[&PageVersion],
        reason: MemberSyncRefusalReason,
        counts: &mut SharedSyncCounts,
    ) -> Result<(), WenlanError> {
        let writer = self.member_writer();
        for &change in versions {
            if change.meta.author.member != self.settings.member_id
                && !self.refused.contains(&change.sha)
                && !self.published.contains(&change.sha)
            {
                writer.refuse(change, reason).await?;
                counts.skipped += 1;
            }
        }
        Ok(())
    }

    pub(super) async fn apply_hosted_version(
        &self,
        page: &Page,
        head: &PageVersion,
        versions: &[PageVersion],
        preceding: &[&PageVersion],
        lost: Option<(&PageVersion, &Together)>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<Held>, WenlanError> {
        let fork = lost.map(|(mine, _)| second_page_id(&page.id, &mine.sha));
        let conflict = lost.map(|(mine, result)| match result {
            Together::Merged(merged) => MemberSyncConflict::Merge {
                title: &merged.title,
                body: &merged.body,
            },
            Together::Both => MemberSyncConflict::Copy {
                id: fork
                    .as_deref()
                    .expect("a losing edit has a conflict copy id"),
                version: mine,
                name: &self.settings.member_name,
            },
            Together::Notice => MemberSyncConflict::Notice,
        });
        let merged = matches!(conflict, Some(MemberSyncConflict::Merge { .. }));
        let outcome = self
            .member_writer()
            .write(MemberSyncInput {
                change: head,
                page: Some(page),
                history: versions,
                operation: MemberSyncOperation::Edit {
                    preceding,
                    conflict,
                },
            })
            .await?;
        let MemberSyncOutcome::Applied { version, wrote, .. } = outcome else {
            counts.skipped += 1;
            return Ok(None);
        };
        if wrote {
            counts.applied += 1;
            if lost.is_some() && !merged {
                counts.conflicts += 1;
            }
        }
        let Some(current) = self.reload_hosted(&page.id).await? else {
            return Ok(None);
        };
        Ok(Some(Held {
            sha: head.sha.clone(),
            version: Some(if merged || current.title != head.page.title {
                page.version
            } else {
                version
            }),
            space: None,
        }))
    }

    pub(super) async fn apply_hosted_create(
        &self,
        versions: &[PageVersion],
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<Held>, WenlanError> {
        let eligible: Vec<_> = versions
            .iter()
            .filter(|change| {
                change.meta.author.member != self.settings.member_id
                    && !self.refused.contains(&change.sha)
                    && !self.published.contains(&change.sha)
            })
            .cloned()
            .collect();
        let Some(head) = version::head(&eligible).map(|head| head.winner) else {
            return Ok(None);
        };
        let outcome = self
            .member_writer()
            .write(MemberSyncInput {
                change: head,
                page: None,
                history: &eligible,
                operation: MemberSyncOperation::Create,
            })
            .await?;
        let MemberSyncOutcome::Applied { wrote, .. } = outcome else {
            counts.skipped += 1;
            return Ok(None);
        };
        if wrote {
            counts.applied += 1;
        }
        let Some(id) = head.page.id.as_deref() else {
            return Ok(None);
        };
        let Some(current) = self.reload_hosted(id).await? else {
            return Ok(None);
        };
        // The member's new page had no host-issued stay. Publish a new root
        // with that witness before any further edit is taken.
        let Some(sha) = self.export_page(&current, &[], None, None, None).await? else {
            counts.skipped += 1;
            return Ok(None);
        };
        counts.exported += 1;
        Ok(Some(Held {
            sha,
            version: Some(current.version),
            space: None,
        }))
    }
}
