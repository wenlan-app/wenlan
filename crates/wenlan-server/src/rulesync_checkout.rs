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
use crate::rulesync_routes::{
    read_stage, rendered_skill_root, report, require_local_person, text, with_bytes,
    write_skill_files, Setup,
};
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
    #[serde(default)]
    skills: BTreeMap<PathBuf, SkillDelivery>,
    #[serde(default)]
    pending: Option<PendingDelivery>,
}

#[derive(Serialize, Deserialize)]
struct SkillDelivery {
    source: String,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct PendingDelivery {
    path: PathBuf,
    digest: String,
    skill_source: Option<String>,
}

impl Checkout {
    fn settle(&mut self, pending: PendingDelivery, snapshot: Snapshot) {
        if let Some(source) = pending.skill_source {
            self.skills.insert(
                pending.path,
                SkillDelivery {
                    source,
                    digest: pending.digest,
                },
            );
        } else {
            self.files.insert(pending.path, snapshot);
        }
    }
}

#[derive(Default)]
struct RenderedCheckout {
    rules: BTreeMap<PathBuf, Vec<u8>>,
    skills: BTreeMap<PathBuf, ProjectSkillFile>,
}

struct ProjectSkillFile {
    source: String,
    bytes: Vec<u8>,
}

// A checkout can match several readable Spaces. A colliding skill has no
// unambiguous source, so neither copy participates in the render.
#[derive(Default)]
struct ProjectSkills {
    packages: BTreeMap<String, BTreeMap<PathBuf, ProjectSkillFile>>,
    conflicts: BTreeSet<String>,
}

impl ProjectSkills {
    fn from_layers(layers: &[TeamRuleLayer]) -> Result<Self> {
        let mut result = Self::default();
        for layer in layers {
            for skill in &layer.skills {
                if result.conflicts.contains(&skill.name) {
                    continue;
                }
                if result.packages.remove(&skill.name).is_some() {
                    result.conflicts.insert(skill.name.clone());
                    continue;
                }
                let files = skill
                    .files
                    .iter()
                    .map(|file| {
                        let source = fingerprint(&(&layer.id, &skill.name, &file.path))?;
                        Ok((
                            PathBuf::from(&file.path),
                            ProjectSkillFile {
                                source,
                                bytes: file.text.as_bytes().to_vec(),
                            },
                        ))
                    })
                    .collect::<Result<_>>()?;
                result.packages.insert(skill.name.clone(), files);
            }
        }
        Ok(result)
    }

    fn stage(&self, input: &Path) -> Result<()> {
        for (name, files) in &self.packages {
            let files = files
                .iter()
                .map(|(path, file)| (path.clone(), file.bytes.clone()))
                .collect();
            write_skill_files(input, name, &files)?;
        }
        Ok(())
    }

    fn take_rendered(
        &self,
        output: &mut BTreeMap<PathBuf, Vec<u8>>,
        rendered: &mut RenderedCheckout,
    ) -> Result<()> {
        for (name, files) in &self.packages {
            let root = rendered_skill_root(output, name)?
                .with_context(|| format!("rulesync did not render project skill {name}"))?
                .to_path_buf();
            for (relative, file) in files {
                let path = root.join(relative);
                let bytes = output.remove(&path).with_context(|| {
                    format!("rulesync omitted {} from {name}", relative.display())
                })?;
                ensure!(
                    rendered
                        .skills
                        .insert(
                            path,
                            ProjectSkillFile {
                                source: file.source.clone(),
                                bytes
                            }
                        )
                        .is_none(),
                    "rulesync produced the same skill path for two tools"
                );
            }
            ensure!(
                !output.keys().any(|path| path.starts_with(&root)),
                "rulesync returned unexpected files for project skill {name}"
            );
        }
        Ok(())
    }
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
            ..Checkout::default()
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
    let result = job.run(&state_path, state, checkout, Some(master), generated)?;
    Ok(CheckoutRulesReceipt {
        directory: setup.data.join(directory),
        report: result,
    })
}

pub(crate) fn refresh(
    setup: &Setup,
    settings: &Path,
    master: Option<&PersonalMaster>,
    generated: &[String],
) {
    fn refresh_inner(
        setup: &Setup,
        settings: &Path,
        master: Option<&PersonalMaster>,
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
            match job.run(&state_path, state, checkout, master, generated) {
                Err(error) => tracing::warn!(%error, "[rules setup] checkout rules refresh failed"),
                Ok(report) if !report.blocked.is_empty() => tracing::warn!(
                    files = report.blocked.len(),
                    "[rules setup] some checkout deliveries need attention"
                ),
                Ok(_) => {}
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

fn report_blocks(result: &mut RulesSyncReport, blocked: BTreeMap<PathBuf, &'static str>) {
    result.blocked.extend(
        blocked
            .into_iter()
            .map(|(path, reason)| format!("{}: {reason}", path.display())),
    );
}

impl Job<'_> {
    fn run(
        &self,
        state_path: &Path,
        state: Snapshot,
        mut checkout: Checkout,
        master: Option<&PersonalMaster>,
        generated: &[String],
    ) -> Result<RulesSyncReport> {
        let repo_id = master
            .and_then(|_| repo_wiki::origin_url(&checkout.repo))
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
        let teams = match master {
            Some(_) => rulesync_team::load_layers(&self.settings)?,
            None => Vec::new(),
        };
        let skills = ProjectSkills::from_layers(&project)?;
        let authorized: BTreeMap<_, _> = project
            .iter()
            .chain(&teams)
            .map(|layer| Ok((rulesync_layers::key(layer)?, layer)))
            .collect::<Result<_>>()?;
        let keys: BTreeSet<_> = authorized.keys().cloned().collect();
        let mut result = report("Checked the rules for this checkout");
        result.blocked.extend(
            skills
                .conflicts
                .iter()
                .map(|name| format!("project skill {name} is approved in more than one Space")),
        );
        let mut blocked = BTreeMap::new();
        if let Some(pending) = checkout.pending.take() {
            let current = self.root.read(&self.directory.join(&pending.path), false)?;
            if fingerprint(&current)? == pending.digest {
                checkout.settle(pending, current);
            }
        }
        let skill_sources: BTreeSet<_> = skills
            .packages
            .values()
            .flat_map(|files| files.values().map(|file| file.source.as_str()))
            .collect();
        let mut observed_skills = self.prune_skills(&mut checkout, &skill_sources, &mut blocked)?;
        // Remove a refused layer from the last delivery even if a later render fails.
        for (path, delivered) in &mut checkout.files {
            let current = self.root.read(&self.directory.join(path), false)?;
            let before = text(delivered)?;
            let remaining = rulesync_layers::remove_inaccessible(before, &keys)?;
            if remaining != before {
                if current == Snapshot::Absent {
                    *delivered = current;
                    continue;
                }
                let after = text(&current)
                    .and_then(|text| rulesync_layers::remove_inaccessible(text, &keys));
                let after = after.unwrap_or_else(|_| remaining.clone());
                let desired = with_bytes(&current, after.into_bytes());
                if self.replace(path, &current, &desired)? {
                    *delivered = with_bytes(delivered, remaining.into_bytes());
                } else {
                    blocked.insert(path.clone(), "Changed while Wenlan was removing a layer. Sync again to finish removing it.");
                }
            }
        }
        let mut record = self.save(state_path, &state, &checkout)?;
        let Some(master) = master else {
            report_blocks(&mut result, blocked);
            return Ok(result);
        };
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
                        blocked.insert(path.clone(), "The checkout rules or their section markers changed. Reconcile the edits with the Rules page and sync again.");
                    }
                }
            }
            observed.insert(path.clone(), current);
        }
        // Capture exact bytes before invoking the renderer, which may be slow.
        let rendered = match self.render(&project, &teams, &skills, master, generated) {
            Ok(files) => files,
            Err(error) => {
                result.blocked.push(format!("Checkout render: {error:#}"));
                report_blocks(&mut result, blocked);
                return Ok(result);
            }
        };
        for (path, bytes) in rendered.rules {
            let current = observed.get(&path).unwrap_or(&Snapshot::Absent);
            if blocked.contains_key(&path) {
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
            let changed = current != &desired;
            let pending = PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&desired)?,
                skill_source: None,
            };
            if self.deliver(
                state_path,
                &mut record,
                &mut checkout,
                pending,
                current,
                desired,
            )? {
                if changed {
                    result.written.push(path.display().to_string());
                }
            } else {
                blocked.insert(
                    path,
                    "Changed during delivery. Sync again to collect the latest edits.",
                );
            }
        }
        for (path, file) in rendered.skills {
            if blocked.contains_key(&path) {
                continue;
            }
            let current = observed_skills.remove(&path).unwrap_or(Snapshot::Absent);
            let desired = with_bytes(&current, file.bytes);
            let changed = current != desired;
            let pending = PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&desired)?,
                skill_source: Some(file.source),
            };
            if self.deliver(
                state_path,
                &mut record,
                &mut checkout,
                pending,
                &current,
                desired,
            )? {
                if changed {
                    result.written.push(path.display().to_string());
                }
            } else {
                blocked.insert(
                    path,
                    "Changed during delivery. Check the skill files and sync again.",
                );
            }
        }
        report_blocks(&mut result, blocked);
        Ok(result)
    }

    fn deliver(
        &self,
        state_path: &Path,
        record: &mut Snapshot,
        checkout: &mut Checkout,
        pending: PendingDelivery,
        current: &Snapshot,
        desired: Snapshot,
    ) -> Result<bool> {
        // Journal the digest before writing a private copy so a restart
        // can still revoke bytes that reached the destination.
        checkout.pending = Some(pending);
        *record = self.save(state_path, record, checkout)?;
        let pending = checkout
            .pending
            .take()
            .context("missing checkout journal")?;
        let applied = self.replace(&pending.path, current, &desired)?;
        if applied {
            checkout.settle(pending, desired);
        }
        *record = self.save(state_path, record, checkout)?;
        Ok(applied)
    }

    fn prune_skills(
        &self,
        checkout: &mut Checkout,
        allowed: &BTreeSet<&str>,
        blocked: &mut BTreeMap<PathBuf, &'static str>,
    ) -> Result<BTreeMap<PathBuf, Snapshot>> {
        let mut retired = Vec::new();
        let mut observed = BTreeMap::new();
        for (path, delivery) in &checkout.skills {
            let current = self.root.read(&self.directory.join(path), false)?;
            if !allowed.contains(delivery.source.as_str()) {
                if self.replace(path, &current, &Snapshot::Absent)? {
                    retired.push(path.clone());
                } else {
                    blocked.insert(path.clone(), "Changed while Wenlan was removing a skill. Sync again to finish removing it.");
                }
            } else if current == Snapshot::Absent || fingerprint(&current)? == delivery.digest {
                observed.insert(path.clone(), current);
            } else {
                blocked.insert(path.clone(), "The installed skill was edited. Reconcile the change with the Space's approved skill and sync again.");
            }
        }
        for path in retired {
            checkout.skills.remove(&path);
        }
        Ok(observed)
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
        skills: &ProjectSkills,
        master: &PersonalMaster,
        generated: &[String],
    ) -> Result<RenderedCheckout> {
        let mut rendered = RenderedCheckout::default();
        let features: &[&str] = if skills.packages.is_empty() {
            &["rules"]
        } else {
            &["rules", "skills"]
        };
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
            skills.stage(&stage.path().join("project"))?;
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
                features,
            )?;
            let mut files = BTreeMap::new();
            read_stage(&output, &output, &mut files)?;
            skills.take_rendered(&mut files, &mut rendered)?;
            rendered.rules.extend(files);
        }
        Ok(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::team::{TeamProjectSkill, TeamProjectSkillFile};

    #[test]
    fn rulesync_checkout_splits_skill_output_and_blocks_colliding_sources() {
        let layer = |id: &str| TeamRuleLayer {
            id: id.to_owned(),
            name: id.to_owned(),
            markdown: String::new(),
            revision: String::new(),
            synced_at: String::new(),
            project: None,
            skills: vec![TeamProjectSkill {
                name: "checked-procedure".into(),
                files: vec![TeamProjectSkillFile {
                    path: "SKILL.md".into(),
                    text: "Expected skill".into(),
                }],
            }],
        };
        let skills = ProjectSkills::from_layers(&[layer("one")]).unwrap();
        let rule = PathBuf::from("AGENTS.md");
        let skill = PathBuf::from(".agents/skills/checked-procedure/SKILL.md");
        let mut stage = BTreeMap::from([
            (rule.clone(), b"rules".to_vec()),
            (skill.clone(), b"a rendered skill".to_vec()),
        ]);
        let mut rendered = RenderedCheckout::default();
        skills.take_rendered(&mut stage, &mut rendered).unwrap();
        assert!(stage.contains_key(&rule));
        assert_eq!(rendered.skills[&skill].bytes, b"a rendered skill");

        let skills = ProjectSkills::from_layers(&[layer("one"), layer("two")]).unwrap();
        assert!(skills.packages.is_empty());
        assert!(skills.conflicts.contains("checked-procedure"));
    }

    #[test]
    fn rulesync_checkout_recovers_then_prunes_revoked_skill_before_a_render_failure() {
        let scratch = tempfile::tempdir().unwrap();
        let data = scratch.path().join("data");
        let source = scratch.path().join("repo");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir(&source).unwrap();
        let setup = Setup {
            home: scratch.path().join("home"),
            data,
            binary: scratch.path().join("unavailable-renderer"),
        };
        let root = FileRoot::open(&setup.data).unwrap();
        let job = Job {
            setup: &setup,
            settings: scratch.path().join("settings.toml"),
            root: &root,
            directory: Path::new("checkouts/example/rules"),
        };
        let path = PathBuf::from(".agents/skills/project-only/SKILL.md");
        let approved = Snapshot::file(b"approved project-only skill".to_vec());
        assert!(job.replace(&path, &Snapshot::Absent, &approved).unwrap());
        let checkout = Checkout {
            repo: source,
            pending: Some(PendingDelivery {
                path: path.clone(),
                digest: fingerprint(&approved).unwrap(),
                skill_source: Some("lost-access".to_owned()),
            }),
            ..Checkout::default()
        };
        let state_path = Path::new("checkouts/example.json");
        let state = job.save(state_path, &Snapshot::Absent, &checkout).unwrap();
        std::fs::create_dir(&setup.home).unwrap();
        let master =
            PersonalMaster::new(&rulesync_personal::discover_adoption(&setup.home).unwrap());
        let report = job
            .run(state_path, state, checkout, Some(&master), &[])
            .unwrap();
        assert!(!report.blocked.is_empty());
        assert_eq!(
            root.read(&job.directory.join(path), false).unwrap(),
            Snapshot::Absent
        );
        let record = root.read(state_path, false).unwrap();
        let saved: Checkout = serde_json::from_str(text(&record).unwrap()).unwrap();
        assert!(saved.skills.is_empty());
        assert!(saved.pending.is_none());
    }
}
