// SPDX-License-Identifier: Apache-2.0
//! Local settings migration requests shared by the terminal and daemon.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::rulesync::AdoptionChange;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPreviewRequest {
    pub source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsApplyRequest {
    pub source: PathBuf,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsPlan {
    /// Binds confirmation to the selected source and every destination snapshot.
    pub token: String,
    pub files: Vec<AdoptionChange>,
}
