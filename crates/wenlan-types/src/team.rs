// SPDX-License-Identifier: Apache-2.0
//! Wire types for a team server: one always-on Wenlan that a team's
//! desktops and AI tools connect to, each person with their own key.
//!
//! The server keeps a member list (`/api/team/*`, on its own computer only)
//! and opens one more listener for the team (`WENLAN_TEAM_ADDR`) that serves
//! nothing but `/health`, `/team/v1/*` (desktops sync a Space's page
//! versions) and `/mcp` (AI tools), every call checked against a member's
//! key. The daemon's own routes are never served there.
//!
//! A member is a person. Every desktop and AI tool of one person uses the
//! same key and is that same member, so their own sentences stay theirs on
//! every computer.

use serde::{Deserialize, Serialize};

/// What a member may do in one Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamAccess {
    /// Read and change the Space's pages.
    Write,
    /// Read only: what they change on their own computer is not taken.
    Read,
}

/// One Space a member can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSpaceAccess {
    pub space: String,
    pub access: TeamAccess,
}

/// A member as the server lists them. The key itself is never in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMember {
    /// Only to tell members apart; never shown.
    pub id: String,
    pub name: String,
    pub spaces: Vec<TeamSpaceAccess>,
    /// When they were added (RFC 3339).
    pub added_at: String,
    /// When they were removed; their key no longer works. Removed members
    /// stay listed so the history keeps their name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
}

/// `GET /api/team`: the member list and the Spaces this server hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamStatus {
    pub members: Vec<TeamMember>,
    /// Spaces this server hosts for the team, by name.
    pub spaces: Vec<String>,
    /// Where the team listener listens, when it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening: Option<String>,
}

/// Body of `POST /api/team/members`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAddMemberRequest {
    pub name: String,
    #[serde(default)]
    pub spaces: Vec<TeamSpaceAccess>,
    /// The address the member's computer reaches this server at, such as
    /// `https://wenlan.example.com` or `http://100.64.1.2:7879`, put in
    /// their join code. Left out, the join code holds the key only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// `POST /api/team/members` and `POST /api/team/members/{id}/key`: the
/// member and their key, shown this once. The server keeps only a hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMemberKey {
    pub member: TeamMember,
    pub key: String,
    /// `<url>#<key>` when a url was given: one string to paste into
    /// Wenlan on the member's computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_code: Option<String>,
}

/// Body of `PUT /api/team/members/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamUpdateMemberRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spaces: Option<Vec<TeamSpaceAccess>>,
}

/// Body of `POST /api/team/spaces`: host a Space of this Wenlan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamHostSpaceRequest {
    pub space: String,
}

/// `GET /team/v1/me`: who a key belongs to and what they can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMe {
    pub member_id: String,
    pub name: String,
    pub spaces: Vec<TeamSpaceAccess>,
}

/// `GET /team/v1/spaces/{space}/files`: every file of the Space's shared
/// folder on the server, by path under it (`space.json` and
/// `members/<member>/pages/<page>/<sha256>.md`). Version files never
/// change, so a path a desktop already has is never fetched again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamFileList {
    pub access: TeamAccess,
    pub files: Vec<String>,
}

/// Body of `POST /api/team/join` on a member's computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamJoinRequest {
    /// The join code (`<url>#<key>`), or the server's url when `key` is
    /// given apart.
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// `POST /api/team/join`: the server's Spaces this person can use. Nothing
/// is joined yet; `POST /api/spaces/{space}/shared/sync` with `host` joins
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamJoinResponse {
    /// The server's address, without the key.
    pub url: String,
    pub me: TeamMe,
}

/// The separator between an AI tool's name and the member it writes for,
/// in the agent name the team server records (`claude-code@team.<member>`).
pub const TEAM_AGENT_SEPARATOR: &str = "@team.";

/// The agent name a team member's AI tool writes under on the server.
pub fn team_agent_name(agent: &str, member: &str) -> String {
    format!("{agent}{TEAM_AGENT_SEPARATOR}{member}")
}

/// The AI tool and the member from a team agent name; `None` for any other
/// agent name.
pub fn split_team_agent(agent: &str) -> Option<(&str, &str)> {
    let (tool, member) = agent.rsplit_once(TEAM_AGENT_SEPARATOR)?;
    let safe = !member.is_empty()
        && member.len() <= 128
        && member
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (safe && !tool.is_empty()).then_some((tool, member))
}

/// Splits a join code (`<url>#<key>`) into the server's url (no trailing
/// slash, no path) and the key. `None` when it is not one.
pub fn split_join_code(code: &str) -> Option<(String, String)> {
    let (url, key) = code.trim().split_once('#')?;
    let key = key.trim();
    if !is_team_key(key) {
        return None;
    }
    Some((
        url.trim().trim_end_matches('/').to_string(),
        key.to_string(),
    ))
}

/// The shape of a member key: `wlt_` and 64 hex digits.
pub fn is_team_key(key: &str) -> bool {
    key.strip_prefix("wlt_")
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_agent_names_round_trip() {
        let name = team_agent_name("claude-code", "a1b2");
        assert_eq!(name, "claude-code@team.a1b2");
        assert_eq!(split_team_agent(&name), Some(("claude-code", "a1b2")));
        assert_eq!(split_team_agent("claude-code"), None);
        assert_eq!(split_team_agent("x@team."), None);
        assert_eq!(split_team_agent("x@team.../etc"), None);
    }

    #[test]
    fn join_codes_split_into_url_and_key() {
        let key = format!("wlt_{}", "a".repeat(64));
        assert_eq!(
            split_join_code(&format!("https://wenlan.example.com/#{key}")),
            Some(("https://wenlan.example.com".into(), key.clone()))
        );
        assert_eq!(split_join_code("https://wenlan.example.com"), None);
        assert_eq!(split_join_code("https://x#wlt_short"), None);
    }
}
