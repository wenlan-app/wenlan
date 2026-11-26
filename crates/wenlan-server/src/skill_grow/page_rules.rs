// SPDX-License-Identifier: Apache-2.0
//! Read the every-Space page rules with the provenance needed by a rule plan.

use wenlan_core::rulesync_personal::credentials::credential_line_numbers;
use wenlan_types::personal_rules::PageRuleLocation;

use crate::rulesync_routes::personal_rules::{PageRule, PageRuleSnapshot};

use super::*;

pub(crate) async fn merge_snapshot(state: &SharedState) -> Result<PageRuleSnapshot, ServerError> {
    let (db, root) = db_and_page_root(state).await?;
    all(&db, root.as_deref()).await
}

pub(super) async fn all(
    db: &MemoryDB,
    root: Option<&Path>,
) -> Result<PageRuleSnapshot, ServerError> {
    let mut snapshot = snapshot(db, root, None).await?;
    // Preserve delivery order and the cap. A second page with the same exact
    // sentence contributes provenance without taking another slot.
    let mut rules = HashSet::new();
    snapshot.rules.retain(|rule| {
        rules.contains(&rule.text)
            || (rules.len() < skill_grow::RULES_MAIN_FILE_MAX && rules.insert(rule.text.clone()))
    });
    // Confirmed preferences move too, in the order tools get them today,
    // each recording the memory it came from. The memory itself stays.
    for (index, line) in preference_lines(db).await?.iter().enumerate() {
        add_rule(&mut snapshot, None, index + 1, line);
    }
    Ok(snapshot)
}

/// One confirmed preference as a shared-line change, after the move.
pub(super) fn preference_snapshot(line: &SkillLine) -> PageRuleSnapshot {
    let mut snapshot = PageRuleSnapshot::default();
    add_rule(&mut snapshot, None, 1, line);
    snapshot
}

pub(super) async fn snapshot(
    db: &MemoryDB,
    root: Option<&Path>,
    only_page: Option<&str>,
) -> Result<PageRuleSnapshot, ServerError> {
    let own = OwnRulePages::load(db, root).await?;
    let mut snapshot = PageRuleSnapshot::default();
    let chosen = |id: &str| only_page.is_none_or(|only| only == id);
    for own_page in own
        .pages
        .iter()
        .filter(|page| page.space.is_none() && chosen(&page.id))
    {
        if let Some(page) = db.get_page(&own_page.id).await? {
            add_page(&mut snapshot, &page, &own_page.lines);
        }
    }
    for space in db.list_spaces().await? {
        let Some((state, page)) = readable_rules_page(db, &space).await? else {
            continue;
        };
        if !chosen(&page.id) {
            continue;
        }
        let lines = page_rule_lines(db, root, &space, &state, &page).await?;
        if lines.iter().any(|line| line.general) {
            add_page(&mut snapshot, &page, &lines);
        }
    }
    // Stop and removal can leave no eligible lines, but the exact revision
    // at which the person stopped or removed them still binds the plan.
    if let Some(id) = only_page {
        if let Some(page) = db.get_page(id).await? {
            snapshot.revisions.insert(id.into(), page.version);
        }
    }
    Ok(snapshot)
}

pub(super) async fn pending_space_rules(
    db: &MemoryDB,
    space: &wenlan_types::Space,
    rules: &SpaceRulesState,
) -> Result<PageRuleSnapshot, ServerError> {
    let mut lines = eligible_lines(db, &space.id, &rules.lines).await?;
    keep_general_to_own_pages(db, &mut lines).await?;
    let mut snapshot = PageRuleSnapshot::default();
    for line in lines.iter().filter(|line| line.general) {
        if let Some(page) = db.get_page(&line.source_page_id).await? {
            add_page(&mut snapshot, &page, std::slice::from_ref(line));
        }
    }
    Ok(snapshot)
}

fn add_page(snapshot: &mut PageRuleSnapshot, page: &wenlan_types::Page, lines: &[SkillLine]) {
    snapshot.revisions.insert(page.id.clone(), page.version);
    snapshot.excluded.extend(
        credential_line_numbers(&page.content)
            .into_iter()
            .map(|line| PageRuleLocation {
                page_id: Some(page.id.clone()),
                line,
            }),
    );
    for (index, rule) in lines.iter().filter(|line| line.general).enumerate() {
        let marker = format!("<!-- wenlan-rule:{} -->", rule.id);
        let position = page
            .content
            .lines()
            .position(|line| line.contains(&marker))
            .or_else(|| {
                page.content
                    .lines()
                    .position(|line| line.contains(rule.text.trim()))
            })
            .map_or(index + 1, |line| line + 1);
        add_rule(snapshot, Some(page), position, rule);
    }
}

fn add_rule(
    snapshot: &mut PageRuleSnapshot,
    page: Option<&wenlan_types::Page>,
    position: usize,
    line: &SkillLine,
) {
    if line.kind != wenlan_types::SkillLineKind::Rule
        || line.text.contains(skill_grow::TOOL_RULES_END)
    {
        return;
    }
    let location = PageRuleLocation {
        page_id: page.map(|page| page.id.clone()),
        line: position,
    };
    let text = skill_grow::tool_file_text(&line.text);
    if !credential_line_numbers(&line.text).is_empty() || !credential_line_numbers(&text).is_empty()
    {
        snapshot.excluded.insert(location);
        return;
    }
    if text.is_empty() || snapshot.excluded.contains(&location) {
        return;
    }
    snapshot.rules.push(PageRule {
        text,
        source_text: line.text.clone(),
        location,
        rule_id: line.id.clone(),
        author: (line.hand_written || line.person_edited)
            .then_some("person".to_owned())
            .filter(|_| page.is_some()),
        proposer: None,
    });
}

pub(crate) async fn revisions_match(
    state: &SharedState,
    revisions: &std::collections::BTreeMap<String, i64>,
) -> Result<bool, ServerError> {
    let (db, _) = db_and_page_root(state).await?;
    for (id, version) in revisions {
        if db
            .get_page(id)
            .await?
            .is_none_or(|page| page.version != *version)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) async fn projected_rules(db: &MemoryDB) -> Result<Option<Vec<SkillLine>>, ServerError> {
    let Some(rows) = crate::rulesync_routes::personal_rules::merged_page_rules()? else {
        return Ok(None);
    };
    let mut projected = Vec::with_capacity(rows.len());
    for (text, origin) in rows {
        let preference =
            origin.location.page_id.is_none() && origin.rule_id.starts_with("preference:");
        let page_id = origin.location.page_id.unwrap_or_default();
        let (title, version) = if preference {
            (skill_grow::PREFERENCES_HEADING.to_owned(), 0)
        } else {
            db.get_page(&page_id)
                .await?
                .map_or_else(|| (String::new(), 0), |page| (page.title, page.version))
        };
        projected.push(SkillLine {
            id: origin.rule_id,
            kind: wenlan_types::SkillLineKind::Rule,
            text,
            source_page_id: page_id,
            source_page_title: title,
            source_revision: version,
            source_revision_at: 0,
            source_quote: String::new(),
            person_edited: origin.author.is_some(),
            general: true,
            hand_written: false,
            saved_from: None,
            saved_by: None,
        });
    }
    // Confirmed preferences moved with the page rules, so shared rules hold
    // every line tools follow.
    Ok(Some(projected))
}

pub(super) async fn apply_delta(
    headers: &HeaderMap,
    before: PageRuleSnapshot,
    mut after: PageRuleSnapshot,
) -> Result<Option<String>, ServerError> {
    let unchanged = |left: &PageRule, right: &PageRule| {
        left.rule_id == right.rule_id && left.text == right.text
    };
    let removed = before
        .rules
        .iter()
        .filter(|old| !after.rules.iter().any(|new| unchanged(old, new)))
        .map(|old| old.rule_id.clone())
        .collect();
    after
        .rules
        .retain(|new| !before.rules.iter().any(|old| unchanged(old, new)));
    crate::rulesync_routes::personal_rules::page_change(headers, Vec::new(), after, removed).await
}

pub(crate) struct PageEdit {
    _serial: tokio::sync::MutexGuard<'static, ()>,
    before: Option<PageRuleSnapshot>,
}

impl PageEdit {
    pub(crate) async fn begin(
        db: &MemoryDB,
        root: Option<&Path>,
        id: &str,
        headers: &HeaderMap,
    ) -> Result<Option<Self>, ServerError> {
        if !wenlan_core::rulesync_personal::enabled()
            && !crate::rulesync_routes::personal_rules::page_rules_merged()?
        {
            return Ok(None);
        }
        if !may_hold_own_rules(db, root, id).await {
            return Ok(None);
        }
        let serial = SKILLS_MUTATION_LOCK.lock().await;
        let before = if crate::rulesync_routes::personal_rules::page_rules_merged()? {
            // Refuse before the page changes when shared rules cannot follow
            // it, so the page and the tool files never disagree. A tool
            // proposes a personal-rule change instead of editing this page.
            if crate::rulesync_routes::require_local_person(headers).is_err() {
                return Err(ServerError::Conflict(
                    "Only the person in the Wenlan app on this computer edits a rules page \
                     after the move. An AI tool proposes the change with \
                     propose_personal_rules instead."
                        .into(),
                ));
            }
            crate::rulesync_routes::personal_rules::page_change_ready()?;
            Some(snapshot(db, root, Some(id)).await?)
        } else {
            None
        };
        Ok(Some(Self {
            _serial: serial,
            before,
        }))
    }

    pub(crate) async fn finish(
        self,
        db: &MemoryDB,
        root: Option<&Path>,
        id: &str,
        headers: &HeaderMap,
    ) -> Result<(), ServerError> {
        if let Some(before) = self.before {
            let after = snapshot(db, root, Some(id)).await?;
            apply_delta(headers, before, after).await?;
        }
        Ok(())
    }
}
