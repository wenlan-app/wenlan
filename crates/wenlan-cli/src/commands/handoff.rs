// SPDX-License-Identifier: Apache-2.0
//! `wenlan handoff`: one write verb online and in a portable project file.

use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};
use wenlan_types::{HandoffFile, HandoffRequest, SessionRef, HANDOFF_FILE_SCHEMA};

use crate::client::WenlanClient;
use crate::outbox;
use crate::output::{print_json, ResolvedFormat};

#[derive(Debug, Args)]
pub struct HandoffArgs {
    /// Read a HandoffRequest or wenlan.handoff.v1 file.
    #[arg(long, value_name = "JSON_FILE")]
    pub file: PathBuf,
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    args: HandoffArgs,
    effective_space: Option<&str>,
) -> Result<()> {
    let mut handoff = load_handoff(&args.file)?;
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
}
