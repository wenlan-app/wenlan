// SPDX-License-Identifier: Apache-2.0
//! Who counts as the person: the app's marker, believed only with the key.
//!
//! A request is the person's when it carries the person marker
//! (`wenlan_types::requests::CALLER_HEADER`), the person key
//! (`wenlan_types::requests::PERSON_KEY_HEADER`) that this daemon wrote to
//! its data root (`wenlan_core::person_key`), and no agent name. Both are
//! required: the marker says "the person did this with their own gesture",
//! which the app sends only on those gestures; the key says the request
//! comes from the app, the `wenlan` command or the page `wenlan open`
//! opened, not from any program that wrote one header.
//!
//! [`gate`] sits in front of every route. It always takes the key header
//! off the request, so no handler, log or forwarded call ever sees it, and
//! takes the marker off too unless the key matched. A request with the
//! marker but no key, or the wrong one, therefore reaches the handlers as an
//! unnamed caller, exactly as a request without the marker always has:
//! old clients keep working, and the routes only the person may use answer
//! with their usual refusal. A call the team gate forwards for a member
//! never counts as the person, key or not.
//!
//! The browser page never gets the key. `wenlan open` asks for a one-time
//! pass (`POST /api/person/browser-pass`, person only) and opens `/?pass=…`;
//! the daemon forgets the pass and puts a new browser session token in that
//! one page. The gate believes the token like the key, but it lives only in
//! this process's memory and dies when the daemon stops. The pass works
//! once, for a minute.
//!
//! The limits, said plainly: a program that runs under the same account and
//! reads the key file on purpose can act as the person, and so can a
//! program that listens on Wenlan's address while Wenlan is not running and
//! receives the key from the app or `wenlan`. The key stops programs that
//! only claim to be the person.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::Request,
    http::HeaderMap,
    middleware::Next,
    response::{Json, Response},
};

use crate::error::ServerError;
use crate::route_registry::{post, TrackedRouter};
use crate::state::SharedState;
use crate::team_session::SessionStore;
use wenlan_types::person_key::BrowserPassResponse;
use wenlan_types::requests::{CALLER_HEADER, PERSON_KEY_HEADER};

/// How long a browser pass works.
const PASS_LIFETIME: Duration = Duration::from_secs(60);
/// Passes waiting at once; asking for more drops the oldest.
const MAX_PASSES: usize = 8;

/// The refusal for a pass asked by anyone but the person.
pub(crate) const PASS_NEEDS_THE_PERSON: &str =
    "browser_pass_needs_the_person: only the person can open Wenlan in their browser with \
     their key. Run `wenlan open` in their own terminal";
/// The refusal while the daemon listens beyond this computer.
pub(crate) const PASS_NEEDS_THIS_COMPUTER: &str =
    "browser_pass_needs_this_computer: this Wenlan listens beyond this computer \
     (WENLAN_BIND_ADDR), so no page gets the person's key over it";

/// The key this process believes, set once when the daemon starts.
static DAEMON_KEY: OnceLock<PersonKey> = OnceLock::new();
/// The data root to load the key from again when the first load failed.
static KEY_ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
/// When loading was last tried, so a failing disk is not hit per request.
static LAST_TRY: Mutex<Option<Instant>> = Mutex::new(None);
/// How often a failed load is tried again.
const RETRY_EVERY: Duration = Duration::from_secs(10);

/// Believe `key` from now on in this process. The first key set stays: the
/// daemon sets it once, before it serves, and a test process sets its own.
/// Until a key is set no request counts as the person's.
pub fn install(key: PersonKey) -> &'static PersonKey {
    DAEMON_KEY.get_or_init(|| key)
}

/// Load the key from `data_root` (made there on the first start) and believe
/// it. When that fails, it is tried again on a later request, at most every
/// ten seconds, so a passing disk error does not lock the person out until
/// the next restart. Logs a plain warning, never the key.
pub fn install_from(data_root: &std::path::Path) {
    let _ = KEY_ROOT.set(data_root.to_path_buf());
    if load_from_root().is_none() {
        tracing::warn!(
            "the person key could not be read or written; no request counts as the person's \
             until it can (tried again on later requests)"
        );
    }
}

fn load_from_root() -> Option<&'static PersonKey> {
    if let Some(key) = DAEMON_KEY.get() {
        return Some(key);
    }
    let root = KEY_ROOT.get()?;
    {
        let mut last = LAST_TRY.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < RETRY_EVERY) {
            return None;
        }
        *last = Some(Instant::now());
    }
    let key = PersonKey::load_or_create(root).ok()?;
    if let Some(warning) = wenlan_core::person_key::readable_by_others(root) {
        tracing::warn!("{warning}");
    }
    Some(install(key))
}

/// The key this process believes, if one was set (loading it again when
/// the first try failed).
pub(crate) fn installed() -> Option<&'static PersonKey> {
    DAEMON_KEY.get().or_else(load_from_root)
}

/// Whose session a browser pass becomes in the session store.
const SESSION_OWNER: &str = "person";

/// The key this daemon believes, the browser passes it handed out, and the
/// browser sessions those passes became.
#[derive(Clone)]
pub struct PersonKey {
    key: Arc<str>,
    passes: Arc<Mutex<Vec<(String, Instant)>>>,
    /// Browser sessions, in the team listener's session store: held only
    /// in this process's memory so they die when the daemon stops, each
    /// token kept as its SHA-256, the least recently used dropped at the
    /// store's cap. A page holds a token, never the key.
    sessions: Arc<SessionStore>,
}

impl std::fmt::Debug for PersonKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key itself, in a log or a panic message.
        f.write_str("PersonKey(..)")
    }
}

impl PersonKey {
    /// The key in `data_root`, made there on the first start.
    pub fn load_or_create(data_root: &std::path::Path) -> std::io::Result<Self> {
        wenlan_core::person_key::ensure(data_root).map(Self::new)
    }

    /// A key given directly (tests, and a daemon that already read it).
    pub fn new(key: String) -> Self {
        Self {
            key: key.into(),
            passes: Arc::default(),
            sessions: Arc::default(),
        }
    }

    /// Whether `given` is this key, or a browser session this process
    /// handed out.
    pub(crate) fn believes(&self, given: &[u8]) -> bool {
        if wenlan_core::person_key::matches(&self.key, given) {
            return true;
        }
        std::str::from_utf8(given)
            .ok()
            .and_then(|token| self.sessions.lookup(token))
            .is_some_and(|who| who == SESSION_OWNER)
    }

    /// A new one-time browser pass.
    pub(crate) fn new_browser_pass(&self) -> Result<String, ServerError> {
        let pass = wenlan_core::person_key::new_key()
            .map_err(|_| ServerError::Internal("no random bytes for a browser pass".into()))?;
        let mut passes = self.passes.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        passes.retain(|(_, made)| now.duration_since(*made) < PASS_LIFETIME);
        if passes.len() >= MAX_PASSES {
            passes.remove(0);
        }
        passes.push((pass.clone(), now));
        Ok(pass)
    }

    /// For the one page that brings a pass still unused and in time: a new
    /// browser session token, which the gate believes like the key until the
    /// daemon stops. The pass is gone afterwards, used or late. The page
    /// never gets the key itself.
    pub(crate) fn redeem_browser_pass(&self, pass: &str) -> Option<String> {
        {
            let mut passes = self.passes.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            passes.retain(|(_, made)| now.duration_since(*made) < PASS_LIFETIME);
            let found = passes
                .iter()
                .position(|(held, _)| wenlan_core::person_key::matches(held, pass.as_bytes()))?;
            passes.remove(found);
        }
        self.sessions.create(SESSION_OWNER).ok()
    }
}

/// Believe the person marker only next to the right key (see the module
/// note). Runs before every handler.
pub(crate) async fn gate(mut request: Request, next: Next) -> Response {
    sift(installed(), request.headers_mut());
    next.run(request).await
}

/// Take the key off `headers`, and the person marker too unless the key is
/// `key`. What [`gate`] does, for any key.
pub(crate) fn sift(key: Option<&PersonKey>, headers: &mut HeaderMap) {
    let given = headers.remove(PERSON_KEY_HEADER);
    // A member's call through the team gate is never the person's.
    let forwarded = headers.contains_key(crate::team_actor::SECRET_HEADER);
    let believed = !forwarded
        && matches!((key, &given), (Some(key), Some(given)) if key.believes(given.as_bytes()));
    if !believed {
        headers.remove(CALLER_HEADER);
    }
}

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route("/api/person/browser-pass", post(handle_browser_pass))
}

/// POST /api/person/browser-pass — a one-time pass for `wenlan open`.
pub async fn handle_browser_pass(
    headers: HeaderMap,
) -> Result<Json<BrowserPassResponse>, ServerError> {
    if !matches!(
        crate::security::bind_scope_from_env(),
        crate::security::BindScope::Unset | crate::security::BindScope::Loopback
    ) {
        return Err(ServerError::ValidationError(
            PASS_NEEDS_THIS_COMPUTER.into(),
        ));
    }
    if !crate::memory_routes::caller_is_person(&headers) {
        return Err(ServerError::ValidationError(PASS_NEEDS_THE_PERSON.into()));
    }
    // The marker survived the gate, so a key is set; refuse all the same
    // if not.
    let key =
        installed().ok_or_else(|| ServerError::ValidationError(PASS_NEEDS_THE_PERSON.into()))?;
    Ok(Json(BrowserPassResponse {
        pass: key.new_browser_pass()?,
    }))
}

/// The key every test in this crate's own test build believes, set on first
/// use. Tests that call a route through the router as the person send it.
#[cfg(test)]
pub(crate) fn test_key() -> &'static str {
    const TEST_KEY: &str = "7e577e577e577e577e577e577e577e577e577e577e577e577e577e577e577e57";
    &install(PersonKey::new(TEST_KEY.to_string())).key
}

#[cfg(test)]
#[path = "person_key_test.rs"]
mod person_key_test;
