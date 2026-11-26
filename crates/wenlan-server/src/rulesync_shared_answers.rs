// SPDX-License-Identifier: Apache-2.0
//! Durable answers to hand edits in shared rule sections.

use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

const FILE: &str = "shared-edit-answers.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Waiting {
    pub(crate) layer_id: String,
    pub(crate) layer_name: String,
    pub(crate) revision: String,
    pub(crate) markdown: String,
    pub(crate) files: Vec<String>,
    pub(crate) first_seen: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Answer {
    Send { proposal: String },
    Decline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Answers {
    format: u32,
    pub(crate) waiting: BTreeMap<String, Waiting>,
    pub(crate) answers: BTreeMap<String, Answer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) answered_edits: BTreeMap<String, AnsweredEdit>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AnsweredEdit {
    pub(crate) edit: Waiting,
    pub(crate) received: Vec<String>,
}

impl Default for Answers {
    fn default() -> Self {
        Self {
            format: 1,
            waiting: BTreeMap::new(),
            answers: BTreeMap::new(),
            answered_edits: BTreeMap::new(),
        }
    }
}

pub(crate) fn read(data: &Path) -> Result<Answers> {
    if !data.exists() {
        return Ok(Answers::default());
    }
    let root = FileRoot::open(data)?;
    match root.read_private(Path::new(FILE), None)? {
        Snapshot::Absent => Ok(Answers::default()),
        Snapshot::File { bytes, .. } => {
            let value: Answers = serde_json::from_slice(&bytes)?;
            ensure!(value.format == 1, "unsupported shared edit answers format");
            Ok(value)
        }
        _ => anyhow::bail!("invalid shared edit answers file"),
    }
}

pub(crate) fn save(data: &Path, answers: &Answers) -> Result<()> {
    let root = FileRoot::open(data)?;
    let path = Path::new(FILE);
    let seen = root.read_private(path, None)?;
    let next = Snapshot::file(serde_json::to_vec(answers)?);
    ensure!(
        root.apply(path, &seen, &next, false)? == ApplyOutcome::Applied,
        "shared edit answers changed during the operation"
    );
    root.sync_directory(Path::new(""))?;
    Ok(())
}
