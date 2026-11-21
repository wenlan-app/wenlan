// SPDX-License-Identifier: Apache-2.0
//! `wenlan handoff`: one write verb online and in a portable project file.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use wenlan_types::{HandoffFile, HandoffRequest, SessionRef, HANDOFF_FILE_SCHEMA};

use crate::client::WenlanClient;
use crate::outbox;
use crate::output::{print_json, ResolvedFormat};

#[derive(Debug, Args)]
#[command(subcommand_negates_reqs = true)]
pub struct HandoffArgs {
    /// Read a HandoffRequest or wenlan.handoff.v1 file.
    #[arg(long, value_name = "JSON_FILE", required = true)]
    pub file: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<HandoffCommand>,
}

#[derive(Debug, Subcommand)]
pub enum HandoffCommand {
    /// Import pending handoff files from this project's root.
    ImportPending,
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    args: HandoffArgs,
    effective_space: Option<&str>,
) -> Result<()> {
    if matches!(args.command, Some(HandoffCommand::ImportPending)) {
        let notes = import_pending(client, effective_space, &std::env::current_dir()?).await?;
        if !quiet {
            for note in notes {
                println!("{note}");
            }
        }
        return Ok(());
    }
    let file = args.file.as_deref().context("--file is required")?;
    let mut handoff = load_handoff(file)?;
    if handoff.session.is_none() {
        handoff.session = client.session().map(ToString::to_string);
    }
    let session = handoff
        .session
        .as_deref()
        .map(SessionRef::parse)
        .transpose()
        .map_err(|error| anyhow::anyhow!("invalid session in handoff file: {error}"))?;
    if let Some(space) = effective_space {
        if handoff.request.space.trim() != space {
            bail!(
                "Handoff Space '{}' conflicts with resolved Space '{}'",
                handoff.request.space,
                space
            );
        }
    }

    match client.handoff(&handoff.request, session.as_ref()).await {
        Ok(receipt) => {
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&receipt)?,
                    ResolvedFormat::Table => println!(
                        "Handoff saved for Space '{}' at Brief version {} ({} applied, {} conflicts).",
                        receipt.space,
                        receipt.brief_version,
                        receipt.applied.len(),
                        receipt.conflicts.len()
                    ),
                }
            }
        }
        Err(error) if client.is_local() && outbox::is_daemon_unreachable(&error) => {
            let path = write_project_file(&handoff, &std::env::current_dir()?)?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => print_json(&serde_json::json!({
                        "status": "saved_in_project",
                        "path": path,
                        "operation_id": handoff.request.operation_id,
                    }))?,
                    ResolvedFormat::Table => println!(
                        "Wenlan is not running; handoff saved in the project folder: {}",
                        path.display()
                    ),
                }
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

async fn import_pending(
    client: &WenlanClient,
    effective_space: Option<&str>,
    cwd: &Path,
) -> Result<Vec<String>> {
    import_pending_with(cwd, effective_space, |file| {
        let session = file
            .session
            .as_deref()
            .map(SessionRef::parse)
            .transpose()
            .map_err(anyhow::Error::msg);
        async move {
            let session = session?;
            client.handoff(&file.request, session.as_ref()).await
        }
    })
    .await
}

async fn import_pending_with<F, Fut>(
    cwd: &Path,
    effective_space: Option<&str>,
    mut apply: F,
) -> Result<Vec<String>>
where
    F: FnMut(HandoffFile) -> Fut,
    Fut: std::future::Future<Output = Result<wenlan_types::HandoffReceipt>>,
{
    let Some(effective_space) = effective_space
        .map(str::trim)
        .filter(|space| !space.is_empty())
    else {
        return Ok(vec![
            "Pending handoff import skipped: no project Space resolved".into(),
        ]);
    };
    let root = project_root(cwd);
    let mut pending: Vec<(PathBuf, std::fs::FileType)> = std::fs::read_dir(root)
        .with_context(|| format!("reading project folder {}", root.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with(".wenlan-handoff-") && name.ends_with(".json"))
                .then(|| entry.file_type().ok().map(|kind| (path, kind)))?
        })
        .collect();
    pending.sort_by(|a, b| a.0.cmp(&b.0));
    let mut notes = Vec::new();
    let one_line = |error: &dyn std::fmt::Display| {
        error
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut regular = Vec::new();
    for (path, kind) in pending {
        if kind.is_symlink() {
            notes.push(format!(
                "Kept {}: pending file is a symlink",
                path.display()
            ));
        } else if kind.is_file() {
            regular.push(path);
        }
    }
    if regular.is_empty() {
        return Ok(notes);
    }
    let imported = match checked_imported_dir(root) {
        Ok(imported) => imported,
        Err(reason) => {
            notes.push(format!("Kept pending handoffs: {reason}"));
            return Ok(notes);
        }
    };

    for path in regular {
        let file_name = path.file_name().unwrap_or_default().to_os_string();
        let claimed = root.join(format!(
            "{}.importing-{}",
            file_name.to_string_lossy(),
            uuid::Uuid::new_v4().simple()
        ));
        if let Err(error) = std::fs::rename(&path, &claimed) {
            notes.push(format!(
                "Kept {}: could not claim it: {}",
                path.display(),
                one_line(&error)
            ));
            continue;
        }
        let claimed_kind = match std::fs::symlink_metadata(&claimed) {
            Ok(metadata) => metadata.file_type(),
            Err(error) => {
                notes.push(restore_claimed(
                    &claimed,
                    &path,
                    &format!("could not inspect claimed file: {}", one_line(&error)),
                ));
                continue;
            }
        };
        if claimed_kind.is_symlink() || !claimed_kind.is_file() {
            notes.push(restore_claimed(
                &claimed,
                &path,
                "claimed pending file is a symlink or not a regular file",
            ));
            continue;
        }
        let file = match load_handoff(&claimed) {
            Ok(file) => file,
            Err(error) => {
                notes.push(restore_claimed(&claimed, &path, &one_line(&error)));
                continue;
            }
        };
        if file.request.space.trim().is_empty() || file.request.space.trim() != effective_space {
            notes.push(restore_claimed(
                &claimed,
                &path,
                &format!(
                    "Handoff Space '{}' conflicts with resolved Space '{}'",
                    file.request.space, effective_space
                ),
            ));
            continue;
        }
        match apply(file).await {
            Ok(receipt) if receipt.conflicts.is_empty() => {
                if let Err(reason) = validate_imported_dir(&imported) {
                    notes.push(restore_claimed(&claimed, &path, &reason));
                    continue;
                }
                match archive_claimed(&claimed, &imported, &file_name) {
                    Ok(destination) => notes.push(format!("Imported {}", destination.display())),
                    Err(reason) => notes.push(restore_claimed(&claimed, &path, &reason)),
                }
            }
            Ok(_) => notes.push(restore_claimed(&claimed, &path, "handoff conflicted")),
            Err(error) => notes.push(restore_claimed(&claimed, &path, &one_line(&error))),
        }
    }
    Ok(notes)
}

fn checked_imported_dir(root: &Path) -> Result<PathBuf, String> {
    let imported = root.join("imported");
    match std::fs::symlink_metadata(&imported) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&imported)
                .map_err(|error| format!("could not create {}: {error}", imported.display()))?;
        }
        Err(error) => return Err(format!("could not inspect {}: {error}", imported.display())),
    }
    validate_imported_dir(&imported)?;
    Ok(imported)
}

fn validate_imported_dir(imported: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(imported)
        .map_err(|error| format!("could not inspect {}: {error}", imported.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{} is a symlink", imported.display()));
    }
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", imported.display()));
    }
    Ok(())
}

fn archive_claimed(
    claimed: &Path,
    imported: &Path,
    file_name: &std::ffi::OsStr,
) -> Result<PathBuf, String> {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(".wenlan-handoff");
    for replay in 1usize.. {
        let destination = if replay == 1 {
            imported.join(file_name)
        } else {
            imported.join(format!("{stem}-replay-{replay}.json"))
        };
        match std::fs::hard_link(claimed, &destination) {
            Ok(()) => {
                std::fs::remove_file(claimed).map_err(|error| {
                    format!(
                        "archived at {} but could not remove claimed file {}: {error}",
                        destination.display(),
                        claimed.display()
                    )
                })?;
                return Ok(destination);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not archive {} without overwrite: {error}",
                    claimed.display()
                ))
            }
        }
    }
    unreachable!()
}

fn restore_claimed(claimed: &Path, original: &Path, reason: &str) -> String {
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if std::fs::symlink_metadata(original).is_ok() {
        return format!(
            "Kept {}: {reason}; replacement exists at {}, so claimed file stays",
            claimed.display(),
            original.display()
        );
    }
    match std::fs::hard_link(claimed, original) {
        Ok(()) => match std::fs::remove_file(claimed) {
            Ok(()) => format!("Kept {}: {reason}", original.display()),
            Err(error) => format!(
                "Kept {} and {}: {reason}; could not remove claimed link: {error}",
                original.display(),
                claimed.display()
            ),
        },
        Err(error) => format!(
            "Kept {}: {reason}; could not restore {}: {error}",
            claimed.display(),
            original.display()
        ),
    }
}

pub fn load_handoff(path: &Path) -> Result<HandoffFile> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("reading handoff {}", path.display()))?;
    if let Ok(file) = serde_json::from_str::<HandoffFile>(&body) {
        if file.schema != HANDOFF_FILE_SCHEMA {
            bail!("unsupported handoff file schema '{}'", file.schema);
        }
        return Ok(file);
    }
    let request: HandoffRequest = serde_json::from_str(&body)
        .with_context(|| format!("invalid handoff JSON in {}", path.display()))?;
    Ok(HandoffFile {
        schema: HANDOFF_FILE_SCHEMA.into(),
        request,
        session: None,
    })
}

fn project_root(start: &Path) -> &Path {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(start)
}

fn safe_operation_id(operation_id: &str) -> String {
    let value: String = operation_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if value.is_empty() {
        "handoff".into()
    } else {
        value
    }
}

pub fn write_project_file(file: &HandoffFile, cwd: &Path) -> Result<PathBuf> {
    let path = project_root(cwd).join(format!(
        ".wenlan-handoff-{}.json",
        safe_operation_id(&file.request.operation_id)
    ));
    let bytes = serde_json::to_vec_pretty(file)?;
    if path.exists() {
        let existing = std::fs::read(&path)?;
        if existing == bytes {
            return Ok(path);
        }
        bail!(
            "refusing to overwrite a different handoff file at {}",
            path.display()
        );
    }
    use std::io::Write;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("creating project handoff {}", path.display()))?;
    output
        .write_all(&bytes)
        .with_context(|| format!("writing project handoff {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use wenlan_types::{BriefItemState, BriefMutation};

    fn file() -> HandoffFile {
        HandoffFile {
            schema: HANDOFF_FILE_SCHEMA.into(),
            session: Some("claude-code:test".into()),
            request: HandoffRequest {
                space: "work".into(),
                caller_id: "test".into(),
                operation_id: "same/op".into(),
                summary: None,
                mutations: vec![BriefMutation::Add {
                    text: "one line".into(),
                    details: None,
                    state: BriefItemState::Active,
                    added_at: None,
                    gate: None,
                }],
            },
        }
    }

    #[test]
    fn ux_hand_6_offline_file_is_project_local_and_same_operation_is_stable() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let nested = temp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        let first = write_project_file(&file(), &nested).unwrap();
        let second = write_project_file(&file(), &nested).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.parent(), Some(temp.path()));
        assert_eq!(load_handoff(&first).unwrap(), file());
    }

    #[tokio::test]
    async fn import_pending_moves_success_replays_once_and_keeps_bad_file() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let pending = write_project_file(&file(), temp.path()).unwrap();
        let bytes = std::fs::read(&pending).unwrap();
        let bad = temp.path().join(".wenlan-handoff-bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let db = Arc::new(
            wenlan_core::db::MemoryDB::new(
                &temp.path().join("db"),
                Arc::new(wenlan_core::events::NoopEmitter),
            )
            .await
            .unwrap(),
        );
        let apply = |file: HandoffFile| {
            let db = Arc::clone(&db);
            async move { Ok(db.apply_brief_update(&file.request).await?) }
        };

        let notes = import_pending_with(temp.path(), Some("work"), apply)
            .await
            .unwrap();
        assert!(notes.iter().any(|note| note.starts_with("Imported ")));
        assert!(notes.iter().any(|note| note.starts_with("Kept ")));
        let imported = temp
            .path()
            .join("imported")
            .join(pending.file_name().unwrap());
        assert!(imported.exists());
        assert!(!pending.exists());
        assert!(bad.exists());

        std::fs::write(&pending, bytes).unwrap();
        import_pending_with(temp.path(), Some("work"), apply)
            .await
            .unwrap();
        let brief = db.get_brief_by_space_name("work").await.unwrap().unwrap();
        assert_eq!(
            brief.active.len(),
            1,
            "operation id applies once in the database"
        );
        assert!(!pending.exists());
        assert!(temp
            .path()
            .join("imported/.wenlan-handoff-same_op-replay-2.json")
            .exists());
    }

    #[tokio::test]
    async fn import_pending_requires_the_resolved_matching_space() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let pending = write_project_file(&file(), temp.path()).unwrap();
        let calls = Arc::new(Mutex::new(0usize));
        let apply = |_: HandoffFile| {
            let calls = Arc::clone(&calls);
            async move {
                *calls.lock().unwrap() += 1;
                unreachable!("a missing or mismatched Space must not apply")
            }
        };

        let no_space = import_pending_with(temp.path(), None, apply).await.unwrap();
        assert!(pending.exists());
        assert!(no_space
            .iter()
            .any(|note| note.contains("no project Space")));
        let wrong_space = import_pending_with(temp.path(), Some("other"), apply)
            .await
            .unwrap();
        assert!(pending.exists());
        assert!(wrong_space.iter().any(|note| note.contains("conflicts")));
        let mut empty_space = file();
        empty_space.request.space.clear();
        std::fs::write(&pending, serde_json::to_vec_pretty(&empty_space).unwrap()).unwrap();
        let empty = import_pending_with(temp.path(), Some("work"), apply)
            .await
            .unwrap();
        assert!(pending.exists());
        assert!(empty.iter().any(|note| note.contains("conflicts")));
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn import_pending_claims_before_posting_and_leaves_a_replacement_pending() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let pending = write_project_file(&file(), temp.path()).unwrap();
        let mut replacement = file();
        replacement.request.operation_id = "replacement".into();
        replacement.request.mutations = vec![BriefMutation::Add {
            text: "new pending line".into(),
            details: None,
            state: BriefItemState::Active,
            added_at: None,
            gate: None,
        }];
        let replacement_bytes = serde_json::to_vec_pretty(&replacement).unwrap();
        let apply = |file: HandoffFile| {
            let pending = pending.clone();
            let replacement_bytes = replacement_bytes.clone();
            async move {
                std::fs::write(&pending, replacement_bytes).unwrap();
                Ok(wenlan_types::HandoffReceipt {
                    space: file.request.space,
                    brief_version: 1,
                    applied: vec![],
                    conflicts: vec![],
                    projection_path: None,
                    warnings: vec![],
                })
            }
        };

        import_pending_with(temp.path(), Some("work"), apply)
            .await
            .unwrap();
        assert_eq!(load_handoff(&pending).unwrap(), replacement);
        let imported = temp
            .path()
            .join("imported")
            .join(pending.file_name().unwrap());
        assert_eq!(load_handoff(&imported).unwrap(), file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn import_pending_refuses_symlink_files_and_archive_directories() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let target = temp.path().join("target.json");
        std::fs::write(&target, serde_json::to_vec_pretty(&file()).unwrap()).unwrap();
        let pending = temp.path().join(".wenlan-handoff-linked.json");
        symlink(&target, &pending).unwrap();
        let calls = Arc::new(Mutex::new(0usize));
        let apply = |_: HandoffFile| {
            let calls = Arc::clone(&calls);
            async move {
                *calls.lock().unwrap() += 1;
                unreachable!("a symlink must not apply")
            }
        };

        let notes = import_pending_with(temp.path(), Some("work"), apply)
            .await
            .unwrap();
        assert!(pending.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(notes.iter().any(|note| note.contains("symlink")));

        std::fs::remove_file(&pending).unwrap();
        let real_pending = write_project_file(&file(), temp.path()).unwrap();
        let archive_target = temp.path().join("archive-target");
        std::fs::create_dir(&archive_target).unwrap();
        symlink(&archive_target, temp.path().join("imported")).unwrap();
        let notes = import_pending_with(temp.path(), Some("work"), apply)
            .await
            .unwrap();
        assert!(real_pending.exists());
        assert!(std::fs::read_dir(&archive_target).unwrap().next().is_none());
        assert!(notes
            .iter()
            .any(|note| note.contains("imported") && note.contains("symlink")));
        assert_eq!(*calls.lock().unwrap(), 0);
    }
}
