// SPDX-License-Identifier: Apache-2.0
//! Personal commands and subagents share the setup journal and review flow.

use super::*;
use wenlan_core::rulesync_personal::describe_personal_file_change;
use wenlan_core::rulesync_personal_files::{
    alternate_paths, location_of, sharing_paths, PersonalFileLocation, MAX_FILE_BYTES,
};

pub(super) fn known_paths(manifest: &Manifest) -> BTreeSet<PathBuf> {
    manifest
        .delivered
        .iter()
        .filter(|(_, record)| matches!(record.kind, Kind::PersonalFile))
        .map(|(path, _)| path.clone())
        .collect()
}

struct RenderedFile {
    bytes: Vec<u8>,
    byte_copy_reason: Option<&'static str>,
}

fn render_files(
    sources: BTreeMap<PathBuf, Vec<u8>>,
    render: impl Fn(PersonalFileLocation, &SkillFiles) -> Result<SkillFiles>,
) -> Result<BTreeMap<PathBuf, RenderedFile>> {
    let mut files = BTreeMap::new();
    let mut batches = BTreeMap::<_, (SkillFiles, Vec<PathBuf>)>::new();
    for (path, bytes) in sources {
        let spec = location_of(&path).context("invalid personal command or subagent path")?;
        let byte_copy_reason = match spec.stage(&path, &bytes) {
            Ok((source, staged)) => {
                let (inputs, paths) = batches.entry(spec).or_default();
                ensure!(
                    inputs.insert(source, staged).is_none(),
                    "duplicate rulesync source path"
                );
                paths.push(path.clone());
                None
            }
            Err(_) => Some("rulesync could not represent the native file"),
        };
        files.insert(
            path,
            RenderedFile {
                bytes,
                byte_copy_reason,
            },
        );
    }
    for (spec, (inputs, paths)) in batches {
        let mut output = render(spec, &inputs).ok();
        for path in paths {
            let file = files
                .get_mut(&path)
                .context("missing personal file source")?;
            file.byte_copy_reason = match output.as_mut() {
                Some(output) => match output.remove(&path) {
                    Some(emitted) if emitted == file.bytes => {
                        file.bytes = emitted;
                        None
                    }
                    Some(_) => Some("rulesync changed the native file"),
                    None => Some("rulesync did not emit the native file"),
                },
                None => Some("rulesync output was unavailable"),
            };
        }
    }
    Ok(files)
}

pub(super) fn plan_deliveries(
    adoption: &mut Adoption,
    render: impl Fn(PersonalFileLocation, &SkillFiles) -> Result<SkillFiles>,
) -> Result<()> {
    for (path, file) in render_files(adoption.personal_files.clone(), render)? {
        adoption
            .files
            .get_mut(&path)
            .context("missing personal adoption entry")?
            .after = AdoptionEntry::File(file.bytes);
        let note = file.byte_copy_reason.map_or_else(
            || "Adopt at this path. The rulesync output matches every original byte.".to_owned(),
            |reason| format!("Adopt at this path and deliver byte for byte because {reason}."),
        );
        adoption.personal_file_codes.insert(
            path.clone(),
            if file.byte_copy_reason.is_some() {
                Code::PersonalByteCopy
            } else {
                Code::PersonalImport
            },
        );
        adoption.personal_file_notes.insert(path, note);
    }
    Ok(())
}

impl Setup {
    pub(super) fn render_personal_files(
        &self,
        session: &RenderSession<'_>,
        spec: PersonalFileLocation,
        sources: &SkillFiles,
    ) -> Result<SkillFiles> {
        self.render_tree(session, spec.tool, &[spec.feature.as_str()], |input| {
            write_sources(input, sources)
        })
    }
}

pub(super) fn write_sources(input: &Path, sources: &SkillFiles) -> Result<()> {
    for (path, bytes) in sources {
        ensure!(safe_relative(path), "invalid personal rulesync source path");
        let path = input.join(path);
        fs::create_dir_all(path.parent().context("missing rulesync source directory")?)?;
        fs::write(path, bytes)?;
    }
    Ok(())
}

pub(super) fn draft(
    root: &FileRoot,
    path: &Path,
    baseline: &Snapshot,
    current: Snapshot,
    master: &PersonalMaster,
) -> Result<Draft> {
    let before = text(baseline)?;
    let after = text(&current)?;
    ensure!(
        after.len() as u64 <= MAX_FILE_BYTES && !after.contains('\0'),
        "command and subagent text must be UTF-8 and at most 1 MiB"
    );
    let mut diff = describe_personal_file_change(&preview_path(path), before, after);
    let mut diff_codes = Vec::new();
    if (before.contains("\r\n") && before.replace("\r\n", "\n") == after)
        || (after.contains("\r\n") && after.replace("\r\n", "\n") == before)
    {
        diff_codes.push(Code::LineEndingsChanged);
    }
    let permissions = baseline.file_mode().zip(current.file_mode());
    if let Some((previous, edited)) = permissions.filter(|(previous, edited)| previous != edited) {
        diff_codes.push(Code::PermissionsChanged);
        diff.push_str(&format!(
            "Permissions changed from {} to {}.\n",
            previous.description(),
            edited.description()
        ));
    }
    if diff.is_empty() {
        diff = match (&current, baseline) {
            (Snapshot::Absent, _) => {
                diff_codes.push(Code::RemoveEmpty);
                "Remove the empty file."
            }
            (_, Snapshot::Absent) => {
                diff_codes.push(Code::CreateEmpty);
                "Create an empty file."
            }
            _ => bail!("the file changed without a readable text or permissions change"),
        }
        .into();
    }
    let bytes = file_bytes(&current)
        .or_else(|| file_bytes(baseline))
        .unwrap_or_default();
    let (mut destinations, mut skipped) = sharing_paths(path, bytes, &master.overlays);
    destinations.retain(|dest| {
        if dest == path {
            return true;
        }
        let Some(target) = location_of(dest).and_then(|spec| personal_target(spec.tool)) else {
            return false;
        };
        if root
            .read(target.folder(), false)
            .is_ok_and(|snapshot| snapshot.is_directory())
        {
            return true;
        }
        skipped.insert(
            target.id.to_owned(),
            "The tool's setup folder is missing or cannot be read safely.",
        );
        false
    });
    Ok(Draft {
        path: path.to_owned(),
        scope: DraftScope::PersonalFile(Scope {
            destinations,
            skipped,
            previous_mode: baseline.file_mode(),
        }),
        current,
        diff,
        diff_codes,
        needs_review: true,
    })
}

pub(super) struct Scope {
    pub(super) destinations: BTreeSet<PathBuf>,
    skipped: BTreeMap<String, &'static str>,
    previous_mode: Option<FileMode>,
}

#[derive(Serialize)]
pub(super) struct SharePlan {
    operations: Vec<Pending>,
    worktree_writes: BTreeMap<PathBuf, PathBuf>,
    blocked: BTreeMap<PathBuf, String>,
    missing: BTreeSet<PathBuf>,
}

impl SharePlan {
    pub(super) fn describe(&self, draft: &Draft, worktrees: &DeliveryWorktrees) -> String {
        let DraftScope::PersonalFile(scope) = &draft.scope else {
            unreachable!("only personal files have a share plan");
        };
        let mut lines = Vec::new();
        if self.operations.len() == 1 {
            lines.push(format!(
                "All-tools stays with its tool ({}).",
                tool_at(&draft.path)
            ));
        } else {
            lines.push(format!("All-tools from {}.", preview_path(&draft.path)));
        }
        lines.push(keep_source(draft, scope.previous_mode));
        for (tool, note) in &scope.skipped {
            lines.push(format!("  Skip {}. {note}", tool_name(tool)));
        }
        for op in self.operations.iter().filter(|op| op.path != draft.path) {
            lines.push(describe_operation(op, draft.current.file_mode()));
        }
        for (path, reason) in &self.blocked {
            lines.push(format!(
                "  Skip {} at {}. {reason}",
                tool_at(path),
                preview_path(path)
            ));
        }
        for path in &self.missing {
            lines.push(format!(
                "  Skip {} at {}. There is no managed copy to remove.",
                tool_at(path),
                preview_path(path)
            ));
        }
        let roots: BTreeSet<_> = self
            .worktree_writes
            .values()
            .map(|path| worktrees.display(path).escape_debug().to_string())
            .collect();
        if !roots.is_empty() {
            lines.push(format!(
                "Delivery can change the Git checkout at {}.",
                roots.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        lines.join("\n")
    }

    fn notices(&self, draft: &Draft, scope: &Scope) -> Vec<RulesSetupNotice> {
        let code = if self.operations.len() == 1 {
            Code::PersonalOnly
        } else {
            Code::PersonalShare
        };
        let mut notices = vec![code.into(), source_notice(draft)];
        notices.extend(
            scope
                .skipped
                .keys()
                .map(|tool| RulesSetupNotice::for_tool(Code::SkipIncompatible, tool_name(tool))),
        );
        for op in self
            .operations
            .iter()
            .filter(|op| op.path != draft.path && !op.desired.is_directory())
        {
            let code = if op.desired == Snapshot::Absent {
                Code::RemoveCopy
            } else if op.seen == op.desired {
                Code::KeepCopy
            } else if op.seen == Snapshot::Absent {
                Code::CreateCopy
            } else {
                Code::ReplaceCopy
            };
            notices.push(RulesSetupNotice::for_tool(code, tool_at(&op.path)));
            if op
                .desired
                .file_mode()
                .zip(draft.current.file_mode())
                .is_some_and(|(mode, source)| mode.grants_more_read_access_than(source))
            {
                notices.push(RulesSetupNotice::for_tool(
                    Code::WiderRead,
                    tool_at(&op.path),
                ));
            }
        }
        notices.extend(
            self.blocked
                .keys()
                .map(|path| RulesSetupNotice::for_tool(Code::SkipDestination, tool_at(path))),
        );
        notices.extend(
            self.missing
                .iter()
                .map(|path| RulesSetupNotice::for_tool(Code::SkipNoCopy, tool_at(path))),
        );
        if !self.worktree_writes.is_empty() {
            notices.push(Code::GitDelivery.into());
        }
        notices
    }
}

fn source_notice(draft: &Draft) -> RulesSetupNotice {
    let code = if draft.current == Snapshot::Absent {
        Code::KeepRemoval
    } else {
        Code::KeepEdit
    };
    RulesSetupNotice::for_tool(code, tool_at(&draft.path))
}

fn describe_operation(op: &Pending, source_mode: Option<FileMode>) -> String {
    let path = preview_path(&op.path);
    let (action, snapshot) = if op.desired == Snapshot::Absent {
        ("Remove", &op.seen)
    } else if op.seen == op.desired {
        ("Keep", &op.desired)
    } else if op.desired.is_directory() {
        ("Create directory", &op.desired)
    } else if op.seen == Snapshot::Absent {
        ("Create", &op.desired)
    } else {
        ("Replace", &op.desired)
    };
    let mut line = format!("  {action} {path}");
    let mode = match snapshot {
        Snapshot::Directory { mode } => Some(*mode),
        _ => snapshot.file_mode(),
    };
    line.push_str(&permissions(mode));
    if snapshot
        .file_mode()
        .zip(source_mode)
        .is_some_and(|(mode, source)| {
            op.desired != Snapshot::Absent && mode.grants_more_read_access_than(source)
        })
    {
        line.push_str(". More users can read this copy under its file permissions");
    }
    line
}

fn permissions(mode: Option<FileMode>) -> String {
    mode.map(|mode| format!(" with permissions {}", mode.description()))
        .unwrap_or_default()
}

fn keep_source(draft: &Draft, previous_mode: Option<FileMode>) -> String {
    let path = preview_path(&draft.path);
    if draft.current == Snapshot::Absent {
        return format!(
            "  Keep the removal of {path}{}.",
            permissions(previous_mode)
        );
    }
    format!(
        "  Keep the existing edit at {path}{}.",
        permissions(draft.current.file_mode())
    )
}

fn tool_name(tool: &str) -> &str {
    personal_target(tool).map_or(tool, |target| target.tool.display_name())
}

fn tool_at(path: &Path) -> &str {
    location_of(path).map_or("the source tool", |spec| tool_name(spec.tool))
}

impl Card {
    pub(super) fn describe_personal_review(
        &self,
        drafts: &[Draft],
        path: Option<&Path>,
        action: PullbackAction,
        worktrees: &DeliveryWorktrees,
    ) -> Result<(String, String, Vec<RulesSetupNotice>, Vec<Code>)> {
        let Some(path) = path else {
            return Ok((
                self.wire.diff.clone(),
                "Restore the last approved versions for these files.".into(),
                vec![Code::RestoreReview.into()],
                self.wire.diff_codes.clone(),
            ));
        };
        let draft = drafts
            .iter()
            .find(|draft| draft.path == path)
            .context("missing personal file edit")?;
        let DraftScope::PersonalFile(scope) = &draft.scope else {
            bail!("expected a personal command or subagent");
        };
        let (plan, plan_codes) = match action {
            PullbackAction::AllTools => {
                let plan = &self.share_plans[path];
                (plan.describe(draft, worktrees), plan.notices(draft, scope))
            }
            PullbackAction::ToolOnly => {
                let choice = format!("This review is for {} only.", tool_at(path));
                let peers: BTreeSet<_> = scope
                    .skipped
                    .keys()
                    .map(|tool| tool_name(tool))
                    .chain(
                        scope
                            .destinations
                            .iter()
                            .filter(|peer| *peer != path)
                            .map(|peer| tool_at(peer)),
                    )
                    .collect();
                let mut notices = vec![Code::PersonalOnly.into(), source_notice(draft)];
                notices.extend(
                    peers
                        .iter()
                        .map(|tool| RulesSetupNotice::for_tool(Code::SkipToolOnly, tool)),
                );
                let plan = std::iter::once(choice.clone())
                    .chain(std::iter::once(keep_source(draft, scope.previous_mode)))
                    .chain(
                        peers
                            .into_iter()
                            .map(|tool| format!("  Skip {tool}. {choice}")),
                    )
                    .collect::<Vec<_>>()
                    .join("\n");
                (plan, notices)
            }
            PullbackAction::Dismiss | PullbackAction::Revert => {
                unreachable!("this answer does not select a source")
            }
        };
        Ok((
            draft.diff.clone(),
            plan,
            plan_codes,
            draft.diff_codes.clone(),
        ))
    }
}

fn preview_path(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy().escape_debug().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn operation(path: &Path, seen: Snapshot, desired: Snapshot) -> Pending {
    Pending {
        path: path.to_owned(),
        seen,
        desired,
        kind: Some(Kind::PersonalFile),
        allow_link: false,
    }
}

pub(super) fn plan_shares(
    root: &FileRoot,
    manifest: &Manifest,
    draft: &Draft,
    reviewed: &BTreeMap<&Path, &Snapshot>,
    worktrees: &mut DeliveryWorktrees,
) -> Result<SharePlan> {
    let DraftScope::PersonalFile(scope) = &draft.scope else {
        bail!("expected a personal command or subagent");
    };
    let mut operations = Vec::new();
    let mut blocked = BTreeMap::new();
    let mut missing = BTreeSet::new();
    for preferred in scope
        .destinations
        .iter()
        .filter(|path| **path != draft.path)
    {
        let path = match choose_destination(root, manifest, preferred) {
            Ok(path) => path,
            Err(reason) => {
                blocked.insert(preferred.clone(), reason.into());
                continue;
            }
        };
        let path = path.as_path();
        match share_target(root, manifest, draft, reviewed, path) {
            Ok(Some(seen)) => {
                let desired = match &draft.current {
                    Snapshot::File { bytes, mode } => Snapshot::file_with_permissions(
                        bytes.clone(),
                        manifest
                            .delivered
                            .get(path)
                            .and_then(|peer| peer.delivered.file_mode())
                            .unwrap_or(*mode),
                    ),
                    _ => Snapshot::Absent,
                };
                operations.push(operation(path, seen, desired));
            }
            Ok(None) => {
                missing.insert(path.to_owned());
            }
            Err(reason) => {
                blocked.insert(path.to_owned(), reason);
            }
        }
    }
    let mut operations = with_missing_parents(root, operations, None)?;
    // The source receipt is acknowledged only after the peer batch succeeds.
    operations.push(operation(
        &draft.path,
        draft.current.clone(),
        draft.current.clone(),
    ));
    let mut worktree_writes = BTreeMap::new();
    for op in &operations {
        if let Some(git) = worktrees.at(&op.path)? {
            worktree_writes.insert(op.path.clone(), git);
        }
    }
    Ok(SharePlan {
        operations,
        worktree_writes,
        blocked,
        missing,
    })
}

pub(super) fn choose_destination(
    root: &FileRoot,
    manifest: &Manifest,
    preferred: &Path,
) -> std::result::Result<PathBuf, &'static str> {
    let alternatives = alternate_paths(preferred);
    for path in std::iter::once(preferred).chain(alternatives.iter().map(PathBuf::as_path)) {
        match manifest.case_alias(root, path) {
            Ok(None) => (),
            Ok(Some(_)) => {
                return Err("This name aliases a path already recorded in the local setup.")
            }
            Err(_) => {
                return Err("Could not safely check this name against the local setup backup.")
            }
        }
    }
    if alternatives.is_empty() {
        return Ok(preferred.to_owned());
    }
    let mut occupied = std::iter::once(preferred.to_owned())
        .chain(alternatives)
        .filter(|path| {
            manifest.delivered.contains_key(path)
                || !matches!(root.read_personal_file(path, false), Ok(Snapshot::Absent))
        });
    let destination = occupied.next().unwrap_or_else(|| preferred.to_owned());
    if occupied.next().is_some() {
        return Err("OpenCode has both legacy and current definitions for this name.");
    }
    Ok(destination)
}

fn share_target(
    root: &FileRoot,
    manifest: &Manifest,
    draft: &Draft,
    reviewed: &BTreeMap<&Path, &Snapshot>,
    path: &Path,
) -> std::result::Result<Option<Snapshot>, String> {
    if let Some(reason) = path
        .ancestors()
        .find_map(|ancestor| manifest.unmanaged_personal_files.get(ancestor))
    {
        return Err(format!(
            "Adoption marked this path as unmanaged because {reason}. Review it with `wenlan rules adopt`."
        ));
    }
    if path_blocked(&manifest.extension_paths, path)
        || manifest
            .pending
            .iter()
            .any(|pending| path.starts_with(&pending.path))
    {
        return Err("A setup write is pending here.".into());
    }
    let current = root
        .read_personal_file(path, false)
        .map_err(|error| format!("Left unmanaged: {error}"))?;
    if let Some(peer) = manifest.delivered.get(path) {
        let expected = reviewed.get(path).copied().unwrap_or(&peer.delivered);
        if !matches!(peer.kind, Kind::PersonalFile)
            || (!reviewed.contains_key(path)
                && file_bytes(&peer.delivered)
                    != file_bytes(&manifest.delivered[&draft.path].delivered)
                && file_bytes(&peer.delivered) != file_bytes(&draft.current))
            || current != *expected
        {
            return Err("This tool has its own version or a pending edit.".into());
        }
        return Ok(
            (draft.current != Snapshot::Absent || *expected != Snapshot::Absent)
                .then(|| expected.clone()),
        );
    }
    if draft.current == Snapshot::Absent {
        return Ok(None);
    }
    if current != Snapshot::Absent {
        return Err("This path is occupied. Review it with `wenlan rules adopt`.".into());
    }
    Ok(Some(Snapshot::Absent))
}

impl Setup {
    pub(super) fn review_personal_file<'a>(
        &self,
        root: &FileRoot,
        manifest: &mut Manifest,
        card: &Card,
        mut drafts: impl Iterator<Item = &'a Draft>,
        path: Option<&Path>,
        action: &PullbackAction,
    ) -> Result<RulesSyncReport> {
        let mut result = report("Reviewed the local command or subagent");
        let mut operations = match action {
            PullbackAction::AllTools => {
                let plan = path
                    .and_then(|path| card.share_plans.get(path))
                    .context("missing personal file sharing plan")?;
                manifest
                    .worktree_writes
                    .extend(plan.worktree_writes.clone());
                for (path, reason) in &plan.blocked {
                    block_path(
                        &mut result,
                        path,
                        Code::SkipDestination,
                        format!("{}: {reason}", path.display()),
                    );
                }
                plan.operations.clone()
            }
            PullbackAction::ToolOnly => {
                let draft = drafts
                    .find(|draft| path == Some(draft.path.as_path()))
                    .context("missing personal file edit")?;
                vec![operation(
                    &draft.path,
                    draft.current.clone(),
                    draft.current.clone(),
                )]
            }
            PullbackAction::Dismiss | PullbackAction::Revert => drafts
                .map(|draft| {
                    operation(
                        &draft.path,
                        draft.current.clone(),
                        manifest.delivered[&draft.path].delivered.clone(),
                    )
                })
                .collect(),
        };
        let source = if matches!(action, PullbackAction::AllTools) {
            let source = operations.pop().context("missing sharing source receipt")?;
            ensure!(
                root.read_personal_file(&source.path, source.allow_link)? == source.seen,
                "{} changed after the preview. Run wenlan rules status --sync",
                source.path.display()
            );
            Some(source)
        } else {
            None
        };
        let changed = operations
            .iter()
            .filter(|op| op.seen != op.desired)
            .map(|op| op.path.clone())
            .collect::<BTreeSet<_>>();
        for batch in std::iter::once(operations).chain(source.map(|op| vec![op])) {
            for (path, status) in self.apply_batch(root, manifest, batch, BatchMode::Review)? {
                ensure!(
                    status == ApplyOutcome::Applied,
                    "{} changed after the preview. Run wenlan rules status --sync",
                    path.display()
                );
                if changed.contains(&path) {
                    result.written.push(path.display().to_string());
                }
            }
        }
        Ok(result)
    }
}

pub(super) fn file_bytes(snapshot: &Snapshot) -> Option<&[u8]> {
    match snapshot {
        Snapshot::File { bytes, .. } => Some(bytes),
        _ => None,
    }
}
