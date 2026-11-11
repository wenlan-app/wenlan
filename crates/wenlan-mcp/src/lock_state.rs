//! Read `WENLAN_SPACE` at startup; expose the value as the "locked space".
//!
//! When set, every outbound daemon call attaches `X-Origin-Space: <value>`,
//! the MCP tool handlers ignore any inbound `space` arg from the model, and
//! the schema-gating layer (later wired in tools.rs) omits the `space`
//! field from tool definitions.
//! Exact `read_source` reads retain a required `space` argument and reject a
//! mismatch with this lock instead of silently overriding the caller's intent.
//!
//! Implementation uses `RwLock<Option<String>>` rather than `OnceLock` so that
//! the test suite can reset state between cases without spawning separate
//! processes.

use std::sync::RwLock;

static LOCKED: RwLock<Option<String>> = RwLock::new(None);
static DEFAULT: RwLock<Option<String>> = RwLock::new(None);

tokio::task_local! {
    /// A strict Space pin for one MCP connection, set while it handles a
    /// call: a team server serves every member's connection from one
    /// process, each pinned to the Space it was opened for. It wins over the
    /// process-wide `WENLAN_SPACE` and turns off `WENLAN_DEFAULT_SPACE`.
    static SESSION_PIN: String;
}

/// Runs `work` with `pin` as the strict Space pin (see [`SESSION_PIN`]);
/// `None` runs it under the process-wide pin as before.
pub async fn with_session_pin<F: std::future::Future>(pin: Option<String>, work: F) -> F::Output {
    match pin {
        Some(pin) => SESSION_PIN.scope(pin, work).await,
        None => work.await,
    }
}

fn session_pin() -> Option<String> {
    SESSION_PIN.try_with(Clone::clone).ok()
}

/// Initialise from the environment. Call once at process startup before
/// accepting any requests. Subsequent calls overwrite the value, which is
/// intentional for test isolation.
pub fn init_from_env() {
    let value = std::env::var("WENLAN_SPACE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    *LOCKED.write().expect("lock_state write lock poisoned") = value;
    let default = std::env::var("WENLAN_DEFAULT_SPACE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    *DEFAULT.write().expect("default_state write lock poisoned") = default;
}

/// Return the locked space slug, or `None` if `WENLAN_SPACE` was not set.
pub fn locked_space() -> Option<String> {
    if let Some(pin) = session_pin() {
        return Some(pin);
    }
    LOCKED
        .read()
        .expect("lock_state read lock poisoned")
        .clone()
}

/// Return the overridable process fallback from `WENLAN_DEFAULT_SPACE`.
pub fn default_space() -> Option<String> {
    if session_pin().is_some() {
        return None;
    }
    DEFAULT
        .read()
        .expect("default_state read lock poisoned")
        .clone()
}

/// Drop the `WENLAN_DEFAULT_SPACE` fallback for this process. Whole-library
/// serving calls this so an inherited default can never silently narrow
/// searches to one Space.
pub fn clear_default_space() {
    *DEFAULT.write().expect("default_state write lock poisoned") = None;
}

/// Return `true` if a space lock is active.
pub fn is_locked() -> bool {
    if session_pin().is_some() {
        return true;
    }
    LOCKED
        .read()
        .expect("lock_state read lock poisoned")
        .is_some()
}

/// Shared mutex for test modules that mutate `WENLAN_SPACE` env var + the
/// `LOCKED` RwLock. Exposed as `pub(crate)` so `client::tests` can share the
/// same serialisation lock and avoid races between test threads.
#[cfg(test)]
pub(crate) static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_space_returns_none_when_env_unset() {
        let _guard = ENV_LOCK.blocking_lock();
        std::env::remove_var("WENLAN_SPACE");
        init_from_env();
        assert_eq!(locked_space(), None);
        assert!(!is_locked());
    }

    #[test]
    fn locked_space_returns_value_when_env_set() {
        let _guard = ENV_LOCK.blocking_lock();
        std::env::set_var("WENLAN_SPACE", "work");
        init_from_env();
        assert_eq!(locked_space().as_deref(), Some("work"));
        assert!(is_locked());
        // Clean up.
        std::env::remove_var("WENLAN_SPACE");
        init_from_env();
    }

    #[test]
    fn whitespace_only_value_treated_as_unset() {
        let _guard = ENV_LOCK.blocking_lock();
        std::env::set_var("WENLAN_SPACE", "   ");
        init_from_env();
        assert_eq!(locked_space(), None);
        std::env::remove_var("WENLAN_SPACE");
        init_from_env();
    }

    #[test]
    fn clear_default_space_drops_only_the_fallback() {
        let _guard = ENV_LOCK.blocking_lock();
        std::env::remove_var("WENLAN_SPACE");
        std::env::set_var("WENLAN_DEFAULT_SPACE", "fallback");
        init_from_env();
        clear_default_space();
        assert_eq!(default_space(), None);
        assert!(!is_locked());
        std::env::remove_var("WENLAN_DEFAULT_SPACE");
        init_from_env();
    }

    #[test]
    fn default_space_is_overridable_not_locked() {
        let _guard = ENV_LOCK.blocking_lock();
        std::env::remove_var("WENLAN_SPACE");
        std::env::set_var("WENLAN_DEFAULT_SPACE", "fallback");
        init_from_env();
        assert_eq!(default_space().as_deref(), Some("fallback"));
        assert!(!is_locked());
        std::env::remove_var("WENLAN_DEFAULT_SPACE");
        init_from_env();
    }
}
