// SPDX-License-Identifier: Apache-2.0
//! Explicit team publication and the host's member-visible library.

use serde::{Deserialize, Serialize};

use crate::team::{TeamInfo, TeamPerson};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamItemKind {
    Page,
    Memory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamAuthorship {
    Human,
    Ai,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamGroup {
    pub id: String,
    pub name: String,
    pub restricted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPublicationReceipt {
    pub team_id: String,
    pub item_id: String,
    pub revision: i64,
    pub memory_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamItem {
    pub team_id: String,
    pub item_id: String,
    pub kind: TeamItemKind,
    pub revision: i64,
    pub space_id: Option<String>,
    pub title: String,
    pub body: String,
    pub contributor: Option<TeamPerson>,
    pub authorship: Option<TeamAuthorship>,
    pub publisher: Option<TeamPerson>,
    pub last_editor: Option<TeamPerson>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamPages {
    pub pages: Vec<TeamItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TeamLibraryInfo {
    pub team: TeamInfo,
    pub spaces: Vec<TeamGroup>,
    pub team_memories: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolScope {
    pub team_id: String,
    pub space_id: Option<String>,
}
