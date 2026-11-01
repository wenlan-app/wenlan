// SPDX-License-Identifier: Apache-2.0
//! Things (一件事): one line of work, such as "the new tagline". Every
//! conversation belongs to at least one thing; the daemon sorts new
//! conversations itself, and the person can rename a thing or move a
//! conversation in one step.

use serde::{Deserialize, Serialize};

use crate::sessions::SessionMemoryEntry;

/// Who named a thing or put a conversation in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThingActor {
    Ai,
    Person,
}

/// A thing as home lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingSummary {
    pub id: String,
    /// `None` until the AI has a name for it.
    pub title: Option<String>,
    pub title_by: ThingActor,
    /// Where it stands: the newest Brief handoff summary one of its
    /// conversations wrote, else the text of the Brief item it came from.
    pub status_line: Option<String>,
    /// The Spaces its conversations ran in, and its Brief item's Space.
    pub spaces: Vec<String>,
    /// Unix seconds: its newest conversation write, else when it was made.
    pub last_activity_at: i64,
}

/// `GET /api/things`: newest activity first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingsResponse {
    pub things: Vec<ThingSummary>,
}

/// A conversation in a thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingSessionEntry {
    pub id: String,
    pub tool: String,
    pub title: Option<String>,
    pub last_write_at: i64,
}

/// How a thing's conversations used a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThingPageVia {
    Wrote,
    Read,
}

/// A page one of a thing's conversations wrote or read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingPageEntry {
    pub id: String,
    pub title: String,
    /// `wrote` when any of its conversations wrote it, else `read`.
    pub via: ThingPageVia,
    /// Unix seconds of the newest write, or read when it was only read.
    pub at: i64,
}

/// `GET /api/things/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingDetailResponse {
    #[serde(flatten)]
    pub thing: ThingSummary,
    /// Newest write first.
    pub sessions: Vec<ThingSessionEntry>,
    /// Newest first.
    pub pages: Vec<ThingPageEntry>,
    /// Newest first.
    pub memories: Vec<SessionMemoryEntry>,
}

/// `PATCH /api/things/{id}`: the person renames a thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThingRenameRequest {
    pub title: String,
}

/// `POST /api/sessions/{id}/things/move`: the person moves a conversation out
/// of one thing into another, or into a new thing they name. Exactly one of
/// `to_thing_id` and `new_title`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThingMoveRequest {
    pub from_thing_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_thing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_title: Option<String>,
}

/// A thing a conversation is in, on the session card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingRef {
    pub id: String,
    pub title: Option<String>,
}
