// SPDX-License-Identifier: Apache-2.0
//! Preview and commit personal shared and per-tool rule changes.

#[cfg(test)]
mod tests;

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Path as RoutePath;
use serde::Serialize;
use uuid::Uuid;
use wenlan_core::rulesync_rule_plan::{
    confirmed_personal_rule_origins, personal_rule_content_digest, preview_personal_rule_edits,
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

struct HeldPlan {
    home: PathBuf,
    data: PathBuf,
    created: Instant,
    response: PersonalRulePlan,
    edits: Vec<PersonalRuleEdit>,
    signature: String,
    restore: Option<PersonalRuleVersion>,
}

struct Prepared {
    plan: PersonalRulePlan,
    operations: Vec<Pending>,
    next_master: PersonalMaster,
    next_authored: BTreeMap<PathBuf, restore_rules::AuthoredRules>,
    signature: String,
    expected_source: Option<String>,
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
    following_source: bool,
    source_base: &'a Option<String>,
    source_read: &'a str,
    source_credential_lines: &'a [usize],
    generated: &'a [String],
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
    fn prepare_rule_plan(
        &self,
        manifest: &Manifest,
        edits: &[PersonalRuleEdit],
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
        let mut changes = preview_personal_rule_edits(&current_master, edits)?;
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

        // A plan uses the source's last reconciled state. Never silently pull
        // a source edit into a different change after the person saw a diff.
        let source = shared_source::SharedSource::read(&self.home, manifest)?;
        if let Some(line) = source.credential_lines.first() {
            return Err(SharedSourceCredential(*line).into());
        }
        ensure!(
            source.merged == manifest.master.shared,
            "Shared source changes are waiting. Sync or resolve them before changing rules."
        );

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
            let source_text = next_master.rule_text(target, generated).map_err(|_| {
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
                kind: Some(Kind::rule(&next_master, target, generated, source_text)),
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
            following_source: manifest.shared_source_following,
            source_base: &manifest.shared_source_base,
            source_read: &source.read,
            source_credential_lines: &source.credential_lines,
            generated,
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
                following_source: manifest.shared_source_following,
                changes,
                blocked_tools: blocked,
                unmanaged_tools: unmanaged,
            },
            operations,
            next_master,
            next_authored,
            signature,
            expected_source: manifest.shared_source_following.then_some(source.read),
        })
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

#[derive(Debug)]
struct SharedSourceCredential(usize);

impl std::fmt::Display for SharedSourceCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The shared source has a possible credential on line {}. Resolve it before changing rules.", self.0)
    }
}

impl std::error::Error for SharedSourceCredential {}

fn server_error(error: anyhow::Error) -> ServerError {
    if let Some(SharedSourceCredential(line)) = error.downcast_ref::<SharedSourceCredential>() {
        return ServerError::Conflict(format!(
            "personal_rule_credential: layer=shared-source line={line}"
        ));
    }
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
    let generated = crate::skill_grow::general_tool_rules(&state).await?;
    tokio::task::spawn_blocking(move || {
        let setup = rendering_setup()?;
        let layers = rulesync_team::load_layers(&wenlan_core::shared_space::settings_path())?;
        let session = RenderSession::new(&setup.binary);
        setup.apply_rule_plan(&request.plan_id, &generated, &|tool, text| {
            setup.render_member_rule(&session, tool, text, &layers)
        })
    })
    .await
    .map_err(|error| ServerError::Internal(error.to_string()))?
    .map(Json)
    .map_err(server_error)
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
) -> PersonalRulePlan {
    let id = Uuid::new_v4();
    prepared.plan.id = id.simple().to_string();
    prepared.plan.expires_at = chrono::Utc::now().timestamp() + PLAN_LIFETIME.as_secs() as i64;
    let response = prepared.plan;
    let mut plans = PLANS.lock().unwrap_or_else(|error| error.into_inner());
    plans.retain(|_, plan| plan.created.elapsed() < PLAN_LIFETIME);
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
        return Ok(HeldPlan {
            home: held.home.clone(),
            data: held.data.clone(),
            created: held.created,
            response: held.response.clone(),
            edits: held.edits.clone(),
            signature: held.signature.clone(),
            restore: held.restore.clone(),
        });
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
    let prepared =
        setup.prepare_rule_plan(&manifest, &edits, generated, proposer, &|tool, text| {
            setup.render_member_rule(&session, tool, text, &layers)
        })?;
    Ok(remember_plan(setup, prepared, edits, restore))
}

/// History files are written before the manifest checkpoint. This intent is
/// persisted first so recovery can remove only this uncommitted version's
/// exact files if a crash or conflict interrupts the operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleHistoryJournal {
    files: BTreeMap<String, Vec<u8>>,
}

fn history_path(id: &str) -> Result<PathBuf> {
    let parsed = Uuid::parse_str(id).context("Invalid rule version.")?;
    ensure!(parsed.simple().to_string() == id, "Invalid rule version.");
    Ok(Path::new(HISTORY_DIRECTORY).join(format!("v-{id}.json")))
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
    for (id, bytes) in &journal.files {
        let path = history_path(id)?;
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
    root.sync_directory(directory)?;
    root.sync_directory(Path::new(""))?;
    Ok(())
}

pub(super) fn discard_history_journal(setup: &Setup, manifest: &mut Manifest) -> Result<()> {
    let Some(journal) = &manifest.personal_rule_journal else {
        return Ok(());
    };
    let root = setup.data_files()?.with_private_directories();
    for (id, bytes) in &journal.files {
        let path = history_path(id)?;
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
    root.sync_directory(Path::new(HISTORY_DIRECTORY))?;
    manifest.personal_rule_journal = None;
    setup.save(manifest)
}

fn read_version(setup: &Setup, manifest: &Manifest, id: &str) -> Result<PersonalRuleVersion> {
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
                    && safe_origin_value(origin.confirmer.as_deref()),
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
    }
    Ok(())
}

fn validate_version(version: &PersonalRuleVersion) -> Result<()> {
    ensure!(
        matches!(
            version.summary.cause.as_str(),
            "baseline" | "change" | "restore"
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
                    && safe_origin_value(origin.confirmer.as_deref()),
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

/// Stage the first known snapshot without inventing an earlier version.
pub(super) fn stage_baseline(setup: &Setup, manifest: &mut Manifest) -> Result<Option<String>> {
    if !manifest.personal_rule_history.is_empty() {
        return Ok(None);
    }
    settled(manifest)?;
    let id = Uuid::new_v4().simple().to_string();
    let version = snapshot_baseline(manifest, id.clone())?;
    let journal = RuleHistoryJournal {
        files: BTreeMap::from([(id.clone(), serde_json::to_vec(&version)?)]),
    };
    manifest.personal_rule_journal = Some(journal.clone());
    setup.save(manifest)?;
    if let Err(error) = write_history_files(setup, &journal) {
        discard_history_journal(setup, manifest)?;
        return Err(error);
    }
    Ok(Some(id))
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
    expected_source: Option<String>,
    authored: Option<BTreeMap<PathBuf, restore_rules::AuthoredRules>>,
}

impl RulePlanUpdate {
    pub(super) fn check_followed_source(&self, setup: &Setup, manifest: &Manifest) -> Result<()> {
        if let Some(expected) = &self.expected_source {
            let unchanged = shared_source::SharedSource::read(&setup.home, manifest)
                .is_ok_and(|source| source.credential_lines.is_empty() && source.read == *expected);
            ensure!(unchanged, "The shared source changed during delivery or could not be checked. Propose the change again.");
        }
        Ok(())
    }

    pub(super) fn commit(self, manifest: &mut Manifest) {
        manifest.master = self.master;
        manifest.personal_rule_origins = self.origins;
        manifest.personal_rule_history = self.history;
        manifest.personal_rule_journal = None;
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
    let confirmer = person_confirmed.then(|| "person".to_owned());
    let mut origins = manifest.personal_rule_origins.clone();
    for change in &changes {
        let old = origins.iter().find(|record| record.layer == change.layer);
        let origin = PersonalRuleOrigin {
            author: confirmer.clone(),
            proposer: None,
            confirmer: confirmer.clone(),
            layer: change.layer.clone(),
            time: Some(at),
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
            proposer: None,
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
    let journal = RuleHistoryJournal { files };
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
        expected_source: None,
        authored: None,
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
    let mut next = std::mem::replace(manifest, before);
    let allow_extension = !manifest.extension_paths.is_empty();
    if let Some(update) = stage_master_history(
        setup,
        manifest,
        next.master.clone(),
        person_confirmed,
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
) -> Result<Vec<PersonalRuleLayerOrigins>> {
    let mut records = manifest.personal_rule_origins.clone();
    for change in &held.response.changes {
        let old = records.iter().find(|record| record.layer == change.layer);
        let confirmed = PersonalRuleOrigin {
            author: Some("person".into()),
            proposer: held.response.proposer.clone(),
            confirmer: Some("person".into()),
            layer: change.layer.clone(),
            time: Some(at),
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
    fn apply_rule_plan(
        &self,
        id: &str,
        generated: &[String],
        render: &impl Fn(&str, &str) -> Result<SkillFiles>,
    ) -> Result<PersonalRuleVersion> {
        let _lock = self.lock()?;
        // A plan is consumed even when it is stale or a write is refused.
        let held = find_plan(self, id, true)?;
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
        let prepared = self.prepare_rule_plan(
            &manifest,
            &held.edits,
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
        let origins = next_origins(&manifest, &held, at)?;
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
                cause: if held.restore.is_some() {
                    "restore"
                } else {
                    "change"
                }
                .into(),
                time: Some(at),
                author: Some("person".into()),
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
        history.push(next_id);
        versions.push(version.clone());
        let files = versions
            .iter()
            .map(|version| Ok((version.summary.id.clone(), serde_json::to_vec(version)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let journal = RuleHistoryJournal { files };
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
            expected_source: prepared.expected_source,
            authored: Some(prepared.next_authored),
        };
        if prepared.operations.is_empty() {
            if let Err(error) = update.check_followed_source(self, &manifest) {
                discard_history_journal(self, &mut manifest)?;
                return Err(error);
            }
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
