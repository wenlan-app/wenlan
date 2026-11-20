// SPDX-License-Identifier: AGPL-3.0-only
//! Who is acting when the team listener forwards a member's browser or MCP
//! call to this daemon over loopback.
//!
//! Both transports attach the shared credential only to daemon requests
//! they build after authenticating a Space grant. The browser gate also
//! forwards the acting member for edits made in person. These requests
//! carry only headers set by the forwarding code. Calls from the person
//! at this computer have no forwarding credential.

use std::sync::LazyLock;

use axum::extract::{FromRequestParts, MatchedPath, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use wenlan_core::shared_space::version::{Author, AuthorKind};

/// The secret the gate proves it is this process with.
pub(crate) const SECRET_HEADER: &str = "x-wenlan-team-gate";
/// The acting member's id.
pub(crate) const MEMBER_HEADER: &str = "x-wenlan-team-member";
/// The acting member's name (percent-free UTF-8; carried for logs).
pub(crate) const MEMBER_NAME_HEADER: &str = "x-wenlan-team-member-name";
/// What is acting: only `person` is accepted.
pub(crate) const KIND_HEADER: &str = "x-wenlan-team-member-kind";
/// Internal Space lookup for a draft mutation. The browser gate constructs
/// this header itself and forwards no caller-supplied headers.
pub(crate) const DRAFT_SPACE_LOOKUP_HEADER: &str = "x-wenlan-draft-space-lookup";

static SECRET: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
});

/// This process's secret, for the gate's forwarded calls only.
pub(crate) fn secret() -> &'static str {
    SECRET.as_str()
}

/// A dedicated loopback client for the hosted MCP service. Redirects and
/// proxies must not carry the forwarding credential outside this daemon.
pub(crate) fn mcp_client() -> reqwest::Result<reqwest::Client> {
    let mut credential = HeaderValue::from_static(secret());
    credential.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(SECRET_HEADER, credential);
    reqwest::Client::builder()
        .default_headers(headers)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// A team member acting in person, through the team server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActingMember {
    pub id: String,
    pub name: String,
}

impl ActingMember {
    /// The writer recorded for this member's hand edit: the same
    /// `shared_person:<member>` a desktop member's edit carries when it
    /// syncs through the team's files, so it is shared as theirs.
    pub fn edited_by(&self) -> String {
        wenlan_core::shared_space::shared_edited_by(&Author {
            member: self.id.clone(),
            kind: AuthorKind::Person,
            agent: None,
        })
    }
}

/// The acting member, when the team gate forwarded this call.
#[derive(Debug, Clone, Default)]
pub struct TeamActor(pub Option<ActingMember>);

fn same(a: &[u8], b: &[u8]) -> bool {
    a.iter()
        .zip(b.iter())
        .fold(a.len() ^ b.len(), |acc, (x, y)| acc | usize::from(x ^ y))
        == 0
}

/// A hosted Space grant covers shared material. Private memory reads belong
/// to the local library. Explicitly published memories can get their own
/// reader here when team memory sharing is introduced.
pub(crate) fn can_read_private_memories(headers: &HeaderMap) -> bool {
    !headers
        .get(SECRET_HEADER)
        .is_some_and(|given| same(given.as_bytes(), secret().as_bytes()))
}

pub(crate) fn required_space(pinned: Option<&str>) -> Result<&str, crate::error::ServerError> {
    pinned.ok_or_else(|| crate::error::ServerError::NotFound("Space not found".into()))
}

/// Limit authenticated forwarding to the browser and MCP calls audited for
/// that distinction. Match Axum's route template, never a client-supplied ID.
pub(crate) async fn guard_private_memory(req: Request, next: Next) -> Response {
    if !req.headers().contains_key(SECRET_HEADER) {
        return next.run(req).await;
    }
    if can_read_private_memories(req.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let admitted = req.extensions().get::<MatchedPath>().is_some_and(|path| {
        let path = path.as_str();
        crate::team_routes::is_member_app_call(req.method(), path)
            || matches!(
                (req.method(), path),
                (&Method::POST, "/api/memory/search" | "/api/brief")
                    | (&Method::GET, "/api/skills/rules")
                    | (
                        &Method::POST,
                        "/api/skills/record" | "/api/pages" | "/api/pages/{id}/rename"
                    )
                    | (&Method::PUT | &Method::DELETE, "/api/pages/{id}")
            )
    });
    if !admitted {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({"error": "This route is unavailable to team members."})),
        )
            .into_response();
    }
    next.run(req).await
}

/// The member these headers name, when they carry this process's secret.
pub(crate) fn from_headers(headers: &HeaderMap) -> Option<ActingMember> {
    if can_read_private_memories(headers) {
        return None;
    }
    if headers.get(KIND_HEADER)?.to_str().ok()? != "person" {
        return None;
    }
    let id = headers
        .get(MEMBER_HEADER)?
        .to_str()
        .ok()?
        .trim()
        .to_string();
    if !wenlan_core::shared_space::version::is_safe_id(&id) {
        return None;
    }
    // UTF-8, not `to_str` (visible ASCII only): a member named 王小明
    // keeps their name.
    let name = headers
        .get(MEMBER_NAME_HEADER)
        .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    Some(ActingMember { id, name })
}

impl<S> FromRequestParts<S> for TeamActor
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(TeamActor(from_headers(&parts.headers)))
    }
}

/// Whether a page or draft filed under `page_space` may be changed by a
/// member call pinned to `pinned`: only in the Space the gate checked.
pub(crate) fn in_pinned_space(page_space: Option<&str>, pinned: Option<&str>) -> bool {
    match (page_space.map(str::trim), pinned.map(str::trim)) {
        (Some(page), Some(pinned)) => !page.is_empty() && page == pinned,
        _ => false,
    }
}

#[cfg(test)]
#[path = "team_memory_privacy_test.rs"]
mod memory_privacy_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(secret_value: &str, kind: &str, id: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(SECRET_HEADER, HeaderValue::from_str(secret_value).unwrap());
        map.insert(KIND_HEADER, HeaderValue::from_str(kind).unwrap());
        map.insert(MEMBER_HEADER, HeaderValue::from_str(id).unwrap());
        map.insert(MEMBER_NAME_HEADER, HeaderValue::from_static("Amy"));
        map
    }

    #[test]
    fn only_this_process_secret_names_a_member() {
        let real = from_headers(&headers(secret(), "person", "m-amy")).unwrap();
        assert_eq!(real.id, "m-amy");
        assert_eq!(real.edited_by(), "shared_person:m-amy");
        assert!(from_headers(&headers("guess", "person", "m-amy")).is_none());
        assert!(from_headers(&headers("", "person", "m-amy")).is_none());
        assert!(from_headers(&headers(secret(), "ai", "m-amy")).is_none());
        assert!(from_headers(&headers(secret(), "person", "../x")).is_none());
        let mut no_secret = headers(secret(), "person", "m-amy");
        no_secret.remove(SECRET_HEADER);
        assert!(from_headers(&no_secret).is_none());
    }

    #[test]
    fn a_member_named_in_chinese_keeps_their_name() {
        let mut map = headers(secret(), "person", "m-wang");
        map.insert(MEMBER_NAME_HEADER, HeaderValue::from_str("王小明").unwrap());
        assert_eq!(from_headers(&map).unwrap().name, "王小明");
    }

    #[test]
    fn a_member_changes_only_the_pinned_space() {
        assert!(in_pinned_space(Some("Alpha"), Some("Alpha")));
        assert!(!in_pinned_space(Some("Beta"), Some("Alpha")));
        assert!(!in_pinned_space(None, Some("Alpha")));
        assert!(!in_pinned_space(Some("Alpha"), None));
    }
}
