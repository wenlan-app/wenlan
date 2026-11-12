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
    /// The shared folder, an absolute path on this computer. Empty when
    /// `host` is given: the Space then syncs through a team server.
    #[serde(default)]
    pub folder: String,
    /// Sync through a team server instead of a folder (see
    /// [`crate::team`]). The name is the server's, `member_name` is ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<SharedHost>,
    /// Join the folder as this member, one already in it: this person's own
    /// other computer. Their sentences then stay theirs here. Left out, a
    /// new member the first time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    /// The name other members see for this person. Left out, the name in
    /// this Wenlan's profile; the call is refused when that is empty too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_name: Option<String>,
}

/// A team server to sync a Space through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedHost {
    /// The join code (`<url>#<key>`), or the server's url when `key` is
    /// given apart.
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The Space's name on the server. Left out, the Space's name here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
}

/// Whether a folder shares a whole Space, one page, or the whole library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedKind {
    Space,
    Page,
    /// Every page of every Space, between one person's own computers.
    Library,
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
    /// Pages the folder archives that were left as they are this pass (the
    /// library only): archiving them would have emptied most of it. Shown
    /// as [`SharedSyncRun::held_back`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub held_back: u32,
    /// Memory versions this Wenlan wrote into the folder (the library only:
    /// a Space or a page never shares memories).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub memories_exported: u32,
    /// Memories made, changed or forgotten here from another computer's
    /// version (the library only).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub memories_applied: u32,
    /// Memories the folder forgets that were left as they are this pass:
    /// forgetting them would have emptied most of the library's memories.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub memories_held_back: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `GET /api/spaces/{space}/shared`: how this Wenlan shares the Space and
/// what its last sync pass did. The daemon runs a pass about every 30
/// seconds; the route runs one when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncStatus {
    pub kind: SharedKind,
    /// The Space's name here, or the page's id.
    pub target: String,
    /// The shared folder on this computer (for a Space synced through a
    /// team server, Wenlan's own copy of the server's folder).
    pub folder: String,
    /// The team server's address, when the Space syncs through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The team the Space belongs to, when it syncs through a team server
    /// or this Wenlan hosts it; `None` until the server has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<crate::team::TeamInfo>,
    /// The team server lets this person read the Space only: what they
    /// change here is not taken, and the server's version is kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// This Wenlan hosts the Space for a team (see [`crate::team`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hosted: bool,
    /// This person's member id, to find them in `members`; never shown.
    pub member_id: String,
    /// The name other members see for this person.
    pub member_name: String,
    /// Everyone in the folder's member list, this person included.
    pub members: Vec<SharedMember>,
    /// The last pass since the daemon started; `None` before the first.
    pub last: Option<SharedSyncRun>,
    /// The library's computers, as its folder lists them (library only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<SharedDevice>,
    /// The id of the computer that writes the phone read copy, takes the
    /// inbox and runs background AI page writing (library only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_device: Option<String>,
    /// This computer's id in `devices` (library only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub this_device: Option<String>,
    /// Why the last reading copy could not be written (library, main
    /// computer only). The sync itself is not affected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_copy_error: Option<String>,
    /// How far a pass that is running now has come; `None` between passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<SharedSyncProgress>,
}

/// Pages a running pass has gone through, of all it goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSyncProgress {
    pub done: u32,
    pub total: u32,
}

/// One computer of a shared library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedDevice {
    pub id: String,
    /// The computer's name.
    pub name: String,
}

/// Body of `POST /api/library/shared/main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetMainDeviceRequest {
    /// A computer in the library's `devices`.
    pub device_id: String,
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
    /// Why pages the folder archives were left alone this pass (library
    /// only): archiving them would have emptied most of the library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_back: Option<String>,
}
