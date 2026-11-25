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
use std::collections::BTreeMap;

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
    /// The hosted Space's id on the server, the same after it is renamed:
    /// members' computers follow a renamed Space by it. Absent for a Space
    /// the server does not host and from servers before it was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The team a server is for: one id that never changes, and the name its
/// owner gave it, the same for everyone on the team. Teams are told apart
/// by `id`, never by the server's address, which may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInfo {
    pub id: String,
    /// `None` until the owner names the team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What a person is on a team: the owner sets it up and brings people in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Owner,
    #[default]
    Member,
}

impl TeamRole {
    pub fn is_member(&self) -> bool {
        *self == TeamRole::Member
    }
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
    #[serde(default, skip_serializing_if = "TeamRole::is_member")]
    pub role: TeamRole,
    /// How they sign in, for the owner's list. Left out when all they have
    /// is a key (and by an older server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<TeamMemberSignIn>,
}

/// How a member signs in, as the owner sees it. Never a subject id and
/// never a whole email address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMemberSignIn {
    /// Providers (`google`, `github`) with an account linked to them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    /// An invite is open and nobody has used it yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invited: bool,
    /// An account used the invite with an email its provider does not vouch
    /// for and waits for the owner to let it in: the email, already masked
    /// by the server (`a***@example.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_email: Option<String>,
    /// How many AI tools (live tool keys) they have connected.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tools: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `GET /api/team`: the member list and the Spaces this server hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamStatus {
    /// The team this server is for. Left out by an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
    pub members: Vec<TeamMember>,
    /// Spaces this server hosts for the team, by name.
    pub spaces: Vec<String>,
    /// Hosted Spaces every member has, now and when added later.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub everyone: Vec<TeamSpaceAccess>,
    /// Where the team listener listens, when it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening: Option<String>,
    /// The address put in invite links, as the owner set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Addresses this computer can likely be reached at by others (a
    /// Tailscale address first, then the local network), for the owner to
    /// pick one when starting a team here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_urls: Vec<String>,
    /// Set when this status is a team server's elsewhere, read for its
    /// owner on this computer: that server's address. Its owner manages it
    /// from here; this computer hosts nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Whether this server signs people in with Google or GitHub (a
    /// provider is set up and its address is `https://`). Left out by an
    /// older server, which has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<bool>,
}

/// `GET /api/team/firewall` on Windows: whether other computers can get
/// through Windows Firewall to this computer's team listener. Off Windows
/// the answer is `null`: nothing here stands in the way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamFirewall {
    /// The program the rules are for: this Wenlan's server.
    pub program: String,
    /// Other computers can get through on every network this computer is
    /// on now.
    pub open: bool,
    /// A network this computer is on is set to Public, where Windows blocks
    /// a new program without asking.
    pub public_network: bool,
}

/// Body of `POST /api/team/start`: makes this computer a team's server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamStartRequest {
    /// The team's name, which every member sees.
    pub name: String,
    /// The owner's own name: how the team sees what they write here.
    pub person: String,
    /// The address put in invite links. Left out: the first suggested one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The port the team listener listens on: on every network of this
    /// computer, or on this computer only when `url` is a loopback address
    /// (`127.0.0.1`, `localhost`, `::1`). Left out: [`DEFAULT_TEAM_PORT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// The team listener's port when none is given.
pub const DEFAULT_TEAM_PORT: u16 = 7879;

/// Body of `POST /api/team/membership`: this computer is on the team an
/// invite link is for, even before any Space is open to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMembershipRequest {
    pub code: String,
}

/// `GET /api/team/membership`: one team this computer has joined. The key
/// is never in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMembershipInfo {
    /// The team server's address.
    pub url: String,
    /// The team, as the server last said. `None` until it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
    /// This person owns the team and manages it from this computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner: bool,
    /// Team-only library Spaces open to this person, as the server last
    /// said (`TeamMe::library_spaces`). Not synced here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_spaces: Vec<TeamSpaceAccess>,
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

/// Body of `POST /team/v1/admin/invites`: a person to invite, who signs in
/// with their own account instead of being handed a key. With an `email`
/// only that address may use the invite; without one, whoever holds the
/// link can.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInviteRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub spaces: Vec<TeamSpaceAccess>,
}

/// The answer to an invite, or to a new one for the same member: the
/// member and the link to give them, shown this once (the server keeps only
/// the token's hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInviteLink {
    pub member: TeamMember,
    /// `<url>/join#inv_<token>`.
    pub link: String,
}

/// Body of `PUT /api/team`: the team's name, which every member sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRenameRequest {
    pub name: String,
}

/// Body of `PUT /api/team/members/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamUpdateMemberRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spaces: Option<Vec<TeamSpaceAccess>>,
}

/// Who on the team has a hosted Space without being added to it one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamEveryone {
    /// Only the members added to it.
    Nobody,
    /// Every member, now and later, may read it.
    Read,
    /// Every member, now and later, may change it.
    Write,
}

/// Body of `POST /api/team/spaces`: host a Space of this Wenlan. With
/// `everyone`, the Space must be hosted already, and only who has it
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamHostSpaceRequest {
    pub space: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub everyone: Option<TeamEveryone>,
    /// Renames the hosted Space `space` to this, for the whole team: its
    /// access and folder go with it, and members' copies follow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_to: Option<String>,
}

/// One person a member shares a Space with, as `GET /team/v1/me` lists them
/// (`people`): enough to name who changed a page, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPerson {
    /// The id their changes are recorded under (`shared_person:<id>`,
    /// `<tool>@team.<id>`).
    pub id: String,
    pub name: String,
    /// The team's owner: the person at the server's own computer, whose
    /// changes are recorded as this computer's person.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner: bool,
}

/// What a request to `GET /team/v1/me` was signed in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamSignedInWith {
    /// A browser's session cookie, from signing in with an account.
    Browser,
    /// The member's key, as an invite link hands it over.
    Key,
}

/// A Google or GitHub account linked to a member, as they see it: the
/// provider and the email it last showed. Never the account's id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamLinkedAccount {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// `GET /auth/providers`, asked before anyone has signed in: the providers
/// sign-in offers (empty when it is off) and the team's name, which the
/// invite page already shows to anyone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamProviders {
    pub providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

/// `GET /team/v1/me`: who a key belongs to and what they can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMe {
    pub member_id: String,
    pub name: String,
    pub spaces: Vec<TeamSpaceAccess>,
    /// Team-only Spaces in the team library open to this member, each with
    /// its library id. Their pages are read through the team server (AI
    /// tools, the web app); they are not copied to members' computers, so
    /// they are kept apart from `spaces`, which computers sync. Left out
    /// when there are none (and by an older server).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_spaces: Vec<TeamSpaceAccess>,
    /// The team. Left out by an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
    #[serde(default, skip_serializing_if = "TeamRole::is_member")]
    pub role: TeamRole,
    /// How this request signed in. Left out by an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_with: Option<TeamSignedInWith>,
    /// Accounts linked to the member. Left out when there are none (and by
    /// an older server).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<TeamLinkedAccount>,
}

/// `GET /team/v1/spaces/{space}/files`: every file of the Space's shared
/// folder on the server, by path under it (`space.json` and
/// `members/<member>/pages/<page>/<sha256>.md`). Version files never
/// change, so a path a desktop already has is never fetched again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamFileList {
    pub access: TeamAccess,
    pub files: Vec<String>,
    /// Page identities to remove from a member's legacy Space. A server
    /// may withhold an archived snapshot or retire a page from this feed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted_pages: Vec<String>,
    /// Terminal refusals for this authenticated member's submitted versions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused_changes: Vec<MemberSyncRefusal>,
    /// The approved Rules page version in the team-wide Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<TeamRulesManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSyncRefusal {
    pub page_id: String,
    pub change: String,
    pub reason: MemberSyncRefusalReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberSyncRefusalReason {
    PageUnavailable,
    InvalidAncestry,
    PageChanged,
    OutsideSpaceFolder,
    CitationsWouldChange,
    ReviewUnavailable,
    Unverifiable,
}

/// An approved Rules page version listed with its shared Space files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRulesManifest {
    pub team: TeamInfo,
    pub page_id: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<TeamRulesProject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<TeamProjectSkill>,
    /// Decisions on this member's proposals, keyed by proposal id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub proposal_decisions: BTreeMap<String, bool>,
}

/// A project master follows the Space's grant and is rendered for its bound repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRulesProject {
    pub space: String,
    pub space_id: String,
    /// A digest of the repository identity. Checkout paths and remote credentials stay local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
}

/// An installed project skill's text files, approved by the person on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamProjectSkill {
    pub name: String,
    pub files: Vec<TeamProjectSkillFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamProjectSkillFile {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRulesProposal {
    pub id: String,
    pub author: String,
    pub markdown: String,
    pub base: String,
    pub proposed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRulesDecisionRequest {
    pub accept: bool,
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

/// The path an invite link opens on the team server: `<url>/join#<key>`.
pub const JOIN_PATH: &str = "/join";

/// The invite link for a member: `<url>/join#<key>`. The key stays after
/// `#`, so a browser never sends it to the server.
pub fn invite_link(url: &str, key: &str) -> String {
    format!("{}{JOIN_PATH}#{key}", url.trim().trim_end_matches('/'))
}

/// Splits an invite link (`<url>/join#<key>`, or the older `<url>#<key>`)
/// into the server's url (no trailing slash, no `/join`) and the key.
/// `None` when it is not one.
pub fn split_join_code(code: &str) -> Option<(String, String)> {
    let (url, key) = code.trim().split_once('#')?;
    let key = key.trim();
    if !is_team_key(key) {
        return None;
    }
    let url = url.trim().trim_end_matches('/');
    let url = url
        .strip_suffix(JOIN_PATH)
        .unwrap_or(url)
        .trim_end_matches('/');
    Some((url.to_string(), key.to_string()))
}

/// The shape of a member key: `wlt_` and 64 hex digits.
pub fn is_team_key(key: &str) -> bool {
    key.strip_prefix("wlt_")
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The shape of a sign-in invite token: `inv_` and 64 hex digits.
pub fn is_invite_token(token: &str) -> bool {
    token
        .strip_prefix("inv_")
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invite_token_has_its_own_shape() {
        assert!(is_invite_token(&format!("inv_{}", "a".repeat(64))));
        assert!(!is_invite_token(&format!("wlt_{}", "a".repeat(64))));
        assert!(!is_invite_token("inv_short"));
        assert!(!is_invite_token(&format!("inv_{}g", "a".repeat(63))));
    }

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
        assert_eq!(
            split_join_code(&invite_link("https://wenlan.example.com/", &key)),
            Some(("https://wenlan.example.com".into(), key.clone()))
        );
        assert_eq!(
            invite_link("https://wenlan.example.com", &key),
            format!("https://wenlan.example.com/join#{key}")
        );
        assert_eq!(split_join_code("https://wenlan.example.com"), None);
        assert_eq!(split_join_code("https://x#wlt_short"), None);
    }

    #[test]
    fn sign_in_fields_are_optional_for_older_servers_and_never_hold_an_id() {
        // An older server's member and `me` answers have none of the new fields.
        let member: TeamMember = serde_json::from_str(
            r#"{"id":"m","name":"Amy","spaces":[],"added_at":"2026-11-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(member.sign_in, None);
        let me: TeamMe =
            serde_json::from_str(r#"{"member_id":"m","name":"Amy","spaces":[]}"#).unwrap();
        assert_eq!((me.signed_in_with, me.accounts.len()), (None, 0));
        let status: TeamStatus = serde_json::from_str(r#"{"members":[],"spaces":[]}"#).unwrap();
        assert_eq!(status.sign_in, None);
        // And a member with nothing to say sends nothing new.
        let text = serde_json::to_string(&member).unwrap();
        assert!(!text.contains("sign_in"));
        let waiting = TeamMemberSignIn {
            waiting_email: Some("e***@studio.io".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&waiting).unwrap(),
            r#"{"waiting_email":"e***@studio.io"}"#
        );
        assert_eq!(
            serde_json::to_string(&TeamSignedInWith::Browser).unwrap(),
            r#""browser""#
        );
    }

    #[test]
    fn team_file_list_accepts_older_servers_and_carries_deleted_page_ids() {
        let old: TeamFileList =
            serde_json::from_str(r#"{"access":"write","files":["space.json"]}"#).unwrap();
        assert!(old.deleted_pages.is_empty());
        assert!(old.refused_changes.is_empty());

        let current = TeamFileList {
            access: TeamAccess::Write,
            files: vec!["space.json".into()],
            deleted_pages: vec!["page-gone".into()],
            refused_changes: vec![MemberSyncRefusal {
                page_id: "page-plan".into(),
                change: "a".repeat(64),
                reason: MemberSyncRefusalReason::CitationsWouldChange,
            }],
            rules: None,
        };
        let value: serde_json::Value = serde_json::to_value(current).unwrap();
        assert_eq!(value["deleted_pages"], serde_json::json!(["page-gone"]));
        assert_eq!(
            value["refused_changes"][0]["reason"],
            "citations_would_change"
        );
        let decoded: TeamFileList = serde_json::from_value(value).unwrap();
        assert_eq!(
            decoded.refused_changes[0].reason,
            MemberSyncRefusalReason::CitationsWouldChange
        );
        let empty = serde_json::to_value(TeamFileList {
            access: TeamAccess::Write,
            files: vec!["space.json".into()],
            deleted_pages: Vec::new(),
            refused_changes: Vec::new(),
            rules: None,
        })
        .unwrap();
        assert!(empty.get("deleted_pages").is_none());
        assert!(empty.get("refused_changes").is_none());
    }
}

/// One of a member's AI tools: a tool key, never the key itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolKey {
    pub id: String,
    /// The name the tool was connected under, as the person typed it.
    pub name: String,
    pub created_at: String,
}

/// `POST /auth/device`: an AI tool asks to be connected. `tool` is the
/// name the person sees on the consent screen; whoever asks chose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDeviceRequest {
    pub tool: String,
}

/// The answer to [`TeamDeviceRequest`]: the long `device_code` only the
/// asking program knows, and the short `user_code` the person types at
/// `verification_uri`. The short code alone never yields a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDeviceStart {
    pub device_code: String,
    /// `ABCD-EFGH`.
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until both codes stop working.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// `POST /auth/device/token`: the asking program polls with its code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDevicePoll {
    pub device_code: String,
}

/// The key, once the person allowed it. Given this once only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDeviceGrant {
    pub key: String,
    pub key_id: String,
    /// The Spaces the member can use now, each at `/mcp/<Space>`.
    pub spaces: Vec<TeamSpaceAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
}

/// `POST /auth/device/lookup` with a short code: which tool asks, and what
/// it will be able to use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDeviceLookup {
    pub tool: String,
    pub spaces: Vec<TeamSpaceAccess>,
}

/// `POST /auth/device/approve` (and `deny`): the short code the person typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDeviceDecision {
    pub user_code: String,
}

/// `POST /team/v1/account/desktop-link`: a one-time link with a new member
/// key, for the desktop app. Given this once only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDesktopLink {
    pub link: String,
}

/// The shape of a short device code: 8 characters of [`DEVICE_CODE_ALPHABET`],
/// with or without the dash after the fourth. Returns it without the dash.
pub fn normalize_user_code(code: &str) -> Option<String> {
    let bare: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (bare.len() == 8 && bare.bytes().all(|b| DEVICE_CODE_ALPHABET.contains(&b))).then_some(bare)
}

/// The short code's characters: no `0 O 1 I L`, which are easy to misread.
pub const DEVICE_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// `GET /api/team/my-tools`: this person's AI tools on one team they joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamToolsOnServer {
    /// The team server's address.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
    pub tools: Vec<TeamToolKey>,
    /// Why the list could not be read from that server, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
