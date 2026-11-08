// SPDX-License-Identifier: Apache-2.0
//! Wire types for sharing a Space's pages through a folder that a sync tool
//! (Google Drive, iCloud) keeps the same on every member's computer.
//!
//! `POST /api/spaces/{space}/shared/sync` runs one sync pass: this Wenlan
//! writes its new page versions into its own `members/<member_id>/` folder,
//! then takes the latest version of every page from all members' folders.
//! The daemon runs the same pass about every 30 seconds for every shared
//! Space; `GET /api/spaces/{space}/shared` reports the last one.

use serde::{Deserialize, Serialize};

/// Body of `POST /api/spaces/{space}/shared/sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncRequest {
    /// The shared folder, an absolute path on this computer.
    pub folder: String,
    /// The name other members see for this person.
    pub member_name: String,
}

/// What one sync pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncCounts {
    /// Page versions this Wenlan wrote into the folder.
    pub exported: u32,
    /// Pages written here from another member's version.
    pub applied: u32,
    /// Pages where this Wenlan's own edit and another member's edit were
    /// made at the same time and the other one was taken; this Wenlan's
    /// version stays in the page's history.
    pub conflicts: u32,
    /// Files that could not be read yet (still syncing, only in the cloud,
    /// damaged) and pages that could not be written this time.
    pub skipped: u32,
}

/// `GET /api/spaces/{space}/shared`: how this Wenlan shares the Space and
/// what its last sync pass did. The daemon runs a pass about every 30
/// seconds; the route runs one when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncStatus {
    /// The shared folder on this computer.
    pub folder: String,
    /// The name other members see for this person.
    pub member_name: String,
    /// The last pass since the daemon started; `None` before the first.
    pub last: Option<SharedSyncRun>,
}

/// One sync pass of a shared Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncRun {
    /// When it ended (RFC 3339).
    pub at: String,
    pub counts: SharedSyncCounts,
    /// Why it read and wrote nothing: the folder is missing or cannot be
    /// read (a drive unplugged, a cloud folder not there). Nothing here is
    /// changed while the Space is paused; the next pass that finds the
    /// folder carries on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<String>,
    /// Why it stopped part way, when something else failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
