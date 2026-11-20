//! Private, checkout-specific renders. The repository and its Git index are never the destination.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use wenlan_core::rulesync_personal::{self, fingerprint, write_root_rule, PersonalMaster};
use wenlan_core::rulesync_team::{self, TeamRuleLayer};
use wenlan_core::{repo_wiki, rulesync_renderer, shared_space};
use wenlan_types::rulesync::{CheckoutRulesReceipt, CheckoutRulesRequest, RulesSyncReport};

use crate::error::ServerError;
use crate::route_registry::{post, TrackedRouter};
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};
use crate::rulesync_layers;
use crate::rulesync_routes::{read_stage, report, require_local_person, text, with_bytes, Setup};
use crate::state::SharedState;

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route("/api/rules/setup/checkout", post(checkout))
}

async fn checkout(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<CheckoutRulesRequest>,
) -> Result<Json<CheckoutRulesReceipt>, ServerError> {
    require_local_person(&headers)?;
    if !rulesync_personal::enabled() {
        return Err(ServerError::Conflict("WENLAN_RULESYNC is off".into()));
    }
    let personal = crate::skill_grow::general_tool_rules(&state).await?;
    tokio::task::spawn_blocking(move || Setup::system()?.checkout(&request.repo, &personal))
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map(Json)
        .map_err(|error| ServerError::Conflict(error.to_string()))
}

#[derive(Default, Serialize, Deserialize)]
struct Checkout {
    repo: PathBuf,
    files: BTreeMap<PathBuf, Snapshot>,
}

pub(crate) fn open(
    setup: &Setup,
    repo: &Path,
    master: &PersonalMaster,
    generated: &[String],
) -> Result<CheckoutRulesReceipt> {
    ensure!(repo.is_absolute(), "use an absolute checkout path");
    let repo = repo_wiki::repository_top_level(repo)?;
    let key = fingerprint(&repo)?;
    let state_path = PathBuf::from(format!("checkouts/{key}.json"));
    let root = FileRoot::open(&setup.data)?;
    let state = root.read(&state_path, false)?;
    let checkout = match &state {
        Snapshot::Absent => Checkout {
            repo,
            files: BTreeMap::new(),
        },
        Snapshot::File { bytes, .. } => serde_json::from_slice(bytes)?,
        _ => anyhow::bail!("checkout record must be a regular file"),
    };
    let directory = PathBuf::from(format!("checkouts/{key}/rules"));
    let job = Job {
        setup,
        settings: shared_space::settings_path(),
        root: &root,
        directory: &directory,
    };
    let result = job.run(&state_path, state, checkout, master, generated)?;
    Ok(CheckoutRulesReceipt {
        directory: setup.data.join(directory),
        report: result,
    })
}

pub(crate) fn refresh(
    setup: &Setup,
    settings: &Path,
    master: &PersonalMaster,
    generated: &[String],
) {
    fn refresh_inner(
        setup: &Setup,
        settings: &Path,
        master: &PersonalMaster,
        generated: &[String],
    ) -> Result<()> {
        let entries = match std::fs::read_dir(setup.data.join("checkouts")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let root = FileRoot::open(&setup.data)?;
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let leaf = PathBuf::from(entry.file_name());
            let Some(key) = leaf
                .file_stem()
                .and_then(|key| key.to_str())
                .filter(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
            else {
                continue;
            };
            if leaf.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let state_path = Path::new("checkouts").join(&leaf);
            let state = root.read(&state_path, false)?;
            let Snapshot::File { bytes, .. } = &state else {
                continue;
            };
            let checkout: Checkout = serde_json::from_slice(bytes)?;
            let directory = PathBuf::from(format!("checkouts/{key}/rules"));
            let job = Job {
                setup,
                settings: settings.to_path_buf(),
                root: &root,
                directory: &directory,
            };
            if let Err(error) = job.run(&state_path, state, checkout, master, generated) {
                tracing::warn!(%error, "[rules setup] checkout rules refresh failed");
            }
        }
        Ok(())
    }
    if let Err(error) = refresh_inner(setup, settings, master, generated) {
        tracing::warn!(%error, "[rules setup] checkout receipts could not be read");
    }
}

struct Job<'a> {
    setup: &'a Setup,
    settings: PathBuf,
    root: &'a FileRoot,
    directory: &'a Path,
}

impl Job<'_> {
    fn run(
        &self,
        state_path: &Path,
        state: Snapshot,
        mut checkout: Checkout,
        master: &PersonalMaster,
        generated: &[String],
    ) -> Result<RulesSyncReport> {
        let repo_id = repo_wiki::origin_url(&checkout.repo)
            .as_deref()
            .and_then(rulesync_team::repo_identity);
        let project = match repo_id {
            Some(id) => rulesync_team::load_project_layers(&self.settings)?
                .into_iter()
                .filter(|layer| {
                    layer
                        .project
                        .as_ref()
                        .and_then(|project| project.repo_id.as_ref())
                        == Some(&id)
                })
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        let teams = rulesync_team::load_layers(&self.settings)?;
        let authorized: BTreeMap<_, _> = project
            .iter()
            .chain(&teams)
            .map(|layer| Ok((rulesync_layers::key(layer)?, layer)))
            .collect::<Result<_>>()?;
        let keys: BTreeSet<_> = authorized.keys().cloned().collect();
        let mut result = report("Checked the rules for this checkout");
        let mut blocked = BTreeSet::new();
        // Remove a refused layer from the last delivery even if a later render fails.
        for (path, delivered) in &mut checkout.files {
            let current = self.root.read(&self.directory.join(path), false)?;
            let before = text(delivered)?;
            let remaining = rulesync_layers::remove_inaccessible(before, &keys)?;
            if remaining != before {
                let after = text(&current)
                    .and_then(|text| rulesync_layers::remove_inaccessible(text, &keys));
                let after = after.unwrap_or_else(|_| remaining.clone());
                let desired = with_bytes(&current, after.into_bytes());
                if self.replace(path, &current, &desired)? {
                    *delivered = with_bytes(delivered, remaining.into_bytes());
                } else {
                    blocked.insert(path.clone());
                }
            }
        }
        let mut record = self.save(state_path, &state, &checkout)?;
        let mut observed = BTreeMap::new();
        for (path, delivered) in &checkout.files {
            let current = self.root.read(&self.directory.join(path), false)?;
            if current != *delivered {
                let before = text(delivered)?;
                let after = text(&current)?;
                match rulesync_layers::edits(before, after) {
                    Ok(edits)
                        if !edits.is_empty()
                            && rulesync_layers::only_personal(before)?
                                == rulesync_layers::only_personal(after)? =>
                    {
                        for (key, edit) in edits {
                            let layer = authorized
                                .get(key)
                                .context("shared layer no longer accessible")?;
                            rulesync_team::queue_proposal(
                                &self.settings,
                                &layer.id,
                                edit.revision,
                                &rulesync_layers::proposed_markdown(layer, &edit)?,
                            )?;
                        }
                    }
                    _ => {
                        blocked.insert(path.clone());
                    }
                }
            }
            observed.insert(path.clone(), current);
        }
        // Capture exact bytes before invoking the renderer, which may be slow.
        let rendered = match self.render(&project, &teams, master, generated) {
            Ok(files) => files,
            Err(error) => {
                result.blocked.push(error.to_string());
                return Ok(result);
            }
        };
        for (path, bytes) in rendered {
            let current = observed.get(&path).unwrap_or(&Snapshot::Absent);
            if blocked.contains(&path) {
                continue;
            }
            let found = rulesync_layers::blocks(std::str::from_utf8(&bytes)?)?;
            ensure!(
                found
                    .keys()
                    .map(|key| (*key).to_owned())
                    .collect::<BTreeSet<_>>()
                    == keys,
                "rulesync did not preserve the authorized checkout layers"
            );
            let desired = with_bytes(current, bytes);
            if self.replace(&path, current, &desired)? {
                if *current != desired {
                    result.written.push(path.display().to_string());
                }
                checkout.files.insert(path, desired);
                record = self.save(state_path, &record, &checkout)?;
            } else {
                blocked.insert(path);
            }
        }
        result
            .blocked
            .extend(blocked.iter().map(|path| path.display().to_string()));
        Ok(result)
    }

    fn save(&self, path: &Path, seen: &Snapshot, checkout: &Checkout) -> Result<Snapshot> {
        let desired = with_bytes(seen, serde_json::to_vec(checkout)?);
        ensure!(
            self.root.apply(path, seen, &desired, false)? == ApplyOutcome::Applied,
            "checkout receipt changed during sync"
        );
        Ok(desired)
    }

    fn replace(&self, path: &Path, seen: &Snapshot, desired: &Snapshot) -> Result<bool> {
        Ok(self
            .root
            .apply(&self.directory.join(path), seen, desired, false)?
            == ApplyOutcome::Applied)
    }

    fn render(
        &self,
        project: &[TeamRuleLayer],
        teams: &[TeamRuleLayer],
        master: &PersonalMaster,
        generated: &[String],
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
        let mut files = BTreeMap::new();
        for target in rulesync_personal::adoption_inputs() {
            let stage = rulesync_renderer::new_stage(&self.setup.data)?;
            let output = stage.path().join("output");
            let mut roots = Vec::new();
            for (prefix, layers) in [("Project", project), ("Team", teams)] {
                if !layers.is_empty() {
                    let input = stage.path().join(prefix.to_ascii_lowercase());
                    rulesync_layers::stage(&input, prefix, layers)?;
                    roots.push(input);
                }
            }
            let personal = stage.path().join("personal");
            write_root_rule(&personal, &master.rule_text(&target.tool, generated)?)?;
            roots.push(personal);
            std::fs::create_dir(&output)?;
            rulesync_renderer::render(
                &self.setup.binary,
                stage.path(),
                &roots,
                &output,
                false,
                &[target.tool],
                &["rules"],
            )?;
            read_stage(&output, &output, &mut files)?;
        }
        Ok(files)
    }
}
