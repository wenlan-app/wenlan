// SPDX-License-Identifier: AGPL-3.0-only
//! Browser sessions on the team listener: kept in memory only, so a server
//! restart signs everyone out.
//!
//! A session is a random 32-byte token handed to the browser as a cookie.
//! Only the token's SHA-256 is kept here, next to the member's id and two
//! times. A session ends when it has gone unused for [`IDLE`], or lives
//! [`ABSOLUTE`] from its creation, whichever comes first; an ended session
//! is dropped when it is next looked up or when another is made.
//!
//! The number of live sessions is capped, per member ([`PER_MEMBER_CAP`])
//! and in all ([`TOTAL_CAP`]). At a cap the session least recently used
//! (within that member, or overall) is evicted, and the new one is made:
//! refusing instead would let anyone able to sign in shut the real members
//! out of signing in until the old sessions expire, whereas eviction costs
//! the least recently used browser one sign-in. The per-member cap keeps
//! one member's logins from evicting everybody else's.
//!
//! The store holds a `std::sync::Mutex`, never held across an `.await`.
//!
//! A session also remembers when its member last signed in again at the
//! identity provider ([`Session::reauthed`]); [`SessionStore::fresh`] is
//! what a route that issues a long-lived credential asks first.
//!
//! The local listener's browser sessions (`person_key`, made from a
//! `wenlan open` pass) are kept in a store of their own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{header, HeaderMap};
use sha2::{Digest, Sha256};

/// The session cookie's name. `__Host-` makes a browser refuse it unless it
/// is `Secure`, has `Path=/` and has no `Domain`.
pub(crate) const COOKIE_NAME: &str = "__Host-wenlan-session";

/// A session unused for this long ends.
pub(crate) const IDLE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A session ends this long after it was made, however often it is used.
pub(crate) const ABSOLUTE: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// Live sessions of one member.
pub(crate) const PER_MEMBER_CAP: usize = 50;
/// Live sessions in all.
pub(crate) const TOTAL_CAP: usize = 10_000;
/// How long after signing in again at the provider a session may still
/// make a long-lived credential. A stolen cookie can use no more than this.
pub(crate) const REAUTH_WINDOW: Duration = Duration::from_secs(5 * 60);

type Hash = [u8; 32];

struct Session {
    member_id: String,
    created: Instant,
    last_used: Instant,
    /// When the member last proved themselves at the provider with an
    /// account already linked to them; `None` for a session made without.
    reauthed: Option<Instant>,
}

impl Session {
    fn live(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created) < ABSOLUTE
            && now.saturating_duration_since(self.last_used) < IDLE
    }
}

pub(crate) struct SessionStore {
    sessions: Mutex<HashMap<Hash, Session>>,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::with_clock(Arc::new(Instant::now))
    }
}

fn hash(token: &str) -> Hash {
    Sha256::digest(token.as_bytes()).into()
}

impl SessionStore {
    /// A store reading time from `clock` (tests move it).
    pub(crate) fn with_clock(clock: Arc<dyn Fn() -> Instant + Send + Sync>) -> Self {
        Self {
            sessions: Mutex::default(),
            clock,
        }
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<Hash, Session>> {
        // A panic while holding the lock leaves a map that is still valid.
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Makes a session for `member_id` and returns its token (64 hex
    /// characters), which is not kept. It starts without a fresh sign-in.
    pub(crate) fn create(&self, member_id: &str) -> Result<String, getrandom::Error> {
        self.create_with(member_id, false)
    }

    /// As [`SessionStore::create`], for a member who has just signed in at
    /// the provider: the session counts as freshly signed in.
    pub(crate) fn create_fresh(&self, member_id: &str) -> Result<String, getrandom::Error> {
        self.create_with(member_id, true)
    }

    fn create_with(&self, member_id: &str, fresh: bool) -> Result<String, getrandom::Error> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)?;
        let token = hex::encode(bytes);
        let now = (self.clock)();
        let mut sessions = self.sessions();
        sessions.retain(|_, session| session.live(now));
        evict_to(&mut sessions, |s| s.member_id == member_id, PER_MEMBER_CAP);
        evict_to(&mut sessions, |_| true, TOTAL_CAP);
        sessions.insert(
            hash(&token),
            Session {
                member_id: member_id.to_string(),
                created: now,
                last_used: now,
                reauthed: fresh.then_some(now),
            },
        );
        Ok(token)
    }

    /// The member this token's session belongs to, if it is live; the
    /// session's last use is now. An ended session is dropped.
    pub(crate) fn lookup(&self, token: &str) -> Option<String> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let key = hash(token);
        let now = (self.clock)();
        let mut sessions = self.sessions();
        let session = sessions.get_mut(&key)?;
        if !session.live(now) {
            sessions.remove(&key);
            return None;
        }
        session.last_used = now;
        Some(session.member_id.clone())
    }

    /// Whether this token's live session signed in at the provider within
    /// `window`. Asking does not count as use.
    pub(crate) fn fresh(&self, token: &str, window: Duration) -> bool {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        let now = (self.clock)();
        self.sessions().get(&hash(token)).is_some_and(|session| {
            session.live(now)
                && session
                    .reauthed
                    .is_some_and(|at| now.saturating_duration_since(at) < window)
        })
    }

    /// Ends this token's session.
    pub(crate) fn delete(&self, token: &str) {
        self.sessions().remove(&hash(token));
    }

    /// Ends every session of `member_id`.
    pub(crate) fn delete_member(&self, member_id: &str) {
        self.sessions()
            .retain(|_, session| session.member_id != member_id);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sessions().len()
    }
}

/// Evicts the least recently used of the sessions `scope` selects until
/// fewer than `cap` of them remain, leaving room for one more.
fn evict_to(sessions: &mut HashMap<Hash, Session>, scope: impl Fn(&Session) -> bool, cap: usize) {
    let mut inside: Vec<(Instant, Hash)> = sessions
        .iter()
        .filter(|(_, session)| scope(session))
        .map(|(key, session)| (session.last_used, *key))
        .collect();
    if inside.len() < cap {
        return;
    }
    inside.sort();
    for (_, key) in &inside[..inside.len() + 1 - cap] {
        sessions.remove(key);
    }
}

/// The attributes every session cookie carries: no `Domain` (so the cookie
/// is this host's alone), `Path=/`, never readable by a script, https only,
/// and not sent on a cross-site subrequest.
const ATTRIBUTES: &str = "HttpOnly; Secure; SameSite=Lax; Path=/";

/// The `Set-Cookie` value that gives the browser `token`.
pub(crate) fn set_cookie(token: &str) -> String {
    format!(
        "{COOKIE_NAME}={token}; {ATTRIBUTES}; Max-Age={}",
        ABSOLUTE.as_secs()
    )
}

/// The `Set-Cookie` value that removes the cookie: the same name and
/// attributes, empty, already expired.
pub(crate) fn clear_cookie() -> String {
    format!("{COOKIE_NAME}=; {ATTRIBUTES}; Max-Age=0")
}

/// The session token the request's `Cookie` headers carry. `None` when
/// there is none, or when the cookie appears twice (which one a browser
/// meant is then unknown).
pub(crate) fn cookie_token(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, COOKIE_NAME)
}

/// The value of the cookie `name`: `None` when the request has none, or
/// has it twice.
pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for pair in value.to_str().ok()?.split(';') {
            if let Some((n, token)) = pair.split_once('=') {
                if n.trim() == name {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(token.trim().to_string());
                }
            }
        }
    }
    found
}

/// Whether the request carries the session cookie at all.
pub(crate) fn has_cookie(headers: &HeaderMap) -> bool {
    headers.get_all(header::COOKIE).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(';').any(|pair| {
                pair.split_once('=')
                    .is_some_and(|(n, _)| n.trim() == COOKIE_NAME)
            })
        })
    })
}

#[cfg(test)]
#[path = "team_session_test.rs"]
mod tests;
