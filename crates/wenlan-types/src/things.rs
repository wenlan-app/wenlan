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
    /// Where it stands: the grouping model's line for the whole thing, else
    /// its newest read conversation's, else the text of the Brief item it
    /// came from. Never a handoff's Brief summary, which speaks for a Space.
    pub status_line: Option<String>,
    /// The Spaces its conversations ran in, and its Brief item's Space.
    pub spaces: Vec<String>,
    /// Unix seconds: its newest conversation write or Brief item change, else
    /// when it was made.
    pub last_activity_at: i64,
    /// Nothing happened in it for [`THING_QUIET_AFTER_SECS`], or the person
    /// hid it from home since its last activity: it leaves "what you're
    /// working on" but is not deleted, stays findable, and comes back on its
    /// own with the next conversation (UX-HOME-5). A thing that started as a
    /// Space Brief item is always quiet: the item is a line an AI tool wrote,
    /// not the person's own work (UX-HOME-6).
    #[serde(default)]
    pub quiet: bool,
    /// Unix seconds: the newest night that steeped what it settled into
    /// long-term memory and was not taken back (UX-CHAT-5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dreamed_at: Option<i64>,
}

/// How long a thing goes without a conversation or Brief change before it
/// leaves "what you're working on": one week.
pub const THING_QUIET_AFTER_SECS: i64 = 7 * 24 * 60 * 60;

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
    /// Things the AI thinks are the same goal as this one, waiting for the
    /// person to merge them or keep them apart (UX-THING-3). Older daemons
    /// leave it out.
    #[serde(default)]
    pub merge_proposals: Vec<ThingRef>,
    /// The newest night that put this thing's points into long-term memory
    /// and was not taken back (UX-CHAT-5). Older daemons leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dream: Option<ThingDream>,
    /// Points steeped from this thing that are still open, not settled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_points: Vec<String>,
}

/// One night's points of a thing, put into long-term memory: how many were
/// settled and how many still open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingDream {
    /// Unix seconds.
    pub at: i64,
    pub decided: usize,
    pub open: usize,
    /// The memories that night wrote, so the page can mark them as new.
    #[serde(default)]
    pub fresh: Vec<String>,
    /// What the work has come to, as that night wrote it from the thing's
    /// standing points: the first paragraph of its page (the whole
    /// experience, step 3; UX-VIEW-6). Only sentences whose every point still
    /// stands and is in the caller's scope. Older daemons leave it out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conclusion: Vec<ThingConclusionSentence>,
}

/// One sentence of a thing's conclusion and the points it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingConclusionSentence {
    pub text: String,
    /// The memories (`source_id`) of the points it rests on; never empty.
    pub points: Vec<String>,
}

/// A point a night wrote for a thing that still stands: not taken back, not
/// forgotten, not replaced (UX-CHAT-5). The big map draws it beside its thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingNightPoint {
    /// The memory's `source_id`.
    pub memory_id: String,
    /// Its title, else the start of its text.
    pub title: String,
    /// Settled, or still open.
    pub decided: bool,
    /// Unix seconds.
    pub created_at: i64,
}

/// A live thing and the points its nights wrote, oldest point first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingNightPoints {
    pub thing_id: String,
    pub title: Option<String>,
    /// As [`ThingSummary::spaces`].
    pub spaces: Vec<String>,
    /// As [`ThingSummary::quiet`]: off home's "what you're working on". The
    /// map frames only the things that are not.
    #[serde(default)]
    pub quiet: bool,
    pub points: Vec<ThingNightPoint>,
    /// What its newest night not taken back did on the big map, for the
    /// morning line and Replay (UX-GRAPH-5, UX-GRAPH-6). `None` for a thing
    /// whose nights were all taken back, and from older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_night: Option<ThingLastNight>,
}

/// A point a night connected to a topic on the big map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightLinkMade {
    /// The point's memory `source_id`.
    pub memory_id: String,
    /// The topic's entity id.
    pub entity_id: String,
}

/// What one night did for a thing on the big map (UX-GRAPH-5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingLastNight {
    /// When the night ran, Unix seconds.
    pub at: i64,
    /// The points it wrote that still stand, as memory `source_id`s.
    pub written: Vec<String>,
    /// The links it made that still stand.
    pub linked: Vec<NightLinkMade>,
}

/// `POST /api/things/nights/undo`: the person takes back the whole night on
/// the big map (UX-GRAPH-5): every thing whose newest night not taken back
/// ran at or after `since` (Unix seconds; the app sends its local midnight).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NightsUndoRequest {
    pub since: i64,
}

/// What taking back a whole night did: how many things' nights, and how many
/// points those nights had written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightsUndo {
    pub things: usize,
    pub points: usize,
}

/// `GET /api/things/points`: every live thing the caller may see that has
/// night points, newest activity first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingNightPointsResponse {
    pub things: Vec<ThingNightPoints>,
}

/// `POST /api/things/points/connect`: connecting the night points kept before
/// nights named their topics, once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightLinksBackfill {
    /// It had already run; nothing was done.
    pub already_done: bool,
    /// Things with points that connect to nothing.
    pub things: usize,
    /// Those points.
    pub points: usize,
    /// Links made. The other points stay under their thing.
    pub linked: usize,
    /// Things whose model call failed; start it again for those.
    pub failed: usize,
}

/// `POST /api/things/points/connect/undo`: the links taken back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NightLinksBackfillUndo {
    pub removed: usize,
}

/// `POST /api/things/{id}/merge` and `POST /api/things/{id}/keep-apart`: the
/// person answers a merge proposal. Merging folds `other_thing_id` into `{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThingMergeRequest {
    pub other_thing_id: String,
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

/// Where a conversation was saved, for the one line the AI tool shows the
/// person (UX-CHAT-7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedIn {
    pub thing_id: String,
    pub title: String,
    /// Opens the thing in the Wenlan app (`wenlan://thing/{id}`).
    pub link: String,
}

/// What a write just did to a page, for the saved-in line (UX-CHAT-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedPageChange {
    /// The AI wrote a new page.
    New,
    /// The AI changed a page that already existed.
    Updated,
    /// The AI wanted to change sentences the person wrote, so the change
    /// waits for them as a suggestion.
    Proposal,
}

/// Body of `POST /api/sessions/{id}/saved-in`. Empty after a plain save;
/// after a page write, the page and what happened to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<SavedPageChange>,
}

/// `POST /api/sessions/{id}/saved-in`: `saved_in` is set only when this
/// conversation's thing changed since it was last reported, and the thing
/// has a name; otherwise it is null. `report_page` says whether the page
/// change in the request is worth a mention: a new page always is, an update
/// or a suggestion once per page per conversation. When neither is set the
/// AI tool says nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInResponse {
    pub saved_in: Option<SavedIn>,
    #[serde(default)]
    pub report_page: bool,
}

/// Who wrote a point on a thing's map (UX-WHO-2: the AI proposes rather than
/// edits a point the person wrote or edited).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThingMapAuthor {
    /// The person.
    You,
    /// The AI tool in the conversation.
    Ai,
    /// Wenlan's own local model, when no AI tool is connected.
    Local,
}

/// Where a point stands. Kept apart from `kind`, since an option can be
/// decided or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThingMapState {
    Open,
    Decided,
    Dropped,
}

/// The conversation lines a point came from. `from` and `to` are message
/// positions in that conversation (0-based, inclusive); older points know
/// only the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingMapSource {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_id: Option<String>,
}

/// One point on a thing's map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThingMapNode {
    pub id: String,
    /// `None` only for the root.
    pub parent_id: Option<String>,
    /// Order among siblings, ascending.
    pub rank: f64,
    /// An open set the AI picks, such as `question`, `option`, `reason` or
    /// `topic`. A client shows a kind it does not know as a plain point.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ThingMapState>,
    pub label: String,
    pub by: ThingMapAuthor,
    #[serde(default)]
    pub sources: Vec<ThingMapSource>,
}

/// A link between two points beyond parent and child, such as one point
/// answering a question. `kind` is an open set like the node's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThingMapEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
}

/// `GET /api/things/{id}/map`: the thing's map. Empty (`revision` 0, no
/// nodes) until something writes it; the recap and the mind map read the same
/// nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThingMapResponse {
    pub thing_id: String,
    pub revision: i64,
    pub nodes: Vec<ThingMapNode>,
    pub edges: Vec<ThingMapEdge>,
    /// Changes the AI or Wenlan's model asked for on points the person
    /// wrote (UX-WHO-2), oldest first. They wait for the person.
    #[serde(default)]
    pub proposals: Vec<ThingMapProposal>,
}

/// One change to a thing's map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThingMapOp {
    /// A new point. `id` lets later ops in the same request name it; the
    /// daemon makes one when it is left out. `parent_id` is `None` only for
    /// the first point of an empty map. `rank` defaults to after the last
    /// sibling.
    Add {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        parent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rank: Option<f64>,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<ThingMapState>,
        label: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sources: Vec<ThingMapSource>,
    },
    /// Change a point's fields; a field left out keeps its value. `state`
    /// can only be set, not cleared.
    Edit {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<ThingMapState>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Replaces the point's sources.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<Vec<ThingMapSource>>,
    },
    /// Put a point under another parent, or reorder it.
    Move {
        id: String,
        parent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rank: Option<f64>,
    },
    /// Remove a point and everything under it, with their links.
    Remove {
        id: String,
    },
    Link {
        from: String,
        to: String,
        kind: String,
    },
    Unlink {
        from: String,
        to: String,
        kind: String,
    },
    /// The person's answer to a proposal.
    Accept {
        proposal_id: String,
    },
    Reject {
        proposal_id: String,
    },
}

/// A change waiting for the person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThingMapProposal {
    pub id: String,
    pub by: ThingMapAuthor,
    pub change: ThingMapOp,
    /// Unix seconds.
    pub created_at: i64,
}

/// `POST /api/things/{id}/map/ops`: apply changes in order, all or none.
/// `expected_revision` is the revision the caller read; a different one
/// answers 409 so the caller reads again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThingMapOpsRequest {
    pub expected_revision: i64,
    pub ops: Vec<ThingMapOp>,
}

/// The map after the change, and how many ops waited as proposals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThingMapOpsResponse {
    #[serde(flatten)]
    pub map: ThingMapResponse,
    pub proposed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_point_keeps_an_unknown_kind_and_optional_fields() {
        let wire = serde_json::json!({
            "thing_id": "thg_a",
            "revision": 3,
            "nodes": [
                {"id": "n1", "parent_id": null, "rank": 0.0, "kind": "question",
                 "state": "open", "label": "Which tagline?", "by": "you",
                 "sources": [{"session_id": "s1", "from": 4, "to": 6}]},
                {"id": "n2", "parent_id": "n1", "rank": 1.0, "kind": "a_future_kind",
                 "label": "Option B", "by": "local"}
            ],
            "edges": [{"from": "n2", "to": "n1", "kind": "answers"}]
        });
        let map: ThingMapResponse = serde_json::from_value(wire).unwrap();
        assert_eq!(map.nodes[0].state, Some(ThingMapState::Open));
        assert_eq!(map.nodes[0].sources[0].memory_id, None);
        assert_eq!(map.nodes[1].kind, "a_future_kind");
        assert!(map.nodes[1].sources.is_empty());
        let back = serde_json::to_value(&map.nodes[1]).unwrap();
        assert!(back.get("state").is_none(), "{back}");
        assert_eq!(back["by"], "local");
    }
}
