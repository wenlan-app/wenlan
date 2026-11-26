// SPDX-License-Identifier: Apache-2.0
//! Preview and commit personal shared and per-tool rule changes.

mod pages;
#[cfg(test)]
mod tests;
pub(crate) use pages::{
    merged_page_rules, page_change_ready, page_rules_merged, page_rules_unanswered, PageRule,
    PageRuleSnapshot, PageRulesState,
};
use pages::{PagePlan, MERGE_CAUSE, PAGES_PROPOSER, PAGE_CHANGE_CAUSE};

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Path as RoutePath;
use serde::Serialize;
use uuid::Uuid;
use wenlan_core::rulesync_rule_plan::{
    confirmed_personal_rule_origins, personal_rule_content_digest, personal_rule_edit_diffs,
    preview_personal_rule_edits,
};
use wenlan_types::personal_rules::{
    PersonalRuleApplyRequest, PersonalRuleEdit, PersonalRuleLayer, PersonalRuleLayerOrigins,
    PersonalRuleOrigin, PersonalRulePlan, PersonalRuleProposeRequest, PersonalRuleShow,
    PersonalRuleShowRequest, PersonalRuleVersion, PersonalRuleVersionSummary,
};

use super::*;

const PLAN_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_PLANS: usize = 32;
const HISTORY_DIRECTORY: &str = "rule-history";

static PLANS: LazyLock<Mutex<BTreeMap<Uuid, HeldPlan>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[derive(Clone)]
struct HeldPlan {
    home: PathBuf,
    data: PathBuf,
    created: Instant,
    response: PersonalRulePlan,
    edits: Vec<PersonalRuleEdit>,
    signature: String,
    restore: Option<PersonalRuleVersion>,
    pages: Option<PagePlan>,
}

struct Prepared {
    plan: PersonalRulePlan,
    operations: Vec<Pending>,
    next_master: PersonalMaster,
    next_authored: BTreeMap<PathBuf, restore_rules::AuthoredRules>,
    signature: String,
    page_origins: Option<Vec<Option<PersonalRuleOrigin>>>,
}

struct PlanEdits<'a> {
    edits: &'a [PersonalRuleEdit],
    pages: Option<&'a PagePlan>,
    restore: Option<&'a PersonalRuleVersion>,
}

/// The data-root files are private, and AI callers may only read or propose
/// through a local daemon. A hosted team caller never sees personal rules.
fn require_local_reader(headers: &HeaderMap) -> Result<(), ServerError> {
    if !matches!(
        crate::security::bind_scope_from_env(),
        crate::security::BindScope::Unset | crate::security::BindScope::Loopback
    ) || !crate::team_actor::can_read_private_memories(headers)
    {
        return Err(ServerError::ValidationError(
            "Personal rules are available only to this computer's person and AI tools.".into(),
        ));
    }
    Ok(())
}

fn proposer(headers: &HeaderMap) -> Result<Option<String>, ServerError> {
    if !ai_caller(headers) {
        return Ok(None);
    }
    let name = headers
        .get("x-agent-name")
        .and_then(|name| name.to_str().ok())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            crate::session_routes::extract_session_ref(headers).map(|session| session.tool)
        });
    let Some(name) = name else {
        // A session without a tool name is still an AI caller. Do not invent
        // the tool's identity in history.
        return Ok(None);
    };
    if name
        .chars()
        .any(|ch| ch.is_control() || wenlan_core::skill_registry::is_invisible(ch))
        || !credential_line_numbers(&name).is_empty()
    {
        return Err(ServerError::ValidationError("Invalid AI tool name.".into()));
    }
    Ok(Some(name))
}

fn empty_master() -> PersonalMaster {
    PersonalMaster {
        shared: String::new(),
        overlays: BTreeMap::new(),
        unenrolled_overlays: BTreeSet::new(),
        claude_imported_rules: None,
        codex_sync_replaced: false,
        skills: Vec::new(),
        overrides: BTreeMap::new(),
    }
}

fn empty_manifest() -> Manifest {
    Manifest::new(empty_master(), BTreeMap::new())
}

fn canonical_layer(layer: PersonalRuleLayer) -> Result<PersonalRuleLayer> {
    let PersonalRuleLayer::Tool { tool } = layer else {
        return Ok(PersonalRuleLayer::Shared);
    };
    let alias = match tool.to_ascii_lowercase().as_str() {
        "claude" => "claudecode",
        "codex" => "codexcli",
        "gemini" => "geminicli",
        _ => tool.as_str(),
    };
    let target = PERSONAL_TARGETS
        .iter()
        .find(|target| {
            target.id.eq_ignore_ascii_case(alias)
                || target.agent.eq_ignore_ascii_case(alias)
                || target.tool.display_name().eq_ignore_ascii_case(alias)
        })
        .context("Unknown AI tool.")?;
    Ok(PersonalRuleLayer::Tool {
        tool: target.id.to_owned(),
    })
}

fn canonical_edits(edits: Vec<PersonalRuleEdit>) -> Result<Vec<PersonalRuleEdit>> {
    edits
        .into_iter()
        .map(|edit| {
            Ok(PersonalRuleEdit {
                layer: canonical_layer(edit.layer)?,
                text: edit.text,
            })
        })
        .collect()
}

fn origin_lines(
    manifest: &Manifest,
    layer: &PersonalRuleLayer,
    text: &str,
) -> Result<Vec<Option<PersonalRuleOrigin>>> {
    let digest = personal_rule_content_digest(layer, text)?;
    let count = text.split_inclusive('\n').count();
    Ok(manifest
        .personal_rule_origins
        .iter()
        .find(|record| {
            record.layer == *layer
                && record.content_digest == digest
                && record.lines.len() == count
                && record
                    .lines
                    .iter()
                    .flatten()
                    .all(|origin| origin.layer == *layer)
        })
        .map_or_else(|| vec![None; count], |record| record.lines.clone()))
}

fn show_layer(manifest: &Manifest, layer: &PersonalRuleLayer) -> Result<PersonalRuleShow> {
    let pending_scope = match layer {
        PersonalRuleLayer::Tool { tool } => personal_target(tool).is_some_and(|target| {
            manifest
                .current_scope_reviews()
                .any(|review| review.path == Path::new(target.rules.destination))
        }),
        PersonalRuleLayer::Shared => false,
    };
    let master = materialize_confirmed_tool_overrides(manifest);
    let text = match layer {
        PersonalRuleLayer::Shared => &master.shared,
        PersonalRuleLayer::Tool { tool } => {
            ensure!(personal_target(tool).is_some(), "Unknown AI tool.");
            ensure!(
                pending_scope || !master.overrides.contains_key(tool),
                "This tool has an older tool-only edit that cannot be shown as a separate layer. It was kept unchanged; reconcile it before changing personal rules."
            );
            master.overlays.get(tool).map_or("", String::as_str)
        }
    };
    let origins = origin_lines(manifest, layer, text)?;
    let mut tools = BTreeSet::new();
    for (_, delivery) in manifest.active_deliveries() {
        if let Kind::Rule { target, .. } = &delivery.kind {
            if matches!(layer, PersonalRuleLayer::Shared)
                || matches!(layer, PersonalRuleLayer::Tool { tool } if tool == target)
            {
                tools.insert(target.clone());
            }
        }
    }
    Ok(PersonalRuleShow {
        layer: layer.clone(),
        text: text.to_owned(),
        origins,
        affected_tools: tools.into_iter().collect(),
    })
}

#[derive(Serialize)]
struct PlanBinding<'a> {
    shared: &'a str,
    overlays: &'a BTreeMap<String, String>,
    unenrolled_overlays: &'a BTreeSet<String>,
    overrides: &'a BTreeMap<String, LocalText>,
    codex_sync_replaced: bool,
    claude_imported_rules: &'a Option<BTreeSet<String>>,
    generated: &'a [String],
    page_rules: PageRulesState,
    pages: Option<&'a PagePlan>,
    origins: &'a [PersonalRuleLayerOrigins],
    history: &'a [String],
    authored: &'a BTreeMap<PathBuf, restore_rules::AuthoredRules>,
    changes: &'a [wenlan_types::personal_rules::PersonalRulePlanChange],
    proposer: &'a Option<String>,
    operations: &'a [Pending],
    blocked: &'a [String],
    unmanaged: &'a [String],
}

fn settled(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.pending.is_empty()
            && manifest.stages.is_empty()
            && manifest.rule_share.is_none()
            && manifest.extension_paths.is_empty()
            && manifest.settings_batch.is_none()
            && manifest.recovery_issues.is_empty()
            && manifest.personal_rule_journal.is_none(),
        "A setup write is pending. Finish or undo it before changing rules."
    );
    Ok(())
}

impl Setup {
    #[cfg(test)]
    fn prepare_rule_plan(
        &self,
        manifest: &Manifest,
        edits: &[PersonalRuleEdit],
        generated: &[String],
        proposer: Option<String>,
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<Prepared> {
        self.prepare_plan(
            manifest,
            PlanEdits {
                edits,
                pages: None,
                restore: None,
            },
            generated,
            proposer,
            render,
        )
    }

    fn prepare_plan(
        &self,
        manifest: &Manifest,
        request: PlanEdits<'_>,
        generated: &[String],
        proposer: Option<String>,
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<Prepared> {
        settled(manifest)?;
        ensure!(
            manifest.current_scope_reviews().next().is_none(),
            "A tool rule edit is waiting for review. Resolve it before changing personal rules."
        );
        let current_master = materialize_confirmed_tool_overrides(manifest);
        ensure!(
            current_master.overrides.is_empty(),
            "An older tool-only edit cannot be represented as a separate rule layer. It was kept unchanged; reconcile it before changing personal rules."
        );
        let page_edit;
        let mut page_origins = None;
        let edits = if let Some(pages) = request.pages {
            let (text, origins) = pages.prepare(manifest)?;
            page_origins = Some(origins);
            page_edit = [PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text,
            }];
            &page_edit
        } else {
            request.edits
        };
        let mut changes = if request.pages.is_some() || request.restore.is_some() {
            personal_rule_edit_diffs(&current_master, edits)?
        } else {
            preview_personal_rule_edits(&current_master, edits)?
        };
        if let Some(version) = request.restore {
            for rule in &version.rules {
                if changes.iter().any(|change| change.layer == rule.layer) {
                    continue;
                }
                let current = show_layer(manifest, &rule.layer)?;
                if current.text == rule.text && current.origins != rule.origins {
                    changes.push(provenance_change(rule.layer.clone(), &rule.text));
                }
            }
            ensure!(!changes.is_empty(), "There are no rule changes to propose.");
        }
        if let Some(origins) = &page_origins {
            if changes.is_empty()
                && (request.pages.is_some_and(PagePlan::is_merge)
                    || origin_lines(manifest, &PersonalRuleLayer::Shared, &current_master.shared)?
                        != *origins)
            {
                changes.push(provenance_change(
                    PersonalRuleLayer::Shared,
                    &current_master.shared,
                ));
            }
        }
        let mut next_master = current_master.clone();
        let mut changed_tools = BTreeSet::new();
        let mut shared_changed = false;
        for change in &changes {
            match &change.layer {
                PersonalRuleLayer::Shared => {
                    next_master.shared.clone_from(&change.after);
                    shared_changed = true;
                }
                PersonalRuleLayer::Tool { tool } => {
                    if !next_master.enrolls_target(tool) {
                        next_master.unenrolled_overlays.insert(tool.clone());
                    }
                    next_master
                        .overlays
                        .insert(tool.clone(), change.after.clone());
                    changed_tools.insert(tool.clone());
                }
            }
        }

        // A merge moves the page lines into shared now; the lines still
        // generated after it (conversation preferences) keep being delivered.
        let delivery_state = if request.pages.is_some_and(PagePlan::is_merge) {
            PageRulesState::Merged
        } else {
            manifest.page_rules
        };
        let delivery_generated = &delivery_state.legacy_rules(&next_master.shared, generated);
        let root = self.home_files()?;
        let mut worktrees = DeliveryWorktrees::new(&self.home)?;
        let mut operations = Vec::new();
        let mut blocked = Vec::new();
        let mut next_authored = manifest.authored_rules.clone();
        // A legacy override can be folded by this plan even when its tool is
        // not being delivered. Preserve its old donor before normalizing it.
        for (path, receipt) in manifest.active_deliveries() {
            if !matches!(receipt.kind, Kind::Rule { .. }) {
                continue;
            }
            let expected = receipt.observed.as_ref().unwrap_or(&receipt.delivered);
            match restore_rules::AuthoredRules::for_tool_edit(manifest, path, expected)? {
                Some(authored) => {
                    next_authored.insert(path.clone(), authored);
                }
                None => {
                    next_authored.remove(path);
                }
            }
        }
        for (path, authored) in &next_authored {
            let Some(Delivery {
                kind: Kind::Rule { target, .. },
                ..
            }) = manifest.delivered.get(path)
            else {
                continue;
            };
            let before = current_master.rule_text(target, &[])?;
            let after = next_master.rule_text(target, &[])?;
            for key in authored.line_keys().filter(|line| !line.trim().is_empty()) {
                if !before.lines().any(|line| line == key) || after.lines().any(|line| line == key)
                {
                    continue;
                }
                let added_elsewhere = next_master.overlays.iter().any(|(tool, overlay)| {
                    if tool == target {
                        return false;
                    }
                    let old = current_master.overlays.get(tool).map_or("", String::as_str);
                    overlay.lines().filter(|line| *line == key).count()
                        > old.lines().filter(|line| *line == key).count()
                });
                ensure!(
                    !added_elsewhere,
                    "A rule moving between tools has an unclear donor origin. The rules were left unchanged."
                );
            }
        }
        for (path, receipt) in manifest.active_deliveries() {
            let Kind::Rule { target, .. } = &receipt.kind else {
                continue;
            };
            if !shared_changed && !changed_tools.contains(target) {
                continue;
            }
            let target_name =
                personal_target(target).map_or(target.as_str(), |item| item.tool.display_name());
            let expected = receipt.observed.as_ref().unwrap_or(&receipt.delivered);
            let live = root.read(path, false);
            if receipt.observed.is_some() || !live.is_ok_and(|snapshot| snapshot == *expected) {
                blocked.push(format!("{target_name}: review its local rule file first."));
                continue;
            }
            if snapshot_has_credentials(expected) {
                blocked.push(format!(
                    "{target_name}: its rule file may contain a credential."
                ));
                continue;
            }
            let source_text = next_master
                .rule_text(target, delivery_generated)
                .map_err(|_| {
                    anyhow::anyhow!("{target_name}: the rule change conflicts with a local edit.")
                })?;
            if let Some(authored) = next_authored.get_mut(path) {
                *authored = authored.for_plan(
                    &source_text,
                    &manifest.master.shared,
                    &next_master.shared,
                    current_master
                        .overlays
                        .get(target)
                        .map_or("", String::as_str),
                    next_master.overlays.get(target).map_or("", String::as_str),
                )?;
            }
            let output = render(target, &source_text)
                .map_err(|_| anyhow::anyhow!("Could not render rules for {target_name}."))?;
            let bytes = output.get(path).with_context(|| {
                format!(
                    "Could not render the managed file {} for {target_name}.",
                    path.display()
                )
            })?;
            let desired = with_bytes(expected, bytes.clone());
            if snapshot_has_credentials(&desired) {
                blocked.push(format!(
                    "{target_name}: the rendered rules may contain a credential."
                ));
                continue;
            }
            if expected != &desired {
                if let Some(reason) = worktrees.unreviewed(path, &manifest.worktree_writes)? {
                    blocked.push(format!("{target_name}: {reason}"));
                    continue;
                }
            }
            operations.push(Pending {
                path: path.clone(),
                seen: expected.clone(),
                desired,
                kind: Some(Kind::rule(
                    &next_master,
                    target,
                    delivery_generated,
                    source_text,
                )),
                allow_link: false,
            });
        }
        for change in &mut changes {
            change.affected_tools = manifest
                .active_deliveries()
                .filter_map(|(_, receipt)| match &receipt.kind {
                    Kind::Rule { target, .. }
                        if matches!(change.layer, PersonalRuleLayer::Shared)
                            || matches!(&change.layer, PersonalRuleLayer::Tool { tool } if tool == target) =>
                    {
                        Some(target.clone())
                    }
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        let mut unmanaged = BTreeSet::new();
        for change in &changes {
            if let PersonalRuleLayer::Tool { tool } = &change.layer {
                if change.affected_tools.is_empty() {
                    unmanaged.insert(tool.clone());
                }
            }
        }
        if shared_changed {
            for tool in &manifest.master.unenrolled_overlays {
                if !changes
                    .iter()
                    .any(|change| change.affected_tools.contains(tool))
                {
                    unmanaged.insert(tool.clone());
                }
            }
        }
        let unmanaged: Vec<_> = unmanaged.into_iter().collect();
        let binding = PlanBinding {
            shared: &manifest.master.shared,
            overlays: &manifest.master.overlays,
            unenrolled_overlays: &manifest.master.unenrolled_overlays,
            overrides: &manifest.master.overrides,
            codex_sync_replaced: manifest.master.codex_sync_replaced,
            claude_imported_rules: &manifest.master.claude_imported_rules,
            generated,
            page_rules: manifest.page_rules,
            pages: request.pages,
            origins: &manifest.personal_rule_origins,
            history: &manifest.personal_rule_history,
            authored: &manifest.authored_rules,
            changes: &changes,
            proposer: &proposer,
            operations: &operations,
            blocked: &blocked,
            unmanaged: &unmanaged,
        };
        let signature = fingerprint(&binding)?;
        Ok(Prepared {
            plan: PersonalRulePlan {
                id: String::new(),
                expires_at: 0,
                proposer,
                changes,
                blocked_tools: blocked,
                unmanaged_tools: unmanaged,
                page_merge: request.pages.and_then(PagePlan::preview),
            },
            operations,
            next_master,
            next_authored,
            signature,
            page_origins,
        })
    }
}

fn provenance_change(
    layer: PersonalRuleLayer,
    text: &str,
) -> wenlan_types::personal_rules::PersonalRulePlanChange {
    wenlan_types::personal_rules::PersonalRulePlanChange {
        layer,
        before: text.into(),
        after: text.into(),
        diff: String::new(),
        affected_tools: Vec::new(),
    }
}

fn rendering_setup() -> Result<Setup> {
    let setup = Setup::system()?;
    let has_rule_destinations = setup.load_if_present()?.is_some_and(|manifest| {
        manifest
            .active_deliveries()
            .any(|(_, delivery)| matches!(delivery.kind, Kind::Rule { .. }))
    });
    if has_rule_destinations {
        setup.ready()
    } else {
        Ok(setup)
    }
}

fn server_error(error: anyhow::Error) -> ServerError {
    if let Some(credential) =
        error.downcast_ref::<wenlan_core::rulesync_rule_plan::PersonalRuleCredential>()
    {
        let layer = match &credential.layer {
            PersonalRuleLayer::Shared => "shared",
            PersonalRuleLayer::Tool { tool } => tool.as_str(),
        };
        return ServerError::Conflict(format!(
            "personal_rule_credential: layer={layer} line={}",
            credential.line
        ));
    }
    ServerError::Conflict(format!("{error:#}"))
}

pub(super) async fn show(
    headers: HeaderMap,
    Json(request): Json<PersonalRuleShowRequest>,
) -> Result<Json<PersonalRuleShow>, ServerError> {
    require_local_reader(&headers)?;
    tokio::task::spawn_blocking(move || {
        let setup = Setup::system()?;
        let manifest = setup.load_if_present()?.unwrap_or_else(empty_manifest);
        show_layer(&manifest, &canonical_layer(request.layer)?)
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

pub(super) async fn propose(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<PersonalRuleProposeRequest>,
) -> Result<Json<PersonalRulePlan>, ServerError> {
    require_local_reader(&headers)?;
    let tool = proposer(&headers)?;
    let generated = crate::skill_grow::general_tool_rules(&state).await?;
    tokio::task::spawn_blocking(move || {
        let setup = rendering_setup()?;
        build_plan(
            &setup,
            canonical_edits(request.edits)?,
            &generated,
            tool,
            None,
        )
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

pub(super) async fn plan(
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<PersonalRulePlan>, ServerError> {
    require_local_reader(&headers)?;
    tokio::task::spawn_blocking(move || {
        let setup = Setup::system()?;
        Ok::<_, anyhow::Error>(find_plan(&setup, &id, false)?.response)
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

pub(super) async fn apply(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<PersonalRuleApplyRequest>,
) -> Result<Json<PersonalRuleVersion>, ServerError> {
    require_local_person(&headers)?;
    let setup = pages_setup().map_err(server_error)?;
    let held = find_plan(&setup, &request.plan_id, false).map_err(server_error)?;
    let _pages_lock = if held.pages.is_some() {
        Some(crate::skill_grow::SKILLS_MUTATION_LOCK.lock().await)
    } else {
        None
    };
    let merge = match &held.pages {
        Some(PagePlan::Merge(_)) => {
            Some(crate::skill_grow::page_rules::merge_snapshot(&state).await?)
        }
        Some(PagePlan::Change { snapshot, .. }) => {
            if !crate::skill_grow::page_rules::revisions_match(&state, &snapshot.revisions).await? {
                find_plan(&setup, &request.plan_id, true).map_err(server_error)?;
                return Err(ServerError::Conflict(
                    "A page changed after the preview. Propose the change again.".into(),
                ));
            }
            None
        }
        None => None,
    };
    let generated = crate::skill_grow::general_tool_rules(&state).await?;
    tokio::task::spawn_blocking(move || {
        let setup = rendering_setup()?;
        let layers = rulesync_team::load_layers(&wenlan_core::shared_space::settings_path())?;
        let session = RenderSession::new(&setup.binary);
        setup.apply_plan(&request.plan_id, &generated, merge, &|tool, text| {
            setup.render_member_rule(&session, tool, text, &layers)
        })
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

fn pages_setup() -> Result<Setup> {
    Ok(Setup::new(
        Setup::tool_home()?,
        wenlan_core::config::data_root().join("rulesync"),
    ))
}

pub(super) async fn preview_merge_pages(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<Option<PersonalRulePlan>>, ServerError> {
    require_local_reader(&headers)?;
    Ok(Json(merge_preview(&state).await?))
}

pub(super) async fn merge_preview(
    state: &SharedState,
) -> Result<Option<PersonalRulePlan>, ServerError> {
    let setup = pages_setup().map_err(server_error)?;
    let manifest = setup
        .load_if_present()
        .map_err(server_error)?
        .unwrap_or_else(empty_manifest);
    if !manifest.page_rules.is_unanswered() {
        return Ok(None);
    }
    let snapshot = crate::skill_grow::page_rules::merge_snapshot(state).await?;
    if !snapshot.has_candidates() {
        return Ok(None);
    }
    // Bound to the same lines the apply reads before the move.
    let generated = crate::skill_grow::general_tool_rules(state).await?;
    tokio::task::spawn_blocking(move || {
        let page_plan = PagePlan::Merge(snapshot);
        let layers = rulesync_team::load_layers(&wenlan_core::shared_space::settings_path())?;
        let session = RenderSession::new(&setup.binary);
        let prepared = setup.prepare_page_plan(
            &manifest,
            &page_plan,
            &generated,
            Some(PAGES_PROPOSER.into()),
            &|tool, text| setup.render_member_rule(&session, tool, text, &layers),
        )?;
        Ok(Some(remember_plan(
            &setup,
            prepared,
            Vec::new(),
            None,
            Some(page_plan),
        )))
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map_err(server_error)
}

pub(super) async fn decline_merge_pages(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<PersonalRuleApplyRequest>,
) -> Result<Json<()>, ServerError> {
    require_local_person(&headers)?;
    let _serial = crate::skill_grow::SKILLS_MUTATION_LOCK.lock().await;
    let snapshot = crate::skill_grow::page_rules::merge_snapshot(&state).await?;
    tokio::task::spawn_blocking(move || {
        let setup = pages_setup()?;
        setup.decline_page_plan(&request.plan_id, &snapshot)
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

impl Setup {
    fn prepare_page_plan(
        &self,
        manifest: &Manifest,
        pages: &PagePlan,
        generated: &[String],
        proposer: Option<String>,
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<Prepared> {
        self.prepare_plan(
            manifest,
            PlanEdits {
                edits: &[],
                pages: Some(pages),
                restore: None,
            },
            generated,
            proposer,
            render,
        )
    }

    fn decline_page_plan(&self, id: &str, current: &PageRuleSnapshot) -> Result<()> {
        let _lock = self.lock()?;
        let held = find_plan(self, id, true)?;
        let Some(PagePlan::Merge(previous)) = held.pages else {
            bail!("This is not a page rule move.")
        };
        let mut manifest = self.load_if_present()?.unwrap_or_else(empty_manifest);
        settled(&manifest)?;
        ensure!(
            manifest.page_rules.is_unanswered() && fingerprint(&previous)? == fingerprint(current)?,
            "The page rules changed after the preview. Preview the move again."
        );
        // Declining changes only the offer. Existing tool-file edits need no
        // resolution for the person to keep their current delivery.
        manifest.page_rules = PageRulesState::Declined;
        self.save(&manifest)
    }
}

pub(super) fn pending_page_plans(setup: &Setup) -> Vec<PersonalRulePlan> {
    let mut plans = PLANS.lock().unwrap_or_else(|error| error.into_inner());
    plans.retain(|_, plan| plan.created.elapsed() < PLAN_LIFETIME);
    plans
        .values()
        .filter(|plan| {
            plan.data == setup.data
                && plan.home == setup.home
                && matches!(plan.pages, Some(PagePlan::Change { .. }))
        })
        .map(|plan| plan.response.clone())
        .collect()
}

/// After the move, a tool's save or removal of a rule used in every Space
/// becomes a personal-rule plan for shared rules. No page changes, so a page
/// never holds a rule the tools are not told while the person decides.
pub(crate) async fn propose_shared_line(
    headers: &HeaderMap,
    generated: Vec<String>,
    line: String,
    add: bool,
) -> Result<String, ServerError> {
    require_local_reader(headers)?;
    let tool = proposer(headers)?;
    tokio::task::spawn_blocking(move || {
        let setup = rendering_setup()?;
        let manifest = setup.load_if_present()?.unwrap_or_else(empty_manifest);
        ensure!(
            manifest.page_rules == PageRulesState::Merged,
            "Page rules have not been moved."
        );
        let shared = &manifest.master.shared;
        let text = if add {
            pages::with_shared_line(shared, &line)
        } else {
            pages::without_shared_line(shared, &line)
        };
        ensure!(
            text != *shared,
            if add {
                "This is already a rule used in every Space; nothing was proposed."
            } else {
                "No rule used in every Space has these words; nothing was proposed."
            }
        );
        let edits = vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text,
        }];
        Ok(build_plan(&setup, edits, &generated, tool, None)?.id)
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map_err(|error| ServerError::Conflict(error.to_string()))
}

/// Called only by page actions that have already enforced their scope.
/// The exact local-person gate is the authority for an immediate apply.
pub(crate) async fn page_change(
    headers: &HeaderMap,
    generated: Vec<String>,
    snapshot: PageRuleSnapshot,
    remove: BTreeSet<String>,
) -> Result<Option<String>, ServerError> {
    let proposer = Some(PAGES_PROPOSER.into());
    change_shared_lines(headers, generated, snapshot, remove, proposer).await
}

/// After the move, a confirmed preference becomes one shared line. The
/// person's own confirm applies it at once; a tool's becomes a plan for the
/// person to confirm. `proposer` is the tool the preference came from.
pub(crate) async fn preference_change(
    headers: &HeaderMap,
    mut snapshot: PageRuleSnapshot,
    from_tool: Option<String>,
) -> Result<Option<String>, ServerError> {
    // A saved tool name that could not be shown safely is not kept: the line
    // is still the person's, and an unsafe name would make every later
    // rules read fail.
    let proposer = match from_tool.filter(|tool| safe_tool_name(tool)) {
        Some(tool) => Some(tool),
        None => proposer(headers)?,
    };
    for rule in &mut snapshot.rules {
        rule.proposer.clone_from(&proposer);
    }
    change_shared_lines(headers, Vec::new(), snapshot, BTreeSet::new(), proposer).await
}

async fn change_shared_lines(
    headers: &HeaderMap,
    generated: Vec<String>,
    snapshot: PageRuleSnapshot,
    remove: BTreeSet<String>,
    proposer: Option<String>,
) -> Result<Option<String>, ServerError> {
    require_local_reader(headers)?;
    let confirmed = require_local_person(headers).is_ok();
    if snapshot.rules.is_empty() && remove.is_empty() {
        return Ok(None);
    }
    tokio::task::spawn_blocking(move || {
        let setup = rendering_setup()?;
        let layers = rulesync_team::load_layers(&wenlan_core::shared_space::settings_path())?;
        let session = RenderSession::new(&setup.binary);
        let change = PageChange {
            snapshot,
            remove,
            confirmed,
            proposer,
        };
        setup.change_page_rules(&generated, change, &|tool, text| {
            setup.render_member_rule(&session, tool, text, &layers)
        })
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map_err(server_error)
}

/// One change to the page-backed shared lines, and whether the local person
/// is the one asking for it.
pub(super) struct PageChange {
    pub snapshot: PageRuleSnapshot,
    pub remove: BTreeSet<String>,
    pub confirmed: bool,
    pub proposer: Option<String>,
}

impl Setup {
    fn change_page_rules(
        &self,
        generated: &[String],
        change: PageChange,
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<Option<String>> {
        let PageChange {
            snapshot,
            remove,
            confirmed,
            proposer,
        } = change;
        let _lock = self.lock()?;
        let manifest = self.load_if_present()?.unwrap_or_else(empty_manifest);
        let pages = PagePlan::Change { snapshot, remove };
        let prepared = self.prepare_page_plan(&manifest, &pages, generated, proposer, render)?;
        if prepared.plan.changes.is_empty() {
            return Ok(None);
        }
        let plan = remember_plan(self, prepared, Vec::new(), None, Some(pages));
        if confirmed {
            let held = find_plan(self, &plan.id, true)?;
            self.commit_rule_plan(&held, generated, render)
                .context("The page was saved, but shared rules did not change")?;
            Ok(None)
        } else {
            Ok(Some(plan.id))
        }
    }
}

pub(super) async fn history(
    headers: HeaderMap,
) -> Result<Json<Vec<PersonalRuleVersionSummary>>, ServerError> {
    require_local_reader(&headers)?;
    tokio::task::spawn_blocking(|| {
        let setup = Setup::system()?;
        let manifest = setup.load_if_present()?.unwrap_or_else(empty_manifest);
        manifest
            .personal_rule_history
            .iter()
            .map(|id| Ok(read_version(&setup, &manifest, id)?.summary))
            .collect::<Result<Vec<_>>>()
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

pub(super) async fn history_version(
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<PersonalRuleVersion>, ServerError> {
    require_local_reader(&headers)?;
    tokio::task::spawn_blocking(move || {
        let setup = Setup::system()?;
        let manifest = setup.load_if_present()?.unwrap_or_else(empty_manifest);
        read_version(&setup, &manifest, &id)
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

pub(super) async fn propose_restore(
    State(state): State<SharedState>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<PersonalRulePlan>, ServerError> {
    require_local_reader(&headers)?;
    let tool = proposer(&headers)?;
    let generated = crate::skill_grow::general_tool_rules(&state).await?;
    tokio::task::spawn_blocking(move || {
        let setup = rendering_setup()?;
        let manifest = setup.load_if_present()?.unwrap_or_else(empty_manifest);
        let version = read_version(&setup, &manifest, &id)?;
        let edits = restore_edits(&manifest, &version)?;
        build_plan(&setup, edits, &generated, tool, Some(version))
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
}

fn fresh_plan_id(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).context("This plan has expired. Propose the change again.")
}

fn remember_plan(
    setup: &Setup,
    mut prepared: Prepared,
    edits: Vec<PersonalRuleEdit>,
    restore: Option<PersonalRuleVersion>,
    pages: Option<PagePlan>,
) -> PersonalRulePlan {
    let id = Uuid::new_v4();
    prepared.plan.id = id.simple().to_string();
    prepared.plan.expires_at = chrono::Utc::now().timestamp() + PLAN_LIFETIME.as_secs() as i64;
    let response = prepared.plan;
    let mut plans = PLANS.lock().unwrap_or_else(|error| error.into_inner());
    plans.retain(|_, plan| plan.created.elapsed() < PLAN_LIFETIME);
    if pages.as_ref().is_some_and(PagePlan::is_merge) {
        if let Some(plan) = plans.values().find(|plan| {
            plan.home == setup.home
                && plan.data == setup.data
                && plan.signature == prepared.signature
        }) {
            return plan.response.clone();
        }
    }
    if plans.len() >= MAX_PLANS {
        if let Some(oldest) = plans
            .iter()
            .min_by_key(|(_, plan)| plan.created)
            .map(|(id, _)| *id)
        {
            plans.remove(&oldest);
        }
    }
    plans.insert(
        id,
        HeldPlan {
            home: setup.home.clone(),
            data: setup.data.clone(),
            created: Instant::now(),
            response: response.clone(),
            edits,
            signature: prepared.signature,
            restore,
            pages,
        },
    );
    response
}

fn find_plan(setup: &Setup, id: &str, consume: bool) -> Result<HeldPlan> {
    let id = fresh_plan_id(id)?;
    let mut plans = PLANS.lock().unwrap_or_else(|error| error.into_inner());
    plans.retain(|_, plan| plan.created.elapsed() < PLAN_LIFETIME);
    let held = plans
        .get(&id)
        .filter(|held| held.home == setup.home && held.data == setup.data)
        .context("This plan has expired. Propose the change again.")?;
    if !consume {
        return Ok(held.clone());
    }
    Ok(plans.remove(&id).expect("the matching plan is present"))
}

fn build_plan(
    setup: &Setup,
    edits: Vec<PersonalRuleEdit>,
    generated: &[String],
    proposer: Option<String>,
    restore: Option<PersonalRuleVersion>,
) -> Result<PersonalRulePlan> {
    let _lock = setup.lock()?;
    let manifest = setup.load_if_present()?.unwrap_or_else(empty_manifest);
    let layers = rulesync_team::load_layers(&wenlan_core::shared_space::settings_path())?;
    let session = RenderSession::new(&setup.binary);
    let prepared = setup.prepare_plan(
        &manifest,
        PlanEdits {
            edits: &edits,
            pages: None,
            restore: restore.as_ref(),
        },
        generated,
        proposer,
        &|tool, text| setup.render_member_rule(&session, tool, text, &layers),
    )?;
    Ok(remember_plan(setup, prepared, edits, restore, None))
}

/// History files are written before the manifest checkpoint. This intent is
/// persisted first so recovery can remove only this uncommitted version's
/// exact files if a crash or conflict interrupts the operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleHistoryJournal {
    files: BTreeMap<String, Vec<u8>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    archives: BTreeMap<String, Vec<u8>>,
}

fn history_path(id: &str) -> Result<PathBuf> {
    let parsed = Uuid::parse_str(id).context("Invalid rule version.")?;
    ensure!(parsed.simple().to_string() == id, "Invalid rule version.");
    Ok(Path::new(HISTORY_DIRECTORY).join(format!("v-{id}.json")))
}

fn archive_path(id: &str) -> Result<PathBuf> {
    let history = history_path(id)?;
    Ok(Path::new(HISTORY_DIRECTORY).join("archive").join(
        history
            .file_name()
            .context("Missing rule history file name")?,
    ))
}

impl RuleHistoryJournal {
    fn entries(&self) -> impl Iterator<Item = Result<(PathBuf, &Vec<u8>)>> {
        self.files
            .iter()
            .map(|(id, bytes)| Ok((history_path(id)?, bytes)))
            .chain(
                self.archives
                    .iter()
                    .map(|(id, bytes)| Ok((archive_path(id)?, bytes))),
            )
    }
}

fn write_history_files(setup: &Setup, journal: &RuleHistoryJournal) -> Result<()> {
    let root = setup.data_files()?.with_private_directories();
    let directory = Path::new(HISTORY_DIRECTORY);
    match root.read(directory, false)? {
        Snapshot::Absent => ensure!(
            root.apply(
                directory,
                &Snapshot::Absent,
                &Snapshot::private_directory(),
                false
            )? == ApplyOutcome::Applied,
            "Rule history changed while making its private folder."
        ),
        Snapshot::Directory { .. } => (),
        _ => bail!("The rule history folder is not a private directory."),
    }
    for entry in journal.entries() {
        let (path, bytes) = entry?;
        let desired = Snapshot::file(bytes.clone());
        match root.read_private(&path, None)? {
            Snapshot::Absent => ensure!(
                root.apply(&path, &Snapshot::Absent, &desired, false)? == ApplyOutcome::Applied,
                "Rule history changed while saving a version."
            ),
            current if current == desired => (),
            _ => bail!("A rule history version already exists with different contents."),
        }
    }
    if !journal.archives.is_empty() {
        root.sync_directory(&directory.join("archive"))?;
    }
    root.sync_directory(directory)?;
    root.sync_directory(Path::new(""))?;
    Ok(())
}

pub(super) fn discard_history_journal(setup: &Setup, manifest: &mut Manifest) -> Result<()> {
    let Some(journal) = &manifest.personal_rule_journal else {
        return Ok(());
    };
    let root = setup.data_files()?.with_private_directories();
    for entry in journal.entries() {
        let (path, bytes) = entry?;
        let expected = Snapshot::file(bytes.clone());
        match root.read_private(&path, None)? {
            Snapshot::Absent => (),
            actual if actual == expected => ensure!(
                root.apply(&path, &actual, &Snapshot::Absent, false)? == ApplyOutcome::Applied,
                "Rule history changed while undoing an interrupted change."
            ),
            _ => bail!("A rule history version changed during recovery. It was left in place."),
        }
    }
    if !journal.archives.is_empty() {
        root.sync_directory(&Path::new(HISTORY_DIRECTORY).join("archive"))?;
    }
    root.sync_directory(Path::new(HISTORY_DIRECTORY))?;
    manifest.personal_rule_journal = None;
    setup.save(manifest)
}

pub(super) fn read_version(
    setup: &Setup,
    manifest: &Manifest,
    id: &str,
) -> Result<PersonalRuleVersion> {
    ensure!(
        manifest
            .personal_rule_history
            .iter()
            .any(|known| known == id),
        "Rule version not found."
    );
    let root = setup.data_files()?.with_private_directories();
    let Snapshot::File { bytes, .. } = root.read_private(&history_path(id)?, None)? else {
        bail!("The rule history version is missing.");
    };
    let version: PersonalRuleVersion =
        serde_json::from_slice(&bytes).context("Invalid rule history version")?;
    ensure!(
        version.summary.id == id,
        "The rule history version does not match its id."
    );
    validate_version(&version)?;
    Ok(version)
}

fn safe_origin_value(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        !value
            .chars()
            .any(|ch| ch.is_control() || wenlan_core::skill_registry::is_invisible(ch))
            && credential_line_numbers(value).is_empty()
    })
}

/// A page line is proposed by the pages themselves. A confirmed preference
/// has no page and keeps the tool it came from (`wenlan-cli`, `claude-code`).
fn proposer_fits(page: &wenlan_types::personal_rules::PersonalRulePageOrigin) -> bool {
    let preference = page.location.page_id.is_none() && page.rule_id.starts_with("preference:");
    page.proposer == PAGES_PROPOSER || (preference && safe_tool_name(&page.proposer))
}

fn safe_tool_name(name: &str) -> bool {
    !name.trim().is_empty() && safe_origin_value(Some(name))
}

fn safe_page_origins(origin: &PersonalRuleOrigin) -> bool {
    origin.pages.iter().all(|page| {
        origin.layer == PersonalRuleLayer::Shared
            && page.location.line != 0
            && safe_origin_value(page.location.page_id.as_deref())
            && safe_origin_value(Some(&page.rule_id))
            && safe_origin_value(page.author.as_deref())
            && proposer_fits(page)
            && page.confirmer == "person"
    })
}

/// Personal metadata is persisted alongside the master. Reject invented or
/// unsafe journal contents before they can be displayed or used in recovery.
pub(super) fn validate_personal_metadata(manifest: &Manifest) -> Result<()> {
    let mut layers = BTreeSet::new();
    for record in &manifest.personal_rule_origins {
        ensure!(layers.insert(&record.layer), "Invalid saved rule origins.");
        ensure!(
            canonical_layer(record.layer.clone())? == record.layer,
            "Invalid saved rule origins."
        );
        ensure!(
            record.content_digest.len() == 64
                && record
                    .content_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')),
            "Invalid saved rule origins."
        );
        for origin in record.lines.iter().flatten() {
            ensure!(
                origin.layer == record.layer
                    && safe_origin_value(origin.author.as_deref())
                    && safe_origin_value(origin.proposer.as_deref())
                    && safe_origin_value(origin.confirmer.as_deref())
                    && safe_page_origins(origin),
                "Invalid saved rule origins."
            );
        }
    }
    let mut ids = BTreeSet::new();
    for id in &manifest.personal_rule_history {
        history_path(id)?;
        ensure!(ids.insert(id), "Invalid rule history index.");
    }
    if let Some(journal) = &manifest.personal_rule_journal {
        ensure!(
            !journal.files.is_empty() && journal.files.len() <= 2,
            "Invalid rule history journal."
        );
        for (id, bytes) in &journal.files {
            history_path(id)?;
            ensure!(!ids.contains(id), "Invalid rule history journal.");
            let version: PersonalRuleVersion =
                serde_json::from_slice(bytes).context("Invalid rule history journal")?;
            ensure!(version.summary.id == *id, "Invalid rule history journal.");
            validate_version(&version)?;
        }
        ensure!(journal.archives.len() <= 1, "Invalid rule archive journal.");
        for (id, bytes) in &journal.archives {
            archive_path(id)?;
            let version: PersonalRuleVersion = serde_json::from_slice(
                journal
                    .files
                    .get(id)
                    .context("Missing merge version for rule archive")?,
            )?;
            ensure!(
                version.summary.cause == MERGE_CAUSE,
                "Invalid rule archive journal."
            );
            let pages: BTreeMap<String, Vec<String>> =
                serde_json::from_slice(bytes).context("Invalid rule archive journal")?;
            ensure!(
                pages
                    .iter()
                    .all(|(page, lines)| safe_origin_value(Some(page))
                        && lines.iter().all(|line| !line.contains(['\n', '\r'])
                            && credential_line_numbers(line).is_empty())),
                "Invalid rule archive journal."
            );
        }
    }
    Ok(())
}

fn validate_version(version: &PersonalRuleVersion) -> Result<()> {
    ensure!(
        matches!(
            version.summary.cause.as_str(),
            "baseline" | "change" | "restore" | MERGE_CAUSE | PAGE_CHANGE_CAUSE
        ),
        "Invalid rule history version."
    );
    ensure!(
        safe_origin_value(version.summary.author.as_deref())
            && safe_origin_value(version.summary.proposer.as_deref())
            && safe_origin_value(version.summary.confirmer.as_deref()),
        "Invalid rule history origin."
    );
    let mut summary_layers = BTreeSet::new();
    for layer in &version.summary.layers {
        ensure!(
            summary_layers.insert(layer) && canonical_layer(layer.clone())? == *layer,
            "Invalid rule history layers."
        );
    }
    let mut layers = BTreeSet::new();
    for rule in &version.rules {
        ensure!(layers.insert(&rule.layer), "Invalid rule history version.");
        ensure!(
            canonical_layer(rule.layer.clone())? == rule.layer,
            "Invalid rule history layer."
        );
        personal_rule_content_digest(&rule.layer, &rule.text)?;
        ensure!(
            rule.origins.len() == rule.text.split_inclusive('\n').count(),
            "Invalid rule history origins."
        );
        for origin in rule.origins.iter().flatten() {
            ensure!(
                origin.layer == rule.layer
                    && safe_origin_value(origin.author.as_deref())
                    && safe_origin_value(origin.proposer.as_deref())
                    && safe_origin_value(origin.confirmer.as_deref())
                    && safe_page_origins(origin),
                "Invalid rule history origin."
            );
        }
        ensure!(
            rule.affected_tools
                .iter()
                .all(|tool| personal_target(tool).is_some()),
            "Invalid rule history tools."
        );
    }
    ensure!(
        layers.contains(&PersonalRuleLayer::Shared),
        "Invalid rule history version."
    );
    Ok(())
}

fn snapshot_version(
    manifest: &Manifest,
    summary: PersonalRuleVersionSummary,
) -> Result<PersonalRuleVersion> {
    let mut rules = vec![show_layer(manifest, &PersonalRuleLayer::Shared)?];
    for tool in manifest.master.overlays.keys() {
        rules.push(show_layer(
            manifest,
            &PersonalRuleLayer::Tool { tool: tool.clone() },
        )?);
    }
    Ok(PersonalRuleVersion { summary, rules })
}

fn snapshot_baseline(manifest: &Manifest, id: String) -> Result<PersonalRuleVersion> {
    snapshot_version(
        manifest,
        PersonalRuleVersionSummary {
            id,
            cause: "baseline".into(),
            time: None,
            author: None,
            proposer: None,
            confirmer: None,
            layers: Vec::new(),
        },
    )
}

fn restore_edits(
    manifest: &Manifest,
    version: &PersonalRuleVersion,
) -> Result<Vec<PersonalRuleEdit>> {
    let mut wanted = BTreeMap::new();
    for rule in &version.rules {
        ensure!(
            wanted
                .insert(rule.layer.clone(), rule.text.clone())
                .is_none(),
            "Invalid rule history version."
        );
    }
    ensure!(
        wanted.contains_key(&PersonalRuleLayer::Shared),
        "Invalid rule history version."
    );
    for tool in manifest.master.overlays.keys() {
        wanted
            .entry(PersonalRuleLayer::Tool { tool: tool.clone() })
            .or_default();
    }
    Ok(wanted
        .into_iter()
        .map(|(layer, text)| PersonalRuleEdit { layer, text })
        .collect())
}

pub(super) struct RulePlanUpdate {
    master: PersonalMaster,
    origins: Vec<PersonalRuleLayerOrigins>,
    history: Vec<String>,
    authored: Option<BTreeMap<PathBuf, restore_rules::AuthoredRules>>,
    merge_pages: bool,
}

impl RulePlanUpdate {
    pub(super) fn commit(self, manifest: &mut Manifest) {
        manifest.master = self.master;
        manifest.personal_rule_origins = self.origins;
        manifest.personal_rule_history = self.history;
        manifest.personal_rule_journal = None;
        if self.merge_pages {
            manifest.page_rules = PageRulesState::Merged;
        }
        if let Some(authored) = self.authored {
            manifest.authored_rules = authored;
        }
    }
}

/// Give the existing source-following and tool-share writers the same private
/// history as explicit plans. The caller commits the returned update at its
/// own existing final checkpoint, or discards the journal on rollback.
pub(super) fn stage_master_history(
    setup: &Setup,
    manifest: &mut Manifest,
    next: PersonalMaster,
    person_confirmed: bool,
    allow_extension: bool,
) -> Result<Option<RulePlanUpdate>> {
    stage_master_history_with_origin(
        setup,
        manifest,
        next,
        person_confirmed.into(),
        allow_extension,
    )
}

pub(super) enum MasterChangeOrigin<'a> {
    Person,
    ToolEdit(&'a str),
    Source,
}

impl From<bool> for MasterChangeOrigin<'_> {
    fn from(person_confirmed: bool) -> Self {
        if person_confirmed {
            Self::Person
        } else {
            Self::Source
        }
    }
}

fn stage_master_history_with_origin(
    setup: &Setup,
    manifest: &mut Manifest,
    next: PersonalMaster,
    origin: MasterChangeOrigin<'_>,
    allow_extension: bool,
) -> Result<Option<RulePlanUpdate>> {
    let mut layers = BTreeSet::from([PersonalRuleLayer::Shared]);
    layers.extend(
        manifest
            .master
            .overlays
            .keys()
            .chain(next.overlays.keys())
            .map(|tool| PersonalRuleLayer::Tool { tool: tool.clone() }),
    );
    let mut edits = Vec::new();
    for layer in layers {
        let (before, after) = match &layer {
            PersonalRuleLayer::Shared => (manifest.master.shared.as_str(), next.shared.as_str()),
            PersonalRuleLayer::Tool { tool } => (
                manifest
                    .master
                    .overlays
                    .get(tool)
                    .map_or("", String::as_str),
                next.overlays.get(tool).map_or("", String::as_str),
            ),
        };
        if before != after {
            edits.push(PersonalRuleEdit {
                layer,
                text: after.to_owned(),
            });
        }
    }
    if edits.is_empty() {
        return Ok(None);
    }
    // During adoption, the extension paths must remain recorded on disk until
    // the master and history checkpoint commits. Allow that one known journal
    // while still refusing every other outstanding write.
    if allow_extension {
        let extension = std::mem::take(&mut manifest.extension_paths);
        let check = settled(manifest);
        manifest.extension_paths = extension;
        check?;
    } else {
        settled(manifest)?;
    }
    let changes = preview_personal_rule_edits(&manifest.master, &edits)?;
    let at = chrono::Utc::now().timestamp();
    let confirmer = matches!(origin, MasterChangeOrigin::Person).then(|| "person".to_owned());
    let proposer = match origin {
        MasterChangeOrigin::ToolEdit(tool) => {
            let name = personal_target(tool).map_or(tool, |target| target.tool.display_name());
            Some(format!("{name} hand edit"))
        }
        _ => None,
    };
    let mut origins = manifest.personal_rule_origins.clone();
    for change in &changes {
        let old = origins.iter().find(|record| record.layer == change.layer);
        let origin = PersonalRuleOrigin {
            author: confirmer.clone(),
            proposer: proposer.clone(),
            confirmer: confirmer.clone(),
            layer: change.layer.clone(),
            time: Some(at),
            pages: Vec::new(),
            kept_shared: false,
        };
        let record = confirmed_personal_rule_origins(
            &change.layer,
            &change.before,
            &change.after,
            old,
            &origin,
        )?;
        origins.retain(|old| old.layer != change.layer);
        origins.push(record);
    }
    let mut history = manifest.personal_rule_history.clone();
    let mut versions = Vec::new();
    if history.is_empty() {
        let id = Uuid::new_v4().simple().to_string();
        versions.push(snapshot_baseline(manifest, id.clone())?);
        history.push(id);
    }
    let id = Uuid::new_v4().simple().to_string();
    let old_master = std::mem::replace(&mut manifest.master, next.clone());
    let old_origins = std::mem::replace(&mut manifest.personal_rule_origins, origins.clone());
    let version = snapshot_version(
        manifest,
        PersonalRuleVersionSummary {
            id: id.clone(),
            cause: "change".into(),
            time: Some(at),
            author: confirmer.clone(),
            proposer,
            confirmer,
            layers: changes.iter().map(|change| change.layer.clone()).collect(),
        },
    );
    manifest.master = old_master;
    manifest.personal_rule_origins = old_origins;
    versions.push(version?);
    history.push(id);
    let files = versions
        .iter()
        .map(|version| Ok((version.summary.id.clone(), serde_json::to_vec(version)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let journal = RuleHistoryJournal {
        files,
        archives: BTreeMap::new(),
    };
    manifest.personal_rule_journal = Some(journal.clone());
    setup.save(manifest)?;
    if let Err(error) = write_history_files(setup, &journal) {
        discard_history_journal(setup, manifest)?;
        return Err(error);
    }
    Ok(Some(RulePlanUpdate {
        master: next,
        origins,
        history,
        authored: None,
        merge_pages: false,
    }))
}

fn save_rule_checkpoint(setup: &Setup, manifest: &Manifest) -> Result<()> {
    #[cfg(test)]
    share_rules::before_rule_final_checkpoint()?;
    setup.save(manifest)?;
    #[cfg(test)]
    share_rules::after_rule_final_checkpoint()?;
    Ok(())
}

fn recover_unconfirmed_checkpoint(setup: &Setup) -> Result<()> {
    let mut durable = setup
        .load()
        .context("Could not check the rule change after a write failure. Check rule history before retrying")?
        .context("The rule state is missing after a write failure. Check rule history before retrying")?;
    ensure!(
        durable.rule_share.is_some() || durable.personal_rule_journal.is_some(),
        "The final rule checkpoint could not be confirmed. The change may have been applied; check rule history before retrying."
    );
    let root = setup.home_files()?;
    setup.recover_pending(&root, &mut durable)
}

pub(super) fn checkpoint_master_change(
    setup: &Setup,
    manifest: &mut Manifest,
    before: Manifest,
    person_confirmed: bool,
) -> Result<()> {
    checkpoint_master_change_with_origin(setup, manifest, before, person_confirmed.into())
}

pub(super) fn checkpoint_master_change_with_origin(
    setup: &Setup,
    manifest: &mut Manifest,
    before: Manifest,
    origin: MasterChangeOrigin<'_>,
) -> Result<()> {
    let mut next = std::mem::replace(manifest, before);
    let allow_extension = !manifest.extension_paths.is_empty();
    if let Some(update) = stage_master_history_with_origin(
        setup,
        manifest,
        next.master.clone(),
        origin,
        allow_extension,
    )? {
        update.commit(&mut next);
    }
    *manifest = next;
    if let Err(error) = save_rule_checkpoint(setup, manifest) {
        recover_unconfirmed_checkpoint(setup)?;
        return Err(error.context("The rule change was not applied"));
    }
    Ok(())
}

fn next_origins(
    manifest: &Manifest,
    held: &HeldPlan,
    at: i64,
    page_origins: Option<Vec<Option<PersonalRuleOrigin>>>,
) -> Result<Vec<PersonalRuleLayerOrigins>> {
    let mut records = manifest.personal_rule_origins.clone();
    if let Some(mut lines) = page_origins {
        let change = &held.response.changes[0];
        for origin in lines.iter_mut().flatten() {
            if origin.time.is_none() && !origin.kept_shared && !origin.pages.is_empty() {
                origin.time = Some(at);
                if held.pages.as_ref().is_some_and(|pages| !pages.is_merge()) {
                    origin.author = Some("person".into());
                }
            }
        }
        records.retain(|record| record.layer != PersonalRuleLayer::Shared);
        records.push(PersonalRuleLayerOrigins {
            layer: PersonalRuleLayer::Shared,
            content_digest: personal_rule_content_digest(
                &PersonalRuleLayer::Shared,
                &change.after,
            )?,
            lines,
        });
        return Ok(records);
    }
    for change in &held.response.changes {
        let old = records.iter().find(|record| record.layer == change.layer);
        let confirmed = PersonalRuleOrigin {
            author: Some("person".into()),
            proposer: held.response.proposer.clone(),
            confirmer: Some("person".into()),
            layer: change.layer.clone(),
            time: Some(at),
            pages: Vec::new(),
            kept_shared: false,
        };
        let restored = held.restore.as_ref().and_then(|version| {
            version.rules.iter().find(|rule| {
                rule.layer == change.layer
                    && rule.text == change.after
                    && rule.origins.len() == rule.text.split_inclusive('\n').count()
                    && rule
                        .origins
                        .iter()
                        .flatten()
                        .all(|origin| origin.layer == rule.layer)
            })
        });
        let record = if let Some(restored) = restored {
            PersonalRuleLayerOrigins {
                layer: change.layer.clone(),
                content_digest: personal_rule_content_digest(&change.layer, &change.after)?,
                lines: restored.origins.clone(),
            }
        } else {
            confirmed_personal_rule_origins(
                &change.layer,
                &change.before,
                &change.after,
                old,
                &confirmed,
            )?
        };
        records.retain(|old| old.layer != change.layer);
        records.push(record);
    }
    Ok(records)
}

impl Setup {
    #[cfg(test)]
    fn apply_rule_plan(
        &self,
        id: &str,
        generated: &[String],
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<PersonalRuleVersion> {
        self.apply_plan(id, generated, None, render)
    }

    fn apply_plan(
        &self,
        id: &str,
        generated: &[String],
        current_merge: Option<PageRuleSnapshot>,
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<PersonalRuleVersion> {
        let _lock = self.lock()?;
        // A plan is consumed even when it is stale or a write is refused.
        let mut held = find_plan(self, id, true)?;
        if held.pages.as_ref().is_some_and(PagePlan::is_merge) {
            held.pages = Some(PagePlan::Merge(
                current_merge.context("Preview the page rules again.")?,
            ));
        }
        self.commit_rule_plan(&held, generated, render)
    }

    fn commit_rule_plan(
        &self,
        held: &HeldPlan,
        generated: &[String],
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<PersonalRuleVersion> {
        let root = self.home_files()?;
        let mut manifest = self.load_if_present()?.unwrap_or_else(empty_manifest);
        settled(&manifest)?;
        if let Some(version) = &held.restore {
            let current = read_version(self, &manifest, &version.summary.id)?;
            ensure!(
                serde_json::to_vec(&current)? == serde_json::to_vec(version)?,
                "The history version changed after the preview. Propose the restore again."
            );
        }
        let prepared = self.prepare_plan(
            &manifest,
            PlanEdits {
                edits: &held.edits,
                pages: held.pages.as_ref(),
                restore: held.restore.as_ref(),
            },
            generated,
            held.response.proposer.clone(),
            render,
        )?;
        ensure!(
            prepared.signature == held.signature,
            "The rules or tool files changed after the preview. Propose the change again."
        );
        ensure!(
            prepared.plan.blocked_tools.is_empty(),
            "A tool needs review before these rules can be changed. Propose the change again after resolving it."
        );
        let at = chrono::Utc::now().timestamp();
        let origins = next_origins(&manifest, held, at, prepared.page_origins)?;
        let mut history = manifest.personal_rule_history.clone();
        let mut versions = Vec::new();
        if history.is_empty() {
            let id = Uuid::new_v4().simple().to_string();
            let baseline = snapshot_baseline(&manifest, id.clone())?;
            history.push(id);
            versions.push(baseline);
        }
        let next_id = Uuid::new_v4().simple().to_string();
        let old_master = std::mem::replace(&mut manifest.master, prepared.next_master.clone());
        let old_origins = std::mem::replace(&mut manifest.personal_rule_origins, origins.clone());
        let version = snapshot_version(
            &manifest,
            PersonalRuleVersionSummary {
                id: next_id.clone(),
                cause: if held.pages.as_ref().is_some_and(PagePlan::is_merge) {
                    MERGE_CAUSE
                } else if held.pages.is_some() {
                    PAGE_CHANGE_CAUSE
                } else if held.restore.is_some() {
                    "restore"
                } else {
                    "change"
                }
                .into(),
                time: Some(at),
                author: (!held.pages.as_ref().is_some_and(PagePlan::is_merge))
                    .then(|| "person".into()),
                proposer: held.response.proposer.clone(),
                confirmer: Some("person".into()),
                layers: held
                    .response
                    .changes
                    .iter()
                    .map(|change| change.layer.clone())
                    .collect(),
            },
        );
        manifest.master = old_master;
        manifest.personal_rule_origins = old_origins;
        let version = version?;
        history.push(next_id.clone());
        versions.push(version.clone());
        let files = versions
            .iter()
            .map(|version| Ok((version.summary.id.clone(), serde_json::to_vec(version)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let archives = match held.pages.as_ref().and_then(PagePlan::archive) {
            Some(archive) => BTreeMap::from([(next_id, serde_json::to_vec(&archive)?)]),
            None => BTreeMap::new(),
        };
        let journal = RuleHistoryJournal { files, archives };
        manifest.personal_rule_journal = Some(journal.clone());
        self.save(&manifest)?;
        if let Err(error) = write_history_files(self, &journal) {
            discard_history_journal(self, &mut manifest)?;
            return Err(error);
        }
        let update = RulePlanUpdate {
            master: prepared.next_master,
            origins,
            history,
            authored: Some(prepared.next_authored),
            merge_pages: held.pages.as_ref().is_some_and(PagePlan::is_merge),
        };
        if prepared.operations.is_empty() {
            update.commit(&mut manifest);
            if let Err(error) = save_rule_checkpoint(self, &manifest) {
                recover_unconfirmed_checkpoint(self)?;
                return Err(error.context("The rule change was not applied"));
            }
            return Ok(version);
        }
        let expected = prepared.operations.len();
        let result = self.apply_batch(
            &root,
            &mut manifest,
            prepared.operations,
            BatchMode::PersonalRules(update),
        );
        let succeeded = result.as_ref().is_ok_and(|outcomes| {
            outcomes.len() == expected
                && outcomes
                    .iter()
                    .all(|(_, outcome)| *outcome == ApplyOutcome::Applied)
        });
        if !succeeded {
            if result.is_err() {
                // The final checkpoint can fail after the in-memory journal
                // was cleared. Only the durable manifest can say whether the
                // batch is still recoverable or the change may have landed.
                recover_unconfirmed_checkpoint(self)?;
            } else {
                self.recover_pending(&root, &mut manifest)?;
                discard_history_journal(self, &mut manifest)?;
            }
            return match result {
                Ok(_) => Err(anyhow::anyhow!(
                    "A tool file changed. Propose the change again."
                )),
                Err(error) => Err(error.context("The rule change was not applied")),
            };
        }
        Ok(version)
    }
}
