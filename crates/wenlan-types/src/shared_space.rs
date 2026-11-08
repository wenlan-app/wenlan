// SPDX-License-Identifier: Apache-2.0
//! Wire types for sharing a Space's pages, or one page, through a folder
//! that a sync tool (Google Drive, iCloud) keeps the same on every member's
//! computer.
//!
//! `POST /api/spaces/{space}/shared/sync` (or `/api/pages/{id}/shared/sync`)
//! runs one sync pass: this Wenlan writes its new page versions into its own
//! `members/<member_id>/` folder, then takes the latest version of every
//! page from all members' folders. The daemon runs the same pass about every
//! 30 seconds for everything shared; `GET …/shared` reports the last one,
//! `DELETE …/shared` stops sharing from here, `GET /api/shared` lists
//! everything shared and `POST /api/shared/folder` tells what a folder
//! shares before joining it.

use serde::{Deserialize, Serialize};

/// Body of `POST /api/spaces/{space}/shared/sync` and
/// `POST /api/pages/{id}/shared/sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncRequest {
    /// The shared folder, an absolute path on this computer.
    pub folder: String,
    /// The name other members see for this person. Left out, the name in
    /// this Wenlan's profile; the call is refused when that is empty too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_name: Option<String>,
}

/// Whether a folder shares a whole Space or one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedKind {
    Space,
    Page,
}

/// A member of a shared Space or page, as its folder lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMember {
    /// Only to tell members apart; never shown.
    pub id: String,
    pub name: String,
}

/// Body of `POST /api/shared/folder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolderRequest {
    /// A folder on this computer, an absolute path.
    pub folder: String,
}

/// `POST /api/shared/folder`: what a folder shares, before joining it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolder {
    pub kind: SharedKind,
    /// The Space's name, or the page's title when it was first shared.
    pub name: String,
    /// The shared page's id (`kind` page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    pub members: Vec<SharedMember>,
}

/// `POST /api/shared/folder` answer: `shared` is `None` when the folder
/// shares nothing yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolderResponse {
    pub shared: Option<SharedFolder>,
}

/// `GET /api/shared`: everything this Wenlan shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedListResponse {
    pub shares: Vec<SharedSyncStatus>,
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
    pub kind: SharedKind,
    /// The Space's name here, or the page's id.
    pub target: String,
    /// The shared folder on this computer.
    pub folder: String,
    /// This person's member id, to find them in `members`; never shown.
    pub member_id: String,
    /// The name other members see for this person.
    pub member_name: String,
    /// Everyone in the folder's member list, this person included.
    pub members: Vec<SharedMember>,
    /// The last pass since the daemon started; `None` before the first.
    pub last: Option<SharedSyncRun>,
}

/// One sync pass of a shared Space or page.
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
