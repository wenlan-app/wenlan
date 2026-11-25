// SPDX-License-Identifier: Apache-2.0
//! One local queue for the existing personal setup reviews.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};
use axum::{http::HeaderMap, Json};
use tokio::sync::Mutex;
use wenlan_core::{
    rulesync_adopt::AdoptedSkill,
    rulesync_personal::{enabled, fingerprint},
    skill_registry::{self, Endpoints, ReqwestFetch},
};
use wenlan_types::{
    rulesync::{
        AdoptedSkillUpdate, DismissSkillUpdate, PullbackAction, PullbackKind, RulesReviewContent,
        RulesReviewItem, RulesReviewList, SkillUpdateStatus,
    },
    rulesync_skill::{SavedSkillScope, SkillProposal, SkillProposalOrigin},
};

use super::{
    require_local_rules_access, require_personal_rules_writer, tool_label, Setup, PERSONAL_TARGETS,
};
#[cfg(test)]
use crate::rulesync_apply::FileRoot;
use crate::{error::ServerError, rulesync_registry, rulesync_settings_routes};

struct CachedUpdates {
    source: String,
    at: Instant,
    rows: Vec<AdoptedSkillUpdate>,
}

static UPDATES: Mutex<Option<CachedUpdates>> = Mutex::const_new(None);

async fn updates(home: &Path, skills: &[AdoptedSkill]) -> Result<Vec<AdoptedSkillUpdate>> {
    if skills.iter().all(|skill| skill.upstream.is_none()) {
        return Ok(Vec::new());
    }
    let source = fingerprint(&(home, skills))?;
    let mut cache = UPDATES.lock().await;
    if let Some(cached) = cache.as_ref().filter(|cached| {
        cached.source == source && cached.at.elapsed() < Duration::from_secs(5 * 60)
    }) {
        return Ok(cached.rows.clone());
    }
    let http = ReqwestFetch::new().map_err(anyhow::Error::msg)?;
    let rows = skill_registry::adopted_updates(&http, &Endpoints::default(), home, skills).await;
    *cache = Some(CachedUpdates {
        source,
        at: Instant::now(),
        rows: rows.clone(),
    });
    Ok(rows)
}

pub(super) async fn existing_update_check() -> Result<Vec<AdoptedSkillUpdate>, ServerError> {
    let (home, skills, dismissed) = on_disk(|| {
        let setup = Setup::system()?;
        let manifest = setup
            .load_if_present()?
            .filter(|manifest| manifest.adopted)
            .context("Run wenlan rules adopt first")?;
        Ok((
            setup.home,
            manifest.master.skills,
            manifest.dismissed_skill_updates,
        ))
    })
    .await?;
    visible_updates(updates(&home, &skills).await.map_err(conflict)?, &dismissed).map_err(conflict)
}

pub(super) async fn list(headers: HeaderMap) -> Result<Json<RulesReviewList>, ServerError> {
    require_local_rules_access(&headers)?;
    if !enabled() {
        return Ok(Json(RulesReviewList::default()));
    }
    let (mut queue, home, skills, dismissed) = on_disk(|| {
        let setup = Setup::system()?;
        let queue = local_queue(&setup)?;
        let (skills, dismissed) = setup
            .load_if_present()?
            .filter(|manifest| manifest.adopted)
            .map(|manifest| (manifest.master.skills, manifest.dismissed_skill_updates))
            .unwrap_or_default();
        Ok((queue, setup.home, skills, dismissed))
    })
    .await?;
    let rows = updates(&home, &skills).await.map_err(conflict)?;
    for update in visible_updates(rows, &dismissed).map_err(conflict)? {
        if let Some((_, id)) = update_offer(&update).map_err(conflict)? {
            queue.items.push(RulesReviewItem {
                id,
                answers: vec![PullbackAction::Dismiss],
                content: RulesReviewContent::SkillUpdate { update },
            });
        }
    }
    Ok(Json(queue))
}

fn local_queue(setup: &Setup) -> Result<RulesReviewList> {
    let status = setup.status_with_source_lock()?;
    let mut items = status
        .proposals
        .into_iter()
        .map(|card| {
            let mut answers = vec![PullbackAction::ToolOnly, PullbackAction::AllTools];
            if card.kind == PullbackKind::Conflict {
                answers.push(PullbackAction::Revert);
            }
            RulesReviewItem {
                id: card.id.clone(),
                answers,
                content: RulesReviewContent::ToolEdit { card },
            }
        })
        .collect::<Vec<_>>();
    let mut tools = status.review_tools;
    for proposal in rulesync_registry::pending_skill_reviews(&setup.data)? {
        if let Some(target) = proposal_tool(&proposal)
            .and_then(|tool| PERSONAL_TARGETS.iter().find(|target| target.tool == tool))
        {
            tools.insert(proposal.id.clone(), tool_label(target));
        }
        items.push(skill_review_item(proposal));
    }
    if status.adopted || status.can_restore {
        let root = setup.home_files()?;
        match rulesync_settings_routes::plan_local(setup, &root) {
            Ok(Some(settings)) => {
                let plan = settings.response;
                items.push(RulesReviewItem {
                    id: format!("settings:{}", plan.token),
                    answers: vec![PullbackAction::AllTools],
                    content: RulesReviewContent::Settings { plan },
                });
            }
            Ok(None) => (),
            Err(error) => items.push(RulesReviewItem {
                id: "settings:blocked".into(),
                answers: Vec::new(),
                content: RulesReviewContent::Settings {
                    plan: wenlan_types::rulesync_settings::SettingsPlan {
                        token: String::new(),
                        files: Vec::new(),
                        notice: Some(format!("{error:#}")),
                    },
                },
            }),
        }
    }
    Ok(RulesReviewList {
        items,
        review_tools: tools,
    })
}

pub(super) fn proposal_tool(
    proposal: &SkillProposal,
) -> Option<wenlan_types::rulesync_skill::SavedSkillTool> {
    match (&proposal.origin, proposal.scope) {
        (SkillProposalOrigin::Tool { tool } | SkillProposalOrigin::Delivery { tool }, _) => {
            Some(*tool)
        }
        (_, SavedSkillScope::Tool { tool }) => Some(tool),
        (_, SavedSkillScope::Shared) => None,
    }
}

fn skill_review_item(proposal: SkillProposal) -> RulesReviewItem {
    let mut answers = vec![PullbackAction::AllTools];
    if proposal_tool(&proposal).is_some() {
        answers.insert(0, PullbackAction::ToolOnly);
    }
    answers.push(PullbackAction::Dismiss);
    RulesReviewItem {
        id: proposal.id.clone(),
        answers,
        content: RulesReviewContent::SkillProposal { proposal },
    }
}

fn update_offer(update: &AdoptedSkillUpdate) -> Result<Option<(String, String)>> {
    let SkillUpdateStatus::Available { revision } = &update.status else {
        return Ok(None);
    };
    let source = fingerprint(&(&update.name, &update.upstream))?;
    let id = format!("update:{}", fingerprint(&(&source, revision))?);
    Ok(Some((source, id)))
}

fn visible_updates(
    rows: Vec<AdoptedSkillUpdate>,
    dismissed: &BTreeMap<String, String>,
) -> Result<Vec<AdoptedSkillUpdate>> {
    let mut seen = BTreeSet::new();
    let mut visible = Vec::new();
    for update in rows {
        let hidden = update_offer(&update)?
            .is_some_and(|(source, id)| dismissed.get(&source) == Some(&id) || !seen.insert(id));
        if !hidden {
            visible.push(update);
        }
    }
    Ok(visible)
}

pub(super) async fn dismiss_update(
    headers: HeaderMap,
    Json(request): Json<DismissSkillUpdate>,
) -> Result<Json<()>, ServerError> {
    require_personal_rules_writer(&headers)?;
    let (offer, source) = existing_update_check()
        .await?
        .into_iter()
        .find_map(|update| {
            let (source, id) = update_offer(&update).ok().flatten()?;
            (id == request.id).then_some((update, source))
        })
        .ok_or_else(|| ServerError::Conflict("This skill update is no longer available".into()))?;
    on_disk(move || {
        let setup = Setup::system()?;
        let _lock = setup.lock()?;
        let mut manifest = setup
            .load()?
            .filter(|manifest| manifest.adopted)
            .context("Run wenlan rules adopt first")?;
        ensure!(
            manifest
                .master
                .skills
                .iter()
                .any(|skill| skill.name == offer.name
                    && skill.upstream.as_ref() == Some(&offer.upstream)),
            "The skill changed since review"
        );
        manifest.dismissed_skill_updates.insert(source, request.id);
        setup.save(&manifest)
    })
    .await?;
    Ok(Json(()))
}

fn conflict(error: anyhow::Error) -> ServerError {
    ServerError::Conflict(format!("{error:#}"))
}

async fn on_disk<T: Send + 'static>(
    read: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(read)
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map_err(conflict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rulesync_routes::tests::{Fixture, CODEX};
    use wenlan_types::rulesync::{PullbackDecision, SkillUpstream};

    #[test]
    fn rulesync_review_lists_only_applicable_rule_answers() {
        let f = Fixture::new();
        let command = ".claude/commands/private.md";
        std::fs::create_dir_all(f.setup.home.join(command).parent().unwrap()).unwrap();
        f.write(command, "Before.\n");
        f.adopt_from_plan();
        f.write(command, "After.\n");
        f.write(
            CODEX,
            &f.text(CODEX).replace("Second rule.", "Edited in Codex."),
        );
        {
            let _lock = f.setup.lock().unwrap();
            let root = FileRoot::open_home(&f.setup.home).unwrap();
            let mut manifest = f.setup.load().unwrap().unwrap();
            f.sync_review(&root, &mut manifest).unwrap();
        }

        let queue = local_queue(&f.setup).unwrap();
        let [scope, conflict] = [PullbackKind::Scope, PullbackKind::Conflict].map(|kind| {
            queue.items.iter().find(|item| {
                matches!(&item.content, RulesReviewContent::ToolEdit { card } if card.kind == kind)
            }).unwrap()
        });
        assert_eq!(
            scope.answers,
            [PullbackAction::ToolOnly, PullbackAction::AllTools]
        );
        assert_eq!(
            conflict.answers,
            [
                PullbackAction::ToolOnly,
                PullbackAction::AllTools,
                PullbackAction::Revert
            ]
        );
    }

    #[test]
    fn rulesync_settings_changes_wait_in_to_review_without_sync() {
        let f = Fixture::new();
        let before = r#"{"mcpServers":{"shared":{"command":"before"}}}"#;
        f.write(".claude.json", before);
        f.adopt_from_plan();
        f.write(".claude.json", &before.replace("\"before\"", "\"after\""));

        let queue = local_queue(&f.setup).unwrap();
        let item = queue
            .items
            .iter()
            .find(|item| item.id.starts_with("setting-"))
            .unwrap_or_else(|| panic!("settings change missing from To review: {queue:?}"));
        assert_eq!(
            item.answers,
            [
                PullbackAction::ToolOnly,
                PullbackAction::AllTools,
                PullbackAction::Revert
            ]
        );
        let RulesReviewContent::ToolEdit { card } = &item.content else {
            panic!("{item:?}");
        };
        assert!(card.diff.contains("after"), "{card:?}");
        assert_eq!(queue.review_tools[".claude.json"].id, "claude-code");
        assert!(!f.setup.status().unwrap().in_sync);

        let decision = PullbackDecision {
            id: item.id.clone(),
            action: PullbackAction::Revert,
            file: None,
        };
        let preview = f.setup.preview_review(&decision).unwrap();
        assert!(preview.diff.contains("after"), "{preview:?}");
        f.setup
            .review_using(&decision, |root, manifest| f.sync_review(root, manifest))
            .unwrap();
        assert!(f.text(".claude.json").contains("\"before\""));
        assert!(!local_queue(&f.setup)
            .unwrap()
            .items
            .iter()
            .any(|item| item.id.starts_with("setting-")));
    }

    #[test]
    fn rulesync_review_updates_are_unique_and_dismiss_one_source_revision() {
        let update = |package: &str, revision: &str| AdoptedSkillUpdate {
            name: "notes".into(),
            upstream: SkillUpstream::Npm {
                package: package.into(),
                version: "1.0.0".into(),
                directory: String::new(),
            },
            status: SkillUpdateStatus::Available {
                revision: revision.into(),
            },
        };
        let first = update("first-notes", "2.0.0");
        let second = update("second-notes", "2.0.0");
        let newer = update("first-notes", "3.0.0");
        let rows = vec![first.clone(), first.clone(), second.clone(), newer.clone()];
        assert_eq!(
            visible_updates(rows.clone(), &BTreeMap::new()).unwrap(),
            [first.clone(), second.clone(), newer.clone()]
        );
        let (source, id) = update_offer(&first).unwrap().unwrap();
        assert_eq!(
            visible_updates(rows, &BTreeMap::from([(source, id)])).unwrap(),
            [second, newer]
        );
    }
}
