// SPDX-License-Identifier: AGPL-3.0-only
//! Who is acting when the team listener forwards a team member's browser
//! call to this daemon over loopback.
//!
//! The team listener's gate (`team_app_gate`) sets these headers itself,
//! from the member its key authenticated, on a call it builds anew, so no
//! header the browser sent reaches this daemon. They are believed only next to this process's own secret,
//! which never leaves the process, so no other local program and no
//! browser can name a member. Without them every handler behaves as it
//! always has for the person at this computer.

use std::sync::LazyLock;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use wenlan_core::shared_space::version::{Author, AuthorKind};

/// The secret the gate proves it is this process with.
pub(crate) const SECRET_HEADER: &str = "x-wenlan-team-gate";
/// The acting member's id.
pub(crate) const MEMBER_HEADER: &str = "x-wenlan-team-member";
/// The acting member's name (percent-free UTF-8; carried for logs).
pub(crate) const MEMBER_NAME_HEADER: &str = "x-wenlan-team-member-name";
/// What is acting: only `person` is accepted.
pub(crate) const KIND_HEADER: &str = "x-wenlan-team-member-kind";

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

/// The member these headers name, when they carry this process's secret.
pub(crate) fn from_headers(headers: &HeaderMap) -> Option<ActingMember> {
    let given = headers.get(SECRET_HEADER)?;
    if !same(given.as_bytes(), secret().as_bytes()) {
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
    let name = headers
        .get(MEMBER_NAME_HEADER)
        .and_then(|value| value.to_str().ok())
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
    fn a_member_changes_only_the_pinned_space() {
        assert!(in_pinned_space(Some("Alpha"), Some("Alpha")));
        assert!(!in_pinned_space(Some("Beta"), Some("Alpha")));
        assert!(!in_pinned_space(None, Some("Alpha")));
        assert!(!in_pinned_space(Some("Alpha"), None));
    }
}
