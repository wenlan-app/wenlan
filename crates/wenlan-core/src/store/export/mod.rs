// SPDX-License-Identifier: Apache-2.0
//! The lossless export (storage migration step 2, spec section 10): the
//! database written out as the files it will become, into a new folder, so a
//! rebuild from that folder can be compared field by field with the database
//! it came from. The database is only read; deleting the folder undoes it.
//!
//! ```text
//! <out>/visible/…            pages and topics, in the folders they are in today
//! <out>/backstage/entities/  entity files
//! <out>/backstage/archive/   archived pages
//! <out>/export-manifest.json every file written, and every value left out
//! ```
//!
//! This part writes pages, topics, entities and archived pages
//! ([`pages::export_pages`]). Memories, Briefs, originals, canvases, settings
//! and history follow in their own parts and add to the same manifest.

pub mod pages;

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::WenlanError;

/// Bumped when a file shape or the manifest changes, so a rebuild never
/// reads an export it does not understand.
pub const EXPORT_FORMAT: u32 = 1;
pub const MANIFEST_FILE: &str = "export-manifest.json";

/// Where an export goes.
#[derive(Debug, Clone)]
pub struct ExportDirs {
    pub root: PathBuf,
}

impl ExportDirs {
    /// `root` must not exist yet, or be an empty folder: an export never
    /// writes over anything.
    pub fn create(root: impl Into<PathBuf>) -> Result<Self, WenlanError> {
        let root = root.into();
        match std::fs::read_dir(&root) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(WenlanError::Validation(format!(
                        "export folder {} is not empty",
                        root.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&root)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Self { root })
    }

    pub fn visible(&self) -> PathBuf {
        self.root.join("visible")
    }

    pub fn backstage(&self) -> PathBuf {
        self.root.join("backstage")
    }

    /// Writes `text` at `relative` (forward slashes) under the root and
    /// returns what the manifest records for it. Refuses a path that would
    /// leave the root or a file that is already there.
    pub(crate) fn write(
        &self,
        relative: &str,
        id: &str,
        file_type: &str,
        text: &str,
    ) -> Result<ExportedFile, WenlanError> {
        let path = self.root.join(safe_relative(relative)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                WenlanError::Validation(format!("cannot write export file {relative}: {error}"))
            })?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        file.sync_all()?;
        Ok(ExportedFile {
            id: id.to_string(),
            file_type: file_type.to_string(),
            path: relative.to_string(),
            sha256: sha256(text.as_bytes()),
        })
    }

    /// Writes the manifest last, so a folder without one is an export that
    /// did not finish.
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<(), WenlanError> {
        let text = serde_json::to_string_pretty(manifest)
            .map_err(|error| WenlanError::Validation(format!("manifest: {error}")))?;
        let path = self.root.join(MANIFEST_FILE);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        std::io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
        Ok(())
    }
}

/// `relative` as a path under the root: forward slashes, no `..`, nothing
/// absolute.
fn safe_relative(relative: &str) -> Result<PathBuf, WenlanError> {
    let path = Path::new(relative);
    let fine = !relative.is_empty()
        && !relative.contains('\\')
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if fine {
        Ok(path.to_path_buf())
    } else {
        Err(WenlanError::Validation(format!(
            "export path {relative:?} is not inside the export folder"
        )))
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedFile {
    pub id: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub path: String,
    pub sha256: String,
}

/// A value the files do not hold, so the comparison can tell an expected
/// difference from a lost one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftOut {
    pub id: String,
    pub field: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub files: Vec<ExportedFile>,
    pub left_out: Vec<LeftOut>,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            format: EXPORT_FORMAT,
            ..Self::default()
        }
    }
}

#[cfg(test)]
#[path = "export_test.rs"]
mod tests;
