// SPDX-License-Identifier: Apache-2.0
//! Recovery names for regular files held open during delivery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Result};

use super::Pending;
use crate::rulesync_apply::{ApplyOutcome, FileRoot, Snapshot};

const DIRECTORY: &str = "personal-displaced";

pub(super) struct Batch {
    directory: PathBuf,
    files: BTreeMap<usize, PathBuf>,
}

impl Batch {
    pub(super) fn prepare(
        tool: &FileRoot,
        root: &FileRoot,
        operations: &[Pending],
        reserve_undo: bool,
    ) -> Result<Option<Self>> {
        for op in operations {
            preflight(tool, root, op)?;
        }
        let mut files: BTreeMap<_, _> = operations
            .iter()
            .enumerate()
            .filter(|(_, op)| displaces_file(op))
            .map(|(index, op)| (index, op.path.clone()))
            .collect();
        if reserve_undo {
            files.extend(
                operations
                    .iter()
                    .enumerate()
                    .filter(|(_, op)| {
                        op.seen != op.desired && matches!(op.desired, Snapshot::File { .. })
                    })
                    .map(|(index, op)| (index + operations.len(), op.path.clone())),
            );
        }
        if files.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self::prepare_paths(root, files)?))
    }

    pub(super) fn prepare_file(root: &FileRoot, path: &Path) -> Result<PathBuf> {
        let batch = Self::prepare_paths(root, BTreeMap::from([(0, path.to_owned())]))?;
        Ok(batch.directory.join(file_name(0)))
    }

    fn prepare_paths(root: &FileRoot, files: BTreeMap<usize, PathBuf>) -> Result<Self> {
        let directory = Path::new(DIRECTORY).join(uuid::Uuid::new_v4().simple().to_string());
        // The mapping is durable before the first replacement. The held
        // inodes may receive late saves, so the receipt outlives restore.
        let receipt: BTreeMap<_, _> = files
            .iter()
            .map(|(index, path)| (file_name(*index), path))
            .collect();
        ensure!(
            root.apply(
                &directory.join("paths.json"),
                &Snapshot::Absent,
                &Snapshot::file(serde_json::to_vec(&receipt)?),
                false,
            )? == ApplyOutcome::Applied,
            "could not journal recoverable tool files"
        );
        root.sync_directory(&directory)?;
        root.sync_directory(Path::new(DIRECTORY))?;
        root.sync_directory(Path::new(""))?;
        Ok(Self { directory, files })
    }

    pub(super) fn location(&self, index: usize) -> Option<PathBuf> {
        self.files
            .contains_key(&index)
            .then(|| self.directory.join(file_name(index)))
    }

    pub(super) fn sync(&self, root: &FileRoot) -> Result<()> {
        root.sync_directory(&self.directory)?;
        Ok(())
    }
}

fn displaces_file(op: &Pending) -> bool {
    op.seen != op.desired && matches!(op.seen, Snapshot::File { .. })
}

pub(super) fn preflight(tool: &FileRoot, data: &FileRoot, op: &Pending) -> Result<()> {
    if displaces_file(op) {
        tool.check_recovery_volume(&op.path, data, Path::new(DIRECTORY))?;
    }
    Ok(())
}

fn file_name(index: usize) -> String {
    format!("{index}.previous")
}
