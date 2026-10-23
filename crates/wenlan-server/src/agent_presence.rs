// SPDX-License-Identifier: Apache-2.0
//! Best-effort "this agent was seen" signal for the search read routes.
//!
//! Search carries an agent name but never wrote, so a tool that only reads
//! stayed invisible to the Connections list. `note_agent_read` records the
//! sighting through the one DB writer (`MemoryDB::record_agent_presence`),
//! off the response path, at most once per agent per `PRESENCE_WINDOW`.
//!
//! A read never registers or touches an agent: the row it creates is
//! `"unknown"` (search only) and the first write promotes it. See
//! `wenlan_core::db` `agent_presence` for the trust rules.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use wenlan_core::db::{presence_agent_id, AgentPresence, MemoryDB};
use wenlan_core::error::WenlanError;

/// One DB write per agent per window, however often it searches.
pub const PRESENCE_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Distinct agent names the throttle remembers. The header is unauthenticated
/// free text, so the map must not grow with it.
pub const MAX_TRACKED_AGENTS: usize = 256;

/// Remembers when each canonical agent id last had a presence write.
///
/// A plain `std::sync::Mutex`: held only for a map lookup, never across an
/// `.await`.
#[derive(Debug, Default)]
pub struct AgentPresenceThrottle {
    last_recorded: Mutex<HashMap<String, Instant>>,
}

impl AgentPresenceThrottle {
    /// True when `canonical` is due a presence write at `now`; claiming it
    /// starts a new window.
    ///
    /// At the name cap, expired entries are dropped first (they no longer
    /// throttle anything). If the map is still full, a NEW name is refused:
    /// nothing is written for it and every live entry keeps its window. The
    /// alternative, clearing the map, would reopen the window of every other
    /// agent at once, so a burst of made-up header values could turn the
    /// 10-minute throttle into a write per search. A name already in the map
    /// is unaffected and keeps its normal window.
    pub fn claim(&self, canonical: &str, now: Instant) -> bool {
        let mut map = self
            .last_recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match map.get(canonical) {
            Some(last) if now.saturating_duration_since(*last) < PRESENCE_WINDOW => return false,
            Some(_) => {}
            None if map.len() >= MAX_TRACKED_AGENTS => {
                map.retain(|_, last| now.saturating_duration_since(*last) < PRESENCE_WINDOW);
                if map.len() >= MAX_TRACKED_AGENTS {
                    return false;
                }
            }
            None => {}
        }
        map.insert(canonical.to_string(), now);
        true
    }

    /// Give a claim back after a failed write so the next read retries it.
    pub fn release(&self, canonical: &str) {
        self.last_recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(canonical);
    }

    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.last_recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Record that `raw_agent` just made a read request.
///
/// Returns immediately. When the name is valid and its window is open, the DB
/// write runs on a spawned task and its error is logged, never surfaced. The
/// handle is returned only so tests can await the write; handlers drop it.
pub fn note_agent_read(
    db: Arc<MemoryDB>,
    throttle: Arc<AgentPresenceThrottle>,
    raw_agent: &str,
) -> Option<tokio::task::JoinHandle<()>> {
    note_agent_read_at(db, throttle, raw_agent, Instant::now())
}

fn note_agent_read_at(
    db: Arc<MemoryDB>,
    throttle: Arc<AgentPresenceThrottle>,
    raw_agent: &str,
    now: Instant,
) -> Option<tokio::task::JoinHandle<()>> {
    let canonical = presence_agent_id(raw_agent)?;
    if !throttle.claim(&canonical, now) {
        return None;
    }
    let raw_agent = raw_agent.to_string();
    Some(tokio::spawn(async move {
        let outcome = db.record_agent_presence(&raw_agent).await;
        settle(&throttle, &canonical, outcome);
    }))
}

/// Fire the `first-recall` milestone the first time a named AI tool gets
/// something back from a search. The getting-started step "Ask your AI" and
/// its toast read that milestone; nothing fired it before. The app's own
/// searches carry no agent name and never count. Off the response path; a
/// failure is logged, never returned.
pub fn note_agent_recall(
    db: Arc<MemoryDB>,
    raw_agent: &str,
    results: &[wenlan_types::memory::SearchResult],
    pages: Option<&[wenlan_types::memory::SearchResult]>,
) -> Option<tokio::task::JoinHandle<()>> {
    let pages = pages.unwrap_or_default();
    let hits = results.len() + pages.len();
    if hits == 0 {
        return None;
    }
    let canonical = presence_agent_id(raw_agent)?;
    // A page is what the step is about, so it is the one quoted when there is
    // one; otherwise the top memory, char-safe.
    let preview = match pages.first() {
        Some(page) => Some(page.title.clone()),
        None => results.first().map(|hit| {
            let short: String = hit.content.chars().take(100).collect();
            if short.chars().count() < hit.content.chars().count() {
                format!("{}…", short.trim_end())
            } else {
                short
            }
        }),
    };
    Some(tokio::spawn(async move {
        // The toast names the tool the way Connections does ("Claude Code").
        let agent = db
            .get_agent(&canonical)
            .await
            .ok()
            .flatten()
            .and_then(|agent| agent.display_name)
            .unwrap_or_else(|| canonical.clone());
        let evaluator = wenlan_core::onboarding::MilestoneEvaluator::new(
            &db,
            Arc::new(wenlan_core::events::NoopEmitter),
        );
        if let Err(error) = evaluator
            .check_after_context_call(&agent, hits, preview.as_deref())
            .await
        {
            tracing::warn!(agent = %canonical, %error, "first recall not recorded");
        }
    }))
}

/// Register the tool named on a page write (`x-agent-name`) before the write.
///
/// A page write is a write: the tool is registered and promoted as a memory
/// write would be (`MemoryDB::note_page_writer`), so its next recall sees the
/// pages it wrote. A disabled tool is refused with 403, as a memory write is.
/// Any other failure is logged and the write goes ahead: the page matters
/// more than the bookkeeping. No header, or a name a read may not record, does
/// nothing.
pub async fn note_page_writer(
    db: &MemoryDB,
    agent: Option<&str>,
) -> Result<(), crate::error::ServerError> {
    let Some(agent) = agent.map(str::trim).filter(|a| !a.is_empty()) else {
        return Ok(());
    };
    match db.note_page_writer(agent).await {
        Ok(_) => Ok(()),
        Err(error @ WenlanError::AgentDisabled(_)) => Err(error.into()),
        Err(error) => {
            tracing::warn!(%agent, %error, "page writer not registered");
            Ok(())
        }
    }
}

/// Log the outcome of a presence write. An error is never returned to the
/// request; it only gives the throttle claim back so the next read retries.
fn settle(
    throttle: &AgentPresenceThrottle,
    canonical: &str,
    outcome: Result<AgentPresence, WenlanError>,
) {
    match outcome {
        Ok(outcome) => {
            tracing::debug!(agent = %canonical, ?outcome, "agent presence recorded on read");
        }
        Err(error) => {
            tracing::warn!(agent = %canonical, %error, "agent presence not recorded on read");
            throttle.release(canonical);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) async fn test_db() -> (Arc<MemoryDB>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = MemoryDB::new(dir.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap();
        (Arc::new(db), dir)
    }

    #[test]
    fn the_window_holds_then_reopens() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();

        assert!(throttle.claim("claude-code", t0));
        assert!(!throttle.claim("claude-code", t0));
        assert!(!throttle.claim("claude-code", t0 + PRESENCE_WINDOW - Duration::from_secs(1)));
        assert!(throttle.claim("claude-code", t0 + PRESENCE_WINDOW));
        // Another agent has its own window.
        assert!(throttle.claim("cursor", t0 + Duration::from_secs(5)));
    }

    #[test]
    fn a_released_claim_is_retried() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();
        assert!(throttle.claim("codex", t0));

        throttle.release("codex");

        assert!(throttle.claim("codex", t0));
    }

    #[test]
    fn the_map_is_bounded_by_the_name_cap() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();

        for i in 0..MAX_TRACKED_AGENTS {
            assert!(throttle.claim(&format!("agent-{i}"), t0));
        }
        // Past the cap a new name is refused, and the map does not move.
        for i in 0..(MAX_TRACKED_AGENTS * 3) {
            assert!(!throttle.claim(&format!("extra-{i}"), t0));
            assert_eq!(throttle.tracked(), MAX_TRACKED_AGENTS);
        }
    }

    #[test]
    fn a_full_map_refuses_new_names_without_resetting_anyone_elses_window() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();
        assert!(throttle.claim("cursor", t0));
        for i in 1..MAX_TRACKED_AGENTS {
            assert!(throttle.claim(&format!("agent-{i}"), t0));
        }
        assert_eq!(throttle.tracked(), MAX_TRACKED_AGENTS);

        // A new name arrives seconds later: refused, nothing evicted.
        let seconds_later = t0 + Duration::from_secs(5);
        assert!(!throttle.claim("brand-new", seconds_later));

        // The name throttled seconds earlier is still throttled.
        assert!(!throttle.claim("cursor", seconds_later + Duration::from_secs(1)));
        assert_eq!(throttle.tracked(), MAX_TRACKED_AGENTS);

        // A name already in the map keeps its normal window: it reopens on
        // schedule, and once the entries expire the new name is admitted.
        let reopened = t0 + PRESENCE_WINDOW;
        assert!(throttle.claim("cursor", reopened));
        assert!(throttle.claim("brand-new", reopened));
    }

    #[tokio::test]
    async fn a_name_refused_at_the_cap_writes_nothing() {
        let (db, _dir) = test_db().await;
        let throttle = Arc::new(AgentPresenceThrottle::default());
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_AGENTS {
            assert!(throttle.claim(&format!("agent-{i}"), t0));
        }

        let refused = note_agent_read_at(db.clone(), throttle.clone(), "late-comer", t0);

        assert!(refused.is_none());
        assert!(db.get_agent("late-comer").await.unwrap().is_none());
        assert!(db.list_agents().await.unwrap().is_empty());
    }

    #[test]
    fn expired_names_are_evicted_before_the_map_is_cleared() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();
        for i in 0..MAX_TRACKED_AGENTS {
            assert!(throttle.claim(&format!("old-{i}"), t0));
        }
        // One live name, then everything else expires.
        let later = t0 + PRESENCE_WINDOW;
        assert!(throttle.claim("fresh-a", later));
        let still_live = later + Duration::from_secs(1);
        assert!(!throttle.claim("fresh-a", still_live));
        assert_eq!(throttle.tracked(), 1, "expired entries were evicted");
    }

    #[tokio::test]
    async fn invalid_names_spend_no_throttle_slot_and_write_nothing() {
        let (db, _dir) = test_db().await;
        let throttle = Arc::new(AgentPresenceThrottle::default());

        for bad in [
            "",
            "   ",
            "unknown",
            "Unknown",
            "bad\u{0}name",
            "wenlan-setup",
        ] {
            assert!(note_agent_read(db.clone(), throttle.clone(), bad).is_none());
        }

        assert_eq!(throttle.tracked(), 0);
        assert!(db.list_agents().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_read_marks_the_agent_seen_and_the_throttle_holds() {
        let (db, _dir) = test_db().await;
        let throttle = Arc::new(AgentPresenceThrottle::default());
        let t0 = Instant::now();

        let first = note_agent_read_at(db.clone(), throttle.clone(), "Claude Code", t0)
            .expect("first read is due");
        first.await.unwrap();
        let agent = db.get_agent("claude-code").await.unwrap().unwrap();
        assert_eq!(agent.trust_level, "unknown");
        assert!(agent.last_seen_at.is_some());
        assert_eq!(agent.memory_count, 0);

        // Inside the window the second read does not reach the database:
        // deleting the row proves nothing re-creates it.
        db.delete_agent("claude-code").await.unwrap();
        assert!(note_agent_read_at(
            db.clone(),
            throttle.clone(),
            "claude-code",
            t0 + Duration::from_secs(60)
        )
        .is_none());
        assert!(db.get_agent("claude-code").await.unwrap().is_none());

        // After the window it records again.
        note_agent_read_at(
            db.clone(),
            throttle,
            "claude-code",
            t0 + PRESENCE_WINDOW + Duration::from_secs(1),
        )
        .expect("window reopened")
        .await
        .unwrap();
        assert!(db.get_agent("claude-code").await.unwrap().is_some());
    }

    #[test]
    fn a_failed_write_gives_its_claim_back_so_the_next_read_retries() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();
        assert!(throttle.claim("cursor", t0));

        settle(
            &throttle,
            "cursor",
            Err(WenlanError::VectorDb("database is locked".into())),
        );

        assert!(throttle.claim("cursor", t0));
    }

    #[test]
    fn a_successful_write_keeps_its_window() {
        let throttle = AgentPresenceThrottle::default();
        let t0 = Instant::now();
        assert!(throttle.claim("cursor", t0));

        settle(&throttle, "cursor", Ok(AgentPresence::Created));

        assert!(!throttle.claim("cursor", t0));
    }
}

/// The routes that mark an agent seen, and the ones that must not.
#[cfg(test)]
mod route_tests {
    use super::tests::test_db;
    use super::*;
    use crate::{router::AppRouter, state::ServerState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    async fn app_with_db() -> (AppRouter, Arc<MemoryDB>, tempfile::TempDir) {
        let (db, dir) = test_db().await;
        let state = ServerState {
            db: Some(db.clone()),
            ..Default::default()
        };
        let app = crate::router::build_router(Arc::new(RwLock::new(state)));
        (app, db, dir)
    }

    async fn post(app: &AppRouter, uri: &str, agent: Option<&str>, body: &str) -> StatusCode {
        let mut request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(agent) = agent {
            request = request.header("x-agent-name", agent);
        }
        app.clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// The presence write runs on a spawned task, so a positive check waits
    /// for it (bounded).
    async fn wait_for_agent(db: &MemoryDB, name: &str) -> Option<wenlan_types::AgentConnection> {
        for _ in 0..100 {
            if let Some(agent) = db.get_agent(name).await.unwrap() {
                return Some(agent);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }

    async fn agents_over_http(app: &AppRouter) -> Vec<wenlan_types::responses::AgentResponse> {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1_048_576)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).expect("parse agents list")
    }

    #[tokio::test]
    async fn memory_search_with_an_agent_name_marks_it_seen_without_write_trust() {
        let (app, db, _dir) = app_with_db().await;

        let status = post(
            &app,
            "/api/memory/search",
            Some("Route Reader"),
            r#"{"query":"hello"}"#,
        )
        .await;

        assert!(status.is_success(), "search should succeed, got {status}");
        let agent = wait_for_agent(&db, "route-reader")
            .await
            .expect("the read must mark the agent seen");
        assert_eq!(agent.trust_level, "unknown");
        assert!(agent.last_seen_at.is_some());
        assert_eq!(agent.memory_count, 0);
        let listed = agents_over_http(&app).await;
        let over_http = listed
            .iter()
            .find(|a| a.name == "route-reader")
            .expect("GET /api/agents must list it");
        assert_eq!(over_http.trust_level, "unknown");
        assert!(over_http.last_seen_at.is_some());
    }

    #[tokio::test]
    async fn memory_search_with_a_body_source_agent_marks_it_seen() {
        let (app, db, _dir) = app_with_db().await;

        let status = post(
            &app,
            "/api/memory/search",
            None,
            r#"{"query":"hello","source_agent":"body-reader"}"#,
        )
        .await;

        assert!(status.is_success(), "search should succeed, got {status}");
        assert!(wait_for_agent(&db, "body-reader").await.is_some());
    }

    #[tokio::test]
    async fn api_search_with_an_agent_name_marks_it_seen() {
        let (app, db, _dir) = app_with_db().await;

        let status = post(
            &app,
            "/api/search",
            Some("api-search-reader"),
            r#"{"query":"hello"}"#,
        )
        .await;

        assert!(status.is_success(), "search should succeed, got {status}");
        let agent = wait_for_agent(&db, "api-search-reader")
            .await
            .expect("the read must mark the agent seen");
        assert_eq!(agent.trust_level, "unknown");
        assert_eq!(agent.memory_count, 0);
    }

    #[tokio::test]
    async fn a_search_without_a_usable_agent_name_creates_nothing() {
        let (app, db, _dir) = app_with_db().await;

        for uri in ["/api/memory/search", "/api/search"] {
            assert!(post(&app, uri, None, r#"{"query":"hello"}"#)
                .await
                .is_success());
            // A blank header value resolves to the same "unknown" sentinel.
            assert!(post(&app, uri, Some("   "), r#"{"query":"hello"}"#)
                .await
                .is_success());
            assert!(post(&app, uri, Some("Unknown"), r#"{"query":"hello"}"#)
                .await
                .is_success());
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(db.list_agents().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn brief_and_context_stay_read_only() {
        let (app, db, _dir) = app_with_db().await;

        let brief = post(&app, "/api/brief", Some("brief-reader"), "{}").await;
        let context = post(&app, "/api/context", Some("context-reader"), r#"{}"#).await;

        assert!(brief.is_success(), "brief should succeed, got {brief}");
        assert!(
            context.is_success(),
            "context should succeed, got {context}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            db.list_agents().await.unwrap().is_empty(),
            "brief and context must never create an agent row"
        );
    }

    #[tokio::test]
    async fn a_repeat_search_inside_the_window_does_not_reach_the_database() {
        let (app, db, _dir) = app_with_db().await;
        post(
            &app,
            "/api/memory/search",
            Some("steady-reader"),
            r#"{"query":"hello"}"#,
        )
        .await;
        wait_for_agent(&db, "steady-reader").await.unwrap();

        // Deleting the row proves a second search inside the window does not
        // write again.
        db.delete_agent("steady-reader").await.unwrap();
        post(
            &app,
            "/api/memory/search",
            Some("steady-reader"),
            r#"{"query":"hello again"}"#,
        )
        .await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(db.get_agent("steady-reader").await.unwrap().is_none());
    }
}
