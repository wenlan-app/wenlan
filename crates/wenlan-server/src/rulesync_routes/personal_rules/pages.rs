// SPDX-License-Identifier: Apache-2.0
//! Move eligible page rules into the shared layer and record their sources.

use wenlan_types::personal_rules::{
    PageRuleLocation, PageRuleMergePreview, PersonalRulePageOrigin,
};

use super::*;

pub(super) const PAGES_PROPOSER: &str = "Wenlan pages";
pub(super) const MERGE_CAUSE: &str = "How I work merge";
pub(super) const PAGE_CHANGE_CAUSE: &str = "How I work change";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PageRulesState {
    #[default]
    Unanswered,
    Merged,
    Declined,
}

impl PageRulesState {
    pub(crate) fn is_unanswered(&self) -> bool {
        *self == Self::Unanswered
    }

    /// The generated lines a tool file still gets after `shared`. Before the
    /// move that is every line. After it, page rules live in shared, so only
    /// lines shared does not already hold remain: confirmed conversation
    /// preferences, which stay with their memories and are never moved.
    pub(crate) fn legacy_rules(&self, shared: &str, generated: &[String]) -> Vec<String> {
        if *self != Self::Merged {
            return generated.to_vec();
        }
        let held: BTreeSet<_> = shared
            .lines()
            .map(|line| {
                let line = line.trim();
                line.strip_prefix("- ").unwrap_or(line).trim()
            })
            .collect();
        generated
            .iter()
            .filter(|line| !held.contains(line.trim()))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct PageRule {
    pub text: String,
    pub source_text: String,
    pub location: PageRuleLocation,
    pub rule_id: String,
    pub author: Option<String>,
}

#[derive(Default, Clone, Serialize)]
pub(crate) struct PageRuleSnapshot {
    pub rules: Vec<PageRule>,
    pub excluded: BTreeSet<PageRuleLocation>,
    pub revisions: BTreeMap<String, i64>,
}

impl PageRuleSnapshot {
    pub(crate) fn has_candidates(&self) -> bool {
        !self.rules.is_empty() || !self.excluded.is_empty()
    }
}

#[derive(Clone, Serialize)]
pub(super) enum PagePlan {
    Merge(PageRuleSnapshot),
    Change {
        snapshot: PageRuleSnapshot,
        remove: BTreeSet<String>,
    },
}

impl PagePlan {
    pub(super) fn is_merge(&self) -> bool {
        matches!(self, Self::Merge(_))
    }

    pub(super) fn preview(&self) -> Option<PageRuleMergePreview> {
        let Self::Merge(snapshot) = self else {
            return None;
        };
        Some(PageRuleMergePreview {
            rule_count: snapshot
                .rules
                .iter()
                .map(|rule| rule.text.trim())
                .collect::<BTreeSet<_>>()
                .len(),
            excluded: snapshot.excluded.iter().cloned().collect(),
            excludes_space_only: true,
        })
    }

    pub(super) fn archive(&self) -> Option<BTreeMap<String, Vec<String>>> {
        let Self::Merge(snapshot) = self else {
            return None;
        };
        let mut archive = BTreeMap::<_, Vec<_>>::new();
        for rule in &snapshot.rules {
            if let Some(page) = &rule.location.page_id {
                let lines = archive.entry(page.clone()).or_default();
                if !lines.contains(&rule.source_text) {
                    lines.push(rule.source_text.clone());
                }
            }
        }
        Some(archive)
    }

    pub(super) fn prepare(
        &self,
        manifest: &Manifest,
    ) -> Result<(String, Vec<Option<PersonalRuleOrigin>>)> {
        let snapshot = match self {
            Self::Merge(snapshot) => {
                ensure!(
                    manifest.page_rules.is_unanswered() && snapshot.has_candidates(),
                    "There are no page rules waiting to move."
                );
                snapshot
            }
            Self::Change { snapshot, .. } => {
                ensure!(
                    manifest.page_rules == PageRulesState::Merged,
                    "Page rules have not been moved."
                );
                snapshot
            }
        };
        let shared = &manifest.master.shared;
        let mut lines: Vec<_> = shared
            .split_inclusive('\n')
            .map(str::to_owned)
            .zip(origin_lines(manifest, &PersonalRuleLayer::Shared, shared)?)
            .collect();
        if let Self::Change { remove, .. } = self {
            lines.retain_mut(|(_, origin)| {
                let Some(origin) = origin else { return true };
                if origin.pages.is_empty() {
                    return true;
                }
                origin.pages.retain(|page| !remove.contains(&page.rule_id));
                !origin.pages.is_empty() || origin.kept_shared
            });
        }
        for rule in &snapshot.rules {
            ensure!(
                credential_line_numbers(&rule.source_text).is_empty()
                    && credential_line_numbers(&rule.text).is_empty(),
                "A page rule may contain a credential."
            );
            let page = PersonalRulePageOrigin {
                location: rule.location.clone(),
                rule_id: rule.rule_id.clone(),
                author: rule.author.clone(),
                proposer: PAGES_PROPOSER.into(),
                confirmer: "person".into(),
            };
            let sentence = rule.text.trim();
            let bullet = format!("- {sentence}");
            if let Some((_, origin)) = lines
                .iter_mut()
                .find(|(line, _)| line.trim() == sentence || line.trim() == bullet)
            {
                let origin = origin.get_or_insert_with(|| page_origin(None, true));
                if origin.pages.is_empty() {
                    origin.kept_shared = true;
                }
                if !origin.pages.iter().any(|existing| {
                    existing.rule_id == page.rule_id
                        && existing.location.page_id == page.location.page_id
                }) {
                    origin.pages.push(page);
                }
                continue;
            }
            let mut origin = page_origin(Some(&page), false);
            origin.pages.push(page);
            add_under_heading(&mut lines, "How I work", bullet, origin);
        }
        let (text, origins): (Vec<_>, Vec<_>) = lines.into_iter().unzip();
        Ok((text.concat(), origins))
    }
}

fn page_origin(page: Option<&PersonalRulePageOrigin>, kept_shared: bool) -> PersonalRuleOrigin {
    PersonalRuleOrigin {
        author: page.and_then(|page| page.author.clone()),
        proposer: page.map(|page| page.proposer.clone()),
        confirmer: page.map(|page| page.confirmer.clone()),
        layer: PersonalRuleLayer::Shared,
        time: None,
        pages: Vec::new(),
        kept_shared,
    }
}

fn add_under_heading(
    lines: &mut Vec<(String, Option<PersonalRuleOrigin>)>,
    title: &str,
    bullet: String,
    origin: PersonalRuleOrigin,
) {
    let heading = format!("## {title}");
    // Keep the original headings when a rule is saved after migration.
    let at = lines
        .iter()
        .position(|(line, _)| line.trim() == heading)
        .map_or_else(
            || lines.len(),
            |start| {
                lines
                    .iter()
                    .enumerate()
                    .skip(start + 1)
                    .find(|(_, (line, _))| {
                        line.trim_start().starts_with("# ") || line.trim_start().starts_with("## ")
                    })
                    .map_or(lines.len(), |(at, _)| at)
            },
        );
    if let Some((line, _)) = lines.get_mut(at.wrapping_sub(1)) {
        if !line.ends_with('\n') {
            line.push('\n');
        }
    }
    if !lines.iter().any(|(line, _)| line.trim() == heading) {
        if !lines.is_empty() && !lines.last().is_some_and(|(line, _)| line.trim().is_empty()) {
            lines.push(("\n".into(), None));
        }
        lines.push((format!("{heading}\n"), None));
        lines.push(("\n".into(), None));
        lines.push((format!("{bullet}\n"), Some(origin)));
    } else {
        lines.insert(at, (format!("{bullet}\n"), Some(origin)));
    }
}

fn shared_line_matches(line: &str, sentence: &str) -> bool {
    let line = line.trim();
    line == sentence
        || line
            .strip_prefix("- ")
            .is_some_and(|rest| rest.trim() == sentence)
}

/// Shared rules with `sentence` added under "How I work", unless present.
pub(super) fn with_shared_line(shared: &str, sentence: &str) -> String {
    let sentence = sentence.trim();
    if sentence.is_empty()
        || shared
            .lines()
            .any(|line| shared_line_matches(line, sentence))
    {
        return shared.to_owned();
    }
    let mut lines: Vec<_> = shared
        .split_inclusive('\n')
        .map(|line| (line.to_owned(), None))
        .collect();
    add_under_heading(
        &mut lines,
        "How I work",
        format!("- {sentence}"),
        page_origin(None, false),
    );
    lines.into_iter().map(|(line, _)| line).collect()
}

/// Shared rules without the lines that say exactly `sentence`.
pub(super) fn without_shared_line(shared: &str, sentence: &str) -> String {
    let sentence = sentence.trim();
    shared
        .split_inclusive('\n')
        .filter(|line| !shared_line_matches(line, sentence))
        .collect()
}

impl Setup {
    pub(crate) fn mark_empty_page_rules(&self) -> Result<()> {
        let _lock = self.lock()?;
        let mut manifest = self.load_if_present()?.unwrap_or_else(|| {
            let mut manifest = empty_manifest();
            manifest.settings_only = true;
            manifest
        });
        if manifest.page_rules.is_unanswered() {
            settled(&manifest)?;
            manifest.page_rules = PageRulesState::Merged;
            self.save(&manifest)?;
        }
        Ok(())
    }
}

/// Data-only read. It never opens a tool home or prepares a renderer.
pub(crate) fn page_rules_merged() -> Result<bool, ServerError> {
    Ok(page_rules_state()? == PageRulesState::Merged)
}

pub(crate) fn page_rules_unanswered() -> Result<bool, ServerError> {
    Ok(page_rules_state()?.is_unanswered())
}

fn page_rules_state() -> Result<PageRulesState, ServerError> {
    let setup = data_only_setup();
    if !setup
        .data
        .join(storage::MANIFEST)
        .try_exists()
        .map_err(|error| server_error(error.into()))?
    {
        return Ok(PageRulesState::Unanswered);
    }
    let root = setup.data_files().map_err(server_error)?;
    Ok(storage::StoredSetup::read(&root)
        .map_err(server_error)?
        .map_or_else(PageRulesState::default, |setup| setup.state.page_rules))
}

/// After the move, a page edit to an every-Space rule must also change shared
/// rules. Refuse it before the page is saved when that cannot happen now.
pub(crate) fn page_change_ready() -> Result<(), ServerError> {
    let refuse = |error: anyhow::Error| ServerError::Conflict(error.to_string());
    let Some(manifest) = data_only_setup().load_if_present().map_err(server_error)? else {
        return Ok(());
    };
    if manifest.page_rules != PageRulesState::Merged {
        return Ok(());
    }
    page_change_settled(&manifest).map_err(refuse)
}

pub(super) fn page_change_settled(manifest: &Manifest) -> Result<()> {
    settled(manifest)?;
    ensure!(
        manifest
            .active_deliveries()
            .all(|(_, receipt)| !matches!(receipt.kind, Kind::Rule { .. })
                || receipt.observed.is_none()),
        "A tool's rule file needs review first. Review it in Rules, then edit this page again."
    );
    Ok(())
}

pub(crate) fn merged_page_rules(
) -> Result<Option<Vec<(String, PersonalRulePageOrigin)>>, ServerError> {
    if !page_rules_merged()? {
        return Ok(None);
    }
    read_merged_rules().map_err(server_error)
}

fn read_merged_rules() -> Result<Option<Vec<(String, PersonalRulePageOrigin)>>> {
    let Some(manifest) = data_only_setup().load_if_present()? else {
        return Ok(None);
    };
    if manifest.page_rules != PageRulesState::Merged {
        return Ok(None);
    }
    let shared = &manifest.master.shared;
    let origins = origin_lines(&manifest, &PersonalRuleLayer::Shared, shared)?;
    Ok(Some(
        shared
            .lines()
            .zip(origins)
            .filter_map(|(line, origin)| {
                let page = origin?.pages.into_iter().next()?;
                let line = line.trim();
                Some((line.strip_prefix("- ").unwrap_or(line).to_string(), page))
            })
            .collect(),
    ))
}

pub(super) fn data_only_setup() -> Setup {
    Setup::new(
        PathBuf::new(),
        wenlan_core::config::data_root().join("rulesync"),
    )
}
