// SPDX-License-Identifier: AGPL-3.0-only
//! HTTP client for the Wenlan daemon (wenlan-server).
//!
//! Thin wrapper around `reqwest::Client` that maps each daemon endpoint
//! to a typed method. The Tauri app uses this instead of direct DB access.

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use wenlan_types::responses::HealthResponse;

/// A wedged daemon (accept backlog full) otherwise leaves connects hanging;
/// reqwest's default client has no timeouts at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Generous total backstop: a single ingest of a book-sized document
/// legitimately chunks + embeds synchronously before responding.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// The startup health loop polls a daemon that may accept connections but
/// never respond (wedged, or bound-but-still-initializing); the probe must
/// not inherit the ingest-sized backstop above.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
/// Chat-export import only: a measured 10000-message synthetic import took
/// 625.2s server-side (5000 embedded rows), so the 600s shared default fires
/// while the detached daemon still completes. Eight hours bounds the wait
/// without touching the shared client, health, or setup timeouts.
const CHAT_EXPORT_IMPORT_TIMEOUT: Duration = Duration::from_secs(8 * 60 * 60);

/// The on-device model download request stays open until the whole model is
/// fetched and loaded. The largest catalog model is 5.5 GB (the default is
/// 2.7 GB), which at a slow 2 Mbps takes about 6.1 hours of transfer alone
/// (5.5 GB x 8 bits / 2 Mbps = 22,000 s), far past [`REQUEST_TIMEOUT`]. Eight
/// hours leaves room for that plus engine init and a link that dips below
/// 2 Mbps. Timing out early rejects the wizard's download even though the
/// daemon may still be fetching.
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(8 * 60 * 60);

fn build_http_client(connect_timeout: Duration, request_timeout: Duration) -> Client {
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .expect("static reqwest client config cannot fail")
}

/// HTTP client that proxies requests to the Wenlan daemon.
#[derive(Clone)]
pub struct WenlanClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageUpdateRequestError {
    TransportOrDecode(String),
    Http { status: u16, body: String },
}

#[derive(Debug)]
pub(crate) enum RepairRuntimeRequestError {
    Uncertain(String),
    Http { status: u16, body: String },
}

impl RepairRuntimeRequestError {
    pub(crate) fn definitive_refusal(&self, earlier_uncertain: bool) -> bool {
        let Self::Http { status, body } = self else {
            return false;
        };
        // A retry can encounter a prior accepted request while it drains.
        // Busy is also ambiguous after a lost response: the earlier request
        // could still hold the provisional resumption seal.
        #[derive(Deserialize)]
        struct Refusal {
            error: String,
        }
        let Ok(refusal) = serde_json::from_str::<Refusal>(body) else {
            return false;
        };
        (400..500).contains(status)
            && refusal.error != "repair_runtime_shutting_down"
            && !(earlier_uncertain && refusal.error == "repair_background_writer_busy")
    }
}

impl std::fmt::Display for RepairRuntimeRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uncertain(message) => f.write_str(message),
            Self::Http { status, body } => write!(f, "repair runtime returned {status}: {body}"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SetupStatusResponse {
    pub setup_completed: bool,
    pub mode: String,
    pub anthropic_key_configured: bool,
    pub local_model_selected: Option<String>,
    pub local_model_loaded: Option<String>,
    pub local_model_cached: bool,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
struct CaptureStatsResponse {
    total_chunks: u64,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct TagInventoryResponse {
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) document_tags: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct OnDeviceModelEntry {
    pub id: String,
    pub display_name: String,
    pub param_count: String,
    pub ram_required_gb: f64,
    pub file_size_gb: f64,
    pub cached: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct OnDeviceModelResponse {
    pub loaded: Option<String>,
    pub selected: Option<String>,
    pub models: Vec<OnDeviceModelEntry>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct MoveSpaceResponse {
    pub affected: usize,
}

// ── Resolved routing (daemon ≥ PR #357; GET /api/config/routing) ─────────
// Local DTOs, NOT bumped into wenlan-types (the endpoint is unreleased). Shapes
// mirror `wenlan-server::config_routes` exactly; no `deny_unknown_fields` so a
// newer daemon can add fields without breaking this old app. Never carries key
// material.

/// Resolved route for one job class: which source serves it, the model, and how
/// it was chosen. `mode`: "pinned" | "pinned_unavailable" | "auto". `source`
/// (everyday): "anthropic"|"external"|"on_device"|"basic"; (synthesis):
/// "anthropic"|"external"|"on_device"|"none". `pin`: the raw configured source
/// pin, or `None` when unpinned — distinct from `source` (the RESOLVED
/// source): on a `pinned_unavailable` result the two differ, letting the app say
/// "Pinned to X — using Y". `#[serde(default)]` so a daemon that predates the
/// field (pre-#357) still deserializes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobRoute {
    pub source: String,
    pub model: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub pin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicPool {
    pub configured: bool,
    pub everyday_model: Option<String>,
    pub synthesis_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalPool {
    pub endpoint: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnDevicePool {
    pub selected: Option<String>,
    pub loaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutingPool {
    pub anthropic: AnthropicPool,
    pub external: Option<ExternalPool>,
    pub on_device: Option<OnDevicePool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedRouting {
    pub everyday: JobRoute,
    pub synthesis: JobRoute,
    pub pool: RoutingPool,
}

#[derive(Debug, Clone, Serialize)]
struct DistillReviewRequest {}

#[derive(Debug, Clone, Serialize)]
struct PageRedistillRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DistillReviewResponse {
    pub pages_created: usize,
    pub scoped: bool,
    pub created_ids: Vec<String>,
    pub pending: Vec<DistillPendingCluster>,
    pub stale_pages: Vec<DistillStalePage>,
    pub stale_truncated: bool,
    pub orphan_topics: Vec<DistillOrphanTopic>,
    #[serde(default)]
    pub clusters_found: Option<usize>,
    #[serde(default)]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DistillPendingCluster {
    pub source_ids: Vec<String>,
    pub contents: Vec<String>,
    pub entity_id: Option<String>,
    pub entity_name: Option<String>,
    pub space: Option<String>,
    pub estimated_tokens: usize,
    pub centroid_embedding: Option<Vec<f32>>,
    pub existing_page_id: Option<String>,
    pub existing_page_title: Option<String>,
    pub new_memory_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DistillStalePage {
    pub page_id: String,
    pub title: String,
    pub summary: Option<String>,
    pub source_memory_ids: Vec<String>,
    pub sources_updated_count: Option<usize>,
    pub stale_reason: Option<String>,
    pub user_edited: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DistillOrphanTopic {
    pub label: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PageRedistillResponse {
    pub status: String,
    pub updated: bool,
    pub hint: Option<String>,
    /// Present when `updated == false` because the daemon discarded the
    /// rebuild (today: the citation verification gate), so the UI can say why
    /// instead of "already up to date".
    pub reason: Option<String>,
    /// "Update from sources" on a note that relies on other notes stages a
    /// revision card instead of writing: `gated` with the card's id.
    #[serde(default)]
    pub gated: bool,
    #[serde(default)]
    pub revision_card_id: Option<String>,
    /// The note already says what its sources say; nothing was staged.
    #[serde(default)]
    pub unchanged: bool,
}

/// What became of "confirm no impact" on a basis. A refusal is an answer, not
/// an error: the basis moved on and the person is asked to look again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BasisConfirmResult {
    Confirmed {
        basis: wenlan_types::page_basis::BasisRef,
    },
    Conflict {
        message: String,
    },
}

/// The comparison for a basis, or the reason there is none to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BasisDiffResult {
    Available {
        diff: wenlan_types::page_basis::BasisDiffResponse,
    },
    Unavailable {
        reason: String,
        message: String,
    },
}

/// The `error` text of a daemon error envelope, or the body as sent.
fn daemon_error_text(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str().map(str::to_owned)))
        .unwrap_or_else(|| body.trim().to_string())
}

#[derive(Debug, Clone, serde::Serialize)]
struct OnDeviceModelRequest {
    model_id: String,
}

// ── Communities (M6 cartography) ────────────────────────────────────────
// Wire shapes hand-mirrored from `crates/wenlan-types/src/communities.rs`
// (`community-read-v1`). This section deliberately TYPES the response, unlike
// the page-map section below, which forwards `serde_json::Value` untouched:
// the community read is classified here in Rust-adjacent code and by
// src/lib/graph/community.ts, and a field-level mismatch has to fail where a
// test can see it.
//
// The types are hand-mirrored rather than taken from `wenlan_types` (which
// the app now depends on directly, so the module IS reachable) because
// `CommunityReadScope` is an internally tagged enum: a `kind` the app has not
// been taught would fail the WHOLE response to deserialize, and the response
// is a read the UI must degrade on rather than lose. `kind: String` here
// accepts a future variant and lets the frontend decide. Everything else is
// read-tolerant for the same reason: no `deny_unknown_fields`, so additions
// are ignored rather than rejected. `scope` is carried (not ignored) — these
// structs are re-serialized verbatim for the frontend by the Tauri commands
// below, so a field this struct doesn't declare doesn't just go unread here,
// it's dropped from what the frontend ever sees.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunityScope {
    pub kind: String,
    /// Present only on the `space` variant. The daemon's `CommunityReadScope`
    /// is an internally tagged enum, so `global` and `uncategorized` serialize
    /// as `{"kind":"global"}` with no `name` at all — a required String here
    /// would fail the whole read for those scopes, and every space is then
    /// stuck reporting `partial-error`. A Space is resolved as
    /// `uncategorized` whenever an entity carries that literal as its space or
    /// domain and no Space by that name is registered, which is exactly how
    /// the Atlas enumerates spaces, so this is reachable and not theoretical.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunitySummary {
    pub community_id: String,
    pub space: String,
    pub display_name: Option<String>,
    pub member_count: u64,
    pub published_generation: i64,
    pub algo_version: String,
    pub projection_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunityListResponse {
    pub schema_version: String,
    #[serde(default)]
    pub scope: Option<CommunityScope>,
    pub communities: Vec<CommunitySummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunityMember {
    pub space: String,
    pub node_id: String,
    pub node_kind: String,
    pub community_id: String,
    pub published_generation: i64,
    pub attachment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunityMemberCursor {
    pub space: String,
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunityMembersResponse {
    pub schema_version: String,
    #[serde(default)]
    pub scope: Option<CommunityScope>,
    pub members: Vec<CommunityMember>,
    pub next_cursor: Option<CommunityMemberCursor>,
}

// ── M5 truth axes (App PR) ──────────────────────────────────────────────
// `wenlan-types` is a workspace path dependency here, so these are aliases of
// the daemon-owned shapes rather than a hand-mirrored copy (unlike the
// page-map/community types above, which predate the daemon's release).
//
// The daemon populates `Page.truth` in two cases, both gated on the durable
// cutover generation being >= 1: an `EntryOnly`-reduced listing entry
// (`crates/wenlan-core/src/truth_adapter.rs::reduce_to_entry`), and a `Full`
// page whose call named it under a human-intent grant
// (`truth_adapter.rs::filter_pages`'s `with_truth` branch, reached via
// `get_page_explicit_browse`'s `x-wenlan-reader-intent` header). A plain
// `get_page` call carries no marker, so it resolves to `TruthGrant::Automatic`
// and never reaches either branch. `None` everywhere until the daemon's
// cutover ceremony runs.

/// Both M5 truth axes for one page.
pub type PageTruth = wenlan_types::pages::PageTruth;

/// Header declaring this client renders both M5 truth axes.
/// Mirrors `wenlan_core::truth_contract::CONTRACT_HEADER`.
const TRUTH_CONTRACT_HEADER: &str = "x-wenlan-truth-contract";
/// Mirrors `wenlan_core::truth_contract::TRUTH_CONTRACT_VERSION`.
const TRUTH_CONTRACT_VERSION: &str = "1";
/// Header marking a call as a human-initiated explicit browse rather than an
/// automatic/background read. Mirrors `wenlan_core::truth_contract::INTENT_HEADER`.
const TRUTH_INTENT_HEADER: &str = "x-wenlan-reader-intent";
/// Mirrors `wenlan_core::truth_contract::INTENT_MARKER_VALUE`.
const TRUTH_INTENT_EXPLICIT: &str = "explicit";

/// Where the daemon stands on the M5 truth cutover.
pub type TruthStatus = wenlan_types::responses::TruthStatus;

/// What a successful page review recorded. An alias of the daemon-owned
/// shape, same as [`TruthStatus`] above — `wenlan-types` is a workspace path
/// dependency here, so there is nothing to hand-mirror.
pub type PageReviewReceipt = wenlan_types::responses::PageReviewReceipt;

/// The daemon's coarse refusal body: one code and nothing else (D7 §7). Not a
/// daemon-owned wire type — `refusal_response` in
/// `crates/wenlan-server/src/page_routes.rs` builds this shape ad hoc rather
/// than through a shared struct, so this side has nothing to alias.
#[derive(Debug, Clone, Deserialize)]
struct PresenceRefusalBody {
    error: String,
}

/// What `POST /api/pages/{id}/review` answered, in the terms the UI renders.
///
/// Serializable because it is what the review command hands back to
/// JavaScript. Nothing here is capability material — no MAC, no nonce, not
/// even the nonce digest — so §7 holds by shape rather than by every future
/// caller remembering it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageReviewOutcome {
    /// The mark landed. A retry of a review that had already succeeded arrives
    /// here too: the daemon replays the stored receipt with the same 200, so
    /// this side cannot tell the two apart and has no reason to (T7).
    Applied { reviewed_page_version: i64 },
    /// The page's content changed after the human read it, so the approval is
    /// about text nobody is looking at any more (T6). The move is to show what
    /// is there now and ask again.
    ///
    /// `presence_conflict` also covers reusing one operation ID for different
    /// content (T8), which this client cannot produce — it derives the
    /// operation ID from the page and the digest, so different content is
    /// always a different operation. Every conflict reaching this app is the
    /// stale-content one.
    Stale,
    /// The capability's sixty seconds ran out between minting and arrival.
    /// Retrying mints a fresh one, so this is worth offering again.
    Expired,
    /// The capability did not verify, or named a page the daemon does not
    /// have. Deliberately coarse on the daemon side, so there is nothing more
    /// specific to report.
    Invalid,
    /// The nonce was already spent. Unreachable through the receipt path this
    /// client uses, and kept because a refusal class the UI cannot name is a
    /// refusal the UI renders as a lie.
    Replayed,
    /// The install secret is missing or unreadable, so the daemon cannot check
    /// presence at all. Not a caller error, and explicitly not a licence to
    /// proceed (D7 §5).
    Unavailable,
    /// This daemon has no review route — it predates the endpoint. Separate
    /// from [`Self::Unavailable`] because the fix is an upgrade, not a repair.
    Unsupported,
}

impl Default for WenlanClient {
    fn default() -> Self {
        Self::new()
    }
}

impl WenlanClient {
    pub fn new() -> Self {
        let port = daemon_port();
        Self {
            client: build_http_client(CONNECT_TIMEOUT, REQUEST_TIMEOUT),
            base_url: format!("http://127.0.0.1:{}", port),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// The base URL a reachability check runs against — exposed for
    /// `wire_state`, which reports it verbatim in `DaemonWire`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Points a client at an arbitrary `base_url` instead of the real daemon
    /// port. Test-only: lets `wire_state`'s tests exercise an unreachable
    /// daemon without touching the process-global `WENLAN_PORT`/`ORIGIN_PORT`
    /// env vars `WenlanClient::new()` reads (shared with every other test in
    /// this binary — see `env_lock` below).
    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: String) -> Self {
        Self {
            client: build_http_client(CONNECT_TIMEOUT, REQUEST_TIMEOUT),
            base_url,
        }
    }

    // ── Generic helpers ─────────────────────────────────────────────

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP GET {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    /// Read within the daemon's existing Space scope, without changing truth intent.
    /// Encode canonical names as query values, including Unicode and punctuation.
    /// A supplied but empty name must fail instead of becoming global.
    pub async fn get_json_scoped<T: DeserializeOwned>(
        &self,
        path: &str,
        space: Option<&str>,
    ) -> Result<T, String> {
        self.get_json_scoped_with_query(path, &[], space).await
    }

    /// Read with extra query parameters inside the daemon's existing Space
    /// scope. `reqwest` encodes each query value once, so source IDs containing
    /// Unicode, `?`, `&`, or `#` cannot alter the request target.
    pub async fn get_json_scoped_with_query<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        space: Option<&str>,
    ) -> Result<T, String> {
        let Some(space) = space else {
            if query.is_empty() {
                return self.get_json(path).await;
            }
            let resp = self
                .client
                .get(self.url(path))
                .query(query)
                .send()
                .await
                .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("HTTP GET {} returned {}: {}", path, status, body));
            }
            return resp
                .json()
                .await
                .map_err(|e| format!("Parse {}: {}", path, e));
        };
        let space = space.trim();
        if space.is_empty() {
            return Err("Space scope must not be blank".to_string());
        }
        let resp = self
            .client
            .get(self.url(path))
            .query(query)
            .query(&[("space", space)])
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP GET {} returned {}: {}", path, status, body));
        }
        let expected_scope = percent_encode_path_segment(space);
        if resp
            .headers()
            .get("x-wenlan-source-scope")
            .and_then(|value| value.to_str().ok())
            != Some(expected_scope.as_str())
        {
            return Err(format!(
                "HTTP GET {path}: daemon did not confirm the requested source Space scope"
            ));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn get_optional_json<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<Option<T>, String> {
        let resp = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!("HTTP GET {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map(Some)
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn post_json<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, String> {
        let resp = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP POST {} returned {}: {}", path, status, text));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    // ── M5 explicit-browse variants ─────────────────────────────────
    // Identical to the plain helpers above except they attach the two-header
    // marker that tells the daemon's truth guard this is a human-initiated
    // wiki browse (`crates/wenlan-server/src/truth_guard.rs`). Only call
    // these for a route the manifest marks Collection or NamedPage
    // (`GET /api/pages`, `POST /api/pages/search`, `GET /api/pages/{id}`);
    // an automatic/background read must use the plain helpers instead, or
    // the marker will be recorded against a call that never gestured at it.

    pub async fn get_json_explicit_browse<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, String> {
        let resp = self
            .client
            .get(self.url(path))
            .header(TRUTH_CONTRACT_HEADER, TRUTH_CONTRACT_VERSION)
            .header(TRUTH_INTENT_HEADER, TRUTH_INTENT_EXPLICIT)
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP GET {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub(crate) async fn post_page_update(
        &self,
        page_id: &str,
        body: &wenlan_types::requests::ManualPageEditRequest,
    ) -> Result<wenlan_types::responses::ManualPageWriteResponse, PageUpdateRequestError> {
        let path = format!(
            "/api/memory/{}/update-page",
            percent_encode_path_segment(page_id)
        );
        let response = self
            .client
            .post(self.url(&path))
            .json(body)
            .send()
            .await
            .map_err(|error| {
                PageUpdateRequestError::TransportOrDecode(format!("HTTP POST {path}: {error}"))
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(PageUpdateRequestError::Http { status, body });
        }

        response.json().await.map_err(|error| {
            PageUpdateRequestError::TransportOrDecode(format!("Parse {path}: {error}"))
        })
    }

    pub async fn put_json<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, String> {
        let resp = self
            .client
            .put(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("HTTP PUT {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP PUT {} returned {}: {}", path, status, text));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn delete_path<Resp: DeserializeOwned>(&self, path: &str) -> Result<Resp, String> {
        let resp = self
            .client
            .delete(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP DELETE {}: {}", path, e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP DELETE {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    async fn delete_empty(&self, path: &str) -> Result<(), String> {
        let resp = self
            .client
            .delete(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP DELETE {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP DELETE {} returned {}: {}",
                path, status, text
            ));
        }
        Ok(())
    }

    pub async fn delete_json<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, String> {
        let resp = self
            .client
            .delete(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("HTTP DELETE {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP DELETE {} returned {}: {}",
                path, status, text
            ));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn post_empty<Resp: DeserializeOwned>(&self, path: &str) -> Result<Resp, String> {
        let resp = self
            .client
            .post(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP POST {} returned {}: {}", path, status, text));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    // ── Health ──────────────────────────────────────────────────────

    pub async fn health(&self) -> Result<HealthResponse, String> {
        let path = "/api/health";
        let resp = self
            .client
            .get(self.url(path))
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP GET {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn status(&self) -> Result<wenlan_types::responses::StatusResponse, String> {
        self.get_json("/api/status").await
    }

    /// Where the daemon stands on the M5 truth cutover, or `None` when the
    /// daemon predates the field or the cutover has not begun
    /// (`crates/wenlan-server/src/routes.rs::handle_status` only sets it once
    /// `truth_cutover_generation() > 0`). This is itself an automatic read —
    /// the cutover state is not page prose, so it carries no explicit-browse
    /// marker.
    pub async fn truth_status(&self) -> Result<Option<TruthStatus>, String> {
        Ok(self.status().await?.truth)
    }

    /// Submits a minted page-review capability to `POST /api/pages/{id}/review`.
    ///
    /// Takes an already-built [`crate::presence::ReviewPageBody`] rather than
    /// the parts to build one, because the capability inside it must have been
    /// minted for this exact page and this exact content — and the only thing
    /// that can produce one is `presence::mint_page_review`.
    ///
    /// Does not go through [`Self::post_json`], which treats every non-2xx as
    /// an error string. Here the refusals *are* the answer: four of the five
    /// tell the UI something different to do, and flattening them into prose
    /// would throw that away.
    pub async fn review_page(
        &self,
        page_id: &str,
        body: &crate::presence::ReviewPageBody,
    ) -> Result<PageReviewOutcome, String> {
        let path = format!("/api/pages/{}/review", percent_encode_path_segment(page_id));
        let resp = self
            .client
            .post(self.url(&path))
            .json(body)
            .send()
            .await
            // The capability is in the request, never in this message: `body`
            // is not formatted here or anywhere below (§7).
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;

        let status = resp.status();
        if status.is_success() {
            let receipt: PageReviewReceipt = resp
                .json()
                .await
                .map_err(|e| format!("Parse {}: {}", path, e))?;
            return Ok(PageReviewOutcome::Applied {
                reviewed_page_version: receipt.reviewed_page_version,
            });
        }
        // The handler never answers 404 — a page it cannot find is
        // `presence_invalid`, so that the caller cannot probe the page table.
        // A 404 therefore means the router has no such route at all, which is
        // a daemon older than the endpoint.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(PageReviewOutcome::Unsupported);
        }
        let refusal: PresenceRefusalBody = resp
            .json()
            .await
            .map_err(|_| format!("HTTP POST {} returned {}", path, status))?;
        Ok(match refusal.error.as_str() {
            "presence_invalid" => PageReviewOutcome::Invalid,
            "presence_expired" => PageReviewOutcome::Expired,
            "presence_replayed" => PageReviewOutcome::Replayed,
            "presence_conflict" => PageReviewOutcome::Stale,
            "presence_unavailable" => PageReviewOutcome::Unavailable,
            // A code this build does not know is a newer daemon, not a
            // refusal to translate into the nearest familiar one — guessing
            // would show the user a reason that is not the reason.
            other => return Err(format!("unrecognized review refusal: {}", other)),
        })
    }

    // ── Capture stats ────────────────────────────────────────────────

    pub async fn get_capture_stats(&self) -> Result<HashMap<String, u64>, String> {
        let stats: CaptureStatsResponse = self.get_json("/api/capture-stats").await?;
        let mut counts = HashMap::new();
        counts.insert("total".to_string(), stats.total_chunks);
        Ok(counts)
    }

    pub async fn activity(&self) -> Result<wenlan_types::activity::ActivityResponse, String> {
        self.get_json("/api/activity").await
    }

    pub async fn list_tags(&self) -> Result<Vec<String>, String> {
        Ok(self.list_tag_inventory().await?.tags)
    }

    pub(crate) async fn list_tag_inventory(&self) -> Result<TagInventoryResponse, String> {
        self.get_json("/api/tags").await
    }

    // ── Ingest ───────────────────────────────────────────────────────

    pub async fn ingest_webpage(
        &self,
        req: wenlan_types::requests::IngestWebpageRequest,
    ) -> Result<wenlan_types::responses::IngestResponse, String> {
        if !req.create_only {
            return self.post_json("/api/ingest/webpage", &req).await;
        }
        // A missing create route on an old daemon must fail closed; never
        // retry through the legacy replacement endpoint.
        let path = "/api/ingest/webpage/create";
        let resp = self
            .client
            .post(self.url(path))
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;
        if resp.status() == reqwest::StatusCode::CONFLICT {
            return Err("WEBPAGE_ALREADY_EXISTS".to_string());
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP POST {} returned {}: {}", path, status, text));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn distill_review(&self) -> Result<DistillReviewResponse, String> {
        self.post_json("/api/distill", &DistillReviewRequest {})
            .await
    }

    pub async fn redistill_page(&self, page_id: &str) -> Result<PageRedistillResponse, String> {
        let path = format!("/api/distill/{}", percent_encode_path_segment(page_id));
        self.post_json(&path, &PageRedistillRequest {}).await
    }

    // ── Note bases (a note that relies on other notes) ────────────────

    /// One basis call that keeps the daemon's status. A 409 is an answer the
    /// app shows ("it changed again"), not a failure, so the caller decides.
    async fn basis_call<Req: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Req>,
    ) -> Result<(u16, String), String> {
        let mut request = self.client.request(method.clone(), self.url(path));
        if let Some(body) = body {
            request = request.json(body);
        }
        let resp = request
            .send()
            .await
            .map_err(|e| format!("HTTP {} {}: {}", method, path, e))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    fn basis_failure(method: &str, path: &str, status: u16, text: &str) -> String {
        format!("HTTP {method} {path} returned {status}: {text}")
    }

    /// Mark `basis_page_id` as a basis of `page_id`, at the version the person
    /// is looking at now. A person's click is a gesture, so the basis note is
    /// read as an explicit browse.
    pub async fn mark_page_basis(
        &self,
        page_id: &str,
        basis_page_id: &str,
    ) -> Result<wenlan_types::page_basis::BasisRef, String> {
        let basis_path = format!("/api/pages/{}", percent_encode_path_segment(basis_page_id));
        let basis: serde_json::Value = self.get_json_explicit_browse(&basis_path).await?;
        let version = basis
            .get("page")
            .and_then(|page| page.get("version"))
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| format!("Parse {basis_path}: the note has no version"))?;
        let path = Self::basis_path(page_id, basis_page_id);
        self.put_json(
            &path,
            &wenlan_types::page_basis::BasisVersionRequest { version },
        )
        .await
    }

    pub async fn unmark_page_basis(
        &self,
        page_id: &str,
        basis_page_id: &str,
    ) -> Result<wenlan_types::page_basis::BasisRemoveResponse, String> {
        self.delete_path(&Self::basis_path(page_id, basis_page_id))
            .await
    }

    /// Say the citing note accounts for the basis at `version`. A 409 means the
    /// basis moved past `version` (or can no longer be confirmed): the caller
    /// refetches and asks the person to look again.
    pub async fn confirm_page_basis(
        &self,
        page_id: &str,
        basis_page_id: &str,
        version: i64,
    ) -> Result<BasisConfirmResult, String> {
        let path = format!("{}/confirm", Self::basis_path(page_id, basis_page_id));
        let (status, text) = self
            .basis_call(
                reqwest::Method::POST,
                &path,
                Some(&wenlan_types::page_basis::BasisVersionRequest { version }),
            )
            .await?;
        match status {
            200..=299 => serde_json::from_str(&text)
                .map(|basis| BasisConfirmResult::Confirmed { basis })
                .map_err(|e| format!("Parse {path}: {e}")),
            409 => Ok(BasisConfirmResult::Conflict {
                message: daemon_error_text(&text),
            }),
            _ => Err(Self::basis_failure("POST", &path, status, &text)),
        }
    }

    /// The text the citing note relied on next to the basis' text now. A 409
    /// carries a `reason` (deleted, archived, other space, history gone).
    pub async fn get_page_basis_diff(
        &self,
        page_id: &str,
        basis_page_id: &str,
    ) -> Result<BasisDiffResult, String> {
        let path = format!("{}/diff", Self::basis_path(page_id, basis_page_id));
        let (status, text) = self
            .basis_call::<()>(reqwest::Method::GET, &path, None)
            .await?;
        match status {
            200..=299 => serde_json::from_str(&text)
                .map(|diff| BasisDiffResult::Available { diff })
                .map_err(|e| format!("Parse {path}: {e}")),
            409 => {
                let reason = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v.get("reason").and_then(|r| r.as_str().map(str::to_owned)))
                    .unwrap_or_default();
                Ok(BasisDiffResult::Unavailable {
                    reason,
                    message: daemon_error_text(&text),
                })
            }
            _ => Err(Self::basis_failure("GET", &path, status, &text)),
        }
    }

    /// Every basis that is not ok with the notes waiting on it, grouped by
    /// basis note, optionally for one Space.
    pub async fn list_basis_pending(
        &self,
        space: Option<&str>,
    ) -> Result<Vec<wenlan_types::page_basis::BasisPendingEntry>, String> {
        let path = match space.map(str::trim).filter(|s| !s.is_empty()) {
            Some(space) => format!(
                "/api/pages/basis-pending?space={}",
                percent_encode_path_segment(space)
            ),
            None => "/api/pages/basis-pending".to_string(),
        };
        self.get_json(&path).await
    }

    fn basis_path(page_id: &str, basis_page_id: &str) -> String {
        format!(
            "/api/pages/{}/basis/{}",
            percent_encode_path_segment(page_id),
            percent_encode_path_segment(basis_page_id)
        )
    }

    // ── Lint and approval-gated repair ────────────────────────────────

    /// Run a typed lint request through the daemon's query-string contract.
    pub async fn lint(
        &self,
        query: wenlan_types::lint::LintRequestQuery,
    ) -> Result<wenlan_types::lint::LintReport, String> {
        let path = lint_query_path(&query);
        self.get_json(&path).await
    }

    /// Submit an agent-assisted lint verdict packet for the exact query.
    pub async fn lint_submit(
        &self,
        query: wenlan_types::lint::LintRequestQuery,
        submission: wenlan_types::lint::LintAgentSubmission,
    ) -> Result<wenlan_types::lint::LintReport, String> {
        let path = lint_query_path(&query);
        self.post_json(&path, &submission).await
    }

    /// Write every page, across all Spaces, as a pure OKF v0.2 bundle into
    /// `target_dir`. No Space header is sent, which matches
    /// `wenlan export okf <DIR>` without `--space`. The daemon owns target
    /// safety: a foreign or unsafe directory and a truth cutover come back as
    /// 409, a relative path as 422, with the daemon's reason in the body.
    pub async fn export_pages_okf(
        &self,
        target_dir: String,
    ) -> Result<wenlan_types::ExportStats, String> {
        let request = wenlan_types::requests::ExportPagesRequest {
            vault_path: Some(target_dir),
            format: Some(wenlan_types::requests::ExportFormat::Okf),
        };
        self.post_json("/api/pages/export", &request).await
    }

    pub async fn prepare_repair(
        &self,
        request: wenlan_types::repair::PrepareRepairRequest,
    ) -> Result<wenlan_types::repair::RepairManifest, String> {
        self.post_json("/api/repairs/prepare", &request).await
    }

    pub async fn prepare_current_repair(
        &self,
        request: wenlan_types::repair_current::PrepareCurrentRepairRequest,
    ) -> Result<wenlan_types::repair::RepairManifest, String> {
        self.post_json("/api/repairs/prepare-current", &request)
            .await
    }

    /// Read the daemon-owned recovery artifact for one exact review binding.
    /// A successful `null` means there is no durable pending repair; HTTP and
    /// decode failures remain errors so a recovery conflict cannot look empty.
    pub async fn repair_recovery(
        &self,
        review_id: &str,
    ) -> Result<Option<wenlan_types::repair_recovery::RepairRecovery>, String> {
        let path = format!(
            "/api/repairs/recovery/{}",
            percent_encode_path_segment(review_id)
        );
        self.get_json(&path).await
    }

    pub async fn repair_runtime_status(
        &self,
    ) -> Result<wenlan_types::repair_runtime::RepairRuntimeStatus, String> {
        self.get_json("/api/repairs/runtime").await
    }

    pub(crate) async fn resume_repair_runtime(
        &self,
        request: wenlan_types::repair_runtime::ResumeRepairRuntimeRequest,
    ) -> Result<wenlan_types::repair_runtime::RepairRuntimeStatus, RepairRuntimeRequestError> {
        let response = self
            .client
            .post(self.url("/api/repairs/runtime/resume"))
            .json(&request)
            .send()
            .await
            .map_err(|error| RepairRuntimeRequestError::Uncertain(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .map_err(|error| RepairRuntimeRequestError::Uncertain(error.to_string()))?;
            return Err(RepairRuntimeRequestError::Http { status, body });
        }
        response
            .json()
            .await
            .map_err(|error| RepairRuntimeRequestError::Uncertain(error.to_string()))
    }

    pub async fn apply_repair(
        &self,
        request: wenlan_types::repair::ApplyRepairRequest,
    ) -> Result<wenlan_types::repair::RepairApplyReceipt, String> {
        self.post_json("/api/repairs/apply", &request).await
    }

    pub async fn prepare_repair_operation(
        &self,
        request: wenlan_types::repair_prepare_operation::RepairPrepareOperationRequest,
    ) -> Result<wenlan_types::repair_prepare_operation::RepairPrepareOperationStatus, String> {
        self.post_json("/api/repairs/prepare-operation", &request)
            .await
    }

    pub async fn repair_prepare_operation_status(
        &self,
        request: wenlan_types::repair_prepare_operation::RepairPrepareOperationRequest,
    ) -> Result<wenlan_types::repair_prepare_operation::RepairPrepareOperationStatus, String> {
        self.post_json("/api/repairs/prepare-operation/status", &request)
            .await
    }

    pub async fn cancel_repair_prepare_operation(
        &self,
        request: wenlan_types::repair_prepare_operation::RepairPrepareOperationRequest,
    ) -> Result<wenlan_types::repair_prepare_operation::RepairPrepareOperationStatus, String> {
        self.post_json("/api/repairs/prepare-operation/cancel", &request)
            .await
    }

    pub async fn repair_operation_status(
        &self,
        request: wenlan_types::repair::ApplyRepairRequest,
    ) -> Result<wenlan_types::repair_operation::RepairOperationStatus, String> {
        self.post_json("/api/repairs/status", &request).await
    }

    pub async fn cancel_prepared_repair(
        &self,
        request: wenlan_types::repair::ApplyRepairRequest,
    ) -> Result<wenlan_types::repair_operation::RepairOperationStatus, String> {
        self.post_json("/api/repairs/cancel", &request).await
    }

    pub async fn verify_repair(
        &self,
        request: wenlan_types::repair::VerifyRepairRequest,
    ) -> Result<wenlan_types::repair::RepairVerificationReceipt, String> {
        self.post_json("/api/repairs/verify", &request).await
    }

    pub async fn repair_plan(
        &self,
        request: wenlan_types::repair_plan::RepairPlanRequest,
    ) -> Result<wenlan_types::repair_plan::RepairPlanSummary, String> {
        self.post_json("/api/repairs/plan", &request).await
    }

    pub async fn repair_plan_entries(
        &self,
        request: wenlan_types::repair_plan::RepairPlanEntriesRequest,
    ) -> Result<wenlan_types::repair_plan::RepairPlanEntriesPage, String> {
        self.post_json("/api/repairs/plan/entries", &request).await
    }

    pub async fn move_space(&self, from: &str, to: &str) -> Result<MoveSpaceResponse, String> {
        let path = format!(
            "/api/spaces/{}/move-to/{}",
            percent_encode_path_segment(from),
            percent_encode_path_segment(to)
        );
        self.post_empty(&path).await
    }

    // ── Chat export import ─────────────────────────────────────────

    pub async fn import_chat_export(
        &self,
        path: &str,
    ) -> Result<wenlan_types::import::ImportChatExportResponse, String> {
        // Per-request override, NOT `post_json`: the shared client carries the
        // 600s default, which fires on a large import the daemon still
        // completes. Error/status/decode mapping mirrors `post_json` exactly.
        let api_path = "/api/import/chat-export";
        let req = wenlan_types::import::ImportChatExportRequest {
            path: path.to_string(),
        };
        let resp = self
            .client
            .post(self.url(api_path))
            .timeout(CHAT_EXPORT_IMPORT_TIMEOUT)
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", api_path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "HTTP POST {} returned {}: {}",
                api_path, status, text
            ));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", api_path, e))
    }

    pub async fn list_pending_imports(
        &self,
    ) -> Result<Vec<wenlan_types::import::PendingImport>, String> {
        self.get_json("/api/import/state").await
    }

    // ── Agent registration ──────────────────────────────────────────

    /// POST /api/agents — tell the daemon the user connected `name` (an MCP
    /// client) so it is trusted from its first read. Idempotent on the daemon
    /// side; callers on the connect path treat a failure as non-fatal.
    pub async fn register_agent(
        &self,
        name: &str,
    ) -> Result<wenlan_types::responses::AgentResponse, String> {
        let req = wenlan_types::requests::RegisterAgentRequest {
            name: name.to_string(),
        };
        self.post_json("/api/agents", &req).await
    }

    // ── Source registry ──────────────────────────────────────────────

    pub async fn list_sources(&self) -> Result<Vec<wenlan_types::sources::Source>, String> {
        self.get_json("/api/sources").await
    }

    pub async fn add_source(
        &self,
        source_type: String,
        path: String,
    ) -> Result<wenlan_types::sources::Source, String> {
        let req = wenlan_types::requests::AddSourceRequest { source_type, path };
        self.post_json("/api/sources", &req).await
    }

    pub async fn remove_source(&self, id: &str) -> Result<(), String> {
        let path = format!("/api/sources/{}", percent_encode_path_segment(id));
        self.delete_empty(&path).await
    }

    pub async fn sync_source(
        &self,
        id: &str,
    ) -> Result<wenlan_types::responses::SyncStatsResponse, String> {
        let path = format!("/api/sources/{}/sync", percent_encode_path_segment(id));
        self.post_empty(&path).await
    }

    // ── Onboarding milestones ──────────────────────────────────────

    pub async fn list_onboarding_milestones(
        &self,
    ) -> Result<Vec<wenlan_types::onboarding::MilestoneRecord>, String> {
        self.get_json("/api/onboarding/milestones").await
    }

    pub async fn acknowledge_onboarding_milestone(&self, id: &str) -> Result<(), String> {
        let path = format!(
            "/api/onboarding/milestones/{}/acknowledge",
            percent_encode_path_segment(id)
        );
        let resp = self
            .client
            .post(self.url(&path))
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP POST {} returned {}: {}", path, status, text));
        }
        Ok(())
    }

    pub async fn reset_onboarding_milestones(&self) -> Result<(), String> {
        let path = "/api/onboarding/reset";
        let resp = self
            .client
            .post(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP POST {} returned {}: {}", path, status, text));
        }
        Ok(())
    }

    // ── Home delta feed ────────────────────────────────────────────────

    pub async fn list_recent_retrievals(
        &self,
        limit: i64,
    ) -> Result<Vec<wenlan_types::RetrievalEvent>, String> {
        let path = format!("/api/retrievals/recent?limit={}", limit);
        self.get_json(&path).await
    }

    pub async fn list_recent_changes(
        &self,
        limit: i64,
    ) -> Result<Vec<wenlan_types::PageChange>, String> {
        let path = format!("/api/pages/recent-changes?limit={}", limit);
        self.get_json(&path).await
    }

    pub async fn list_recent_memories(
        &self,
        limit: i64,
        since_ms: Option<i64>,
    ) -> Result<Vec<wenlan_types::RecentActivityItem>, String> {
        let path = match since_ms {
            Some(ms) => format!("/api/memory/recent?limit={}&since_ms={}", limit, ms),
            None => format!("/api/memory/recent?limit={}", limit),
        };
        self.get_json(&path).await
    }

    pub async fn get_enrichment_status(
        &self,
        source_id: &str,
    ) -> Result<wenlan_types::EnrichmentStatusResponse, String> {
        let path = format!(
            "/api/memory/{}/enrichment-status",
            percent_encode_path_segment(source_id)
        );
        self.get_json(&path).await
    }

    pub async fn get_memory_revisions(
        &self,
        source_id: &str,
    ) -> Result<wenlan_types::responses::ListMemoryRevisionsResponse, String> {
        let path = format!(
            "/api/memory/{}/revisions",
            percent_encode_path_segment(source_id)
        );
        self.get_json(&path).await
    }

    pub async fn list_unconfirmed_memories(
        &self,
        limit: i64,
    ) -> Result<Vec<wenlan_types::RecentActivityItem>, String> {
        let path = format!("/api/memory/unconfirmed?limit={}", limit);
        self.get_json(&path).await
    }

    pub async fn list_recent_pages(
        &self,
        limit: i64,
        since_ms: Option<i64>,
    ) -> Result<Vec<wenlan_types::RecentActivityItem>, String> {
        let path = match since_ms {
            Some(ms) => format!("/api/pages/recent?limit={}&since_ms={}", limit, ms),
            None => format!("/api/pages/recent?limit={}", limit),
        };
        self.get_json(&path).await
    }

    pub async fn get_page_sources(
        &self,
        page_id: &str,
    ) -> Result<Vec<wenlan_types::PageSourceWithMemory>, String> {
        let path = format!(
            "/api/pages/{}/sources",
            percent_encode_path_segment(page_id)
        );
        self.get_json(&path).await
    }

    /// `GET /api/sessions/{id}`; `None` when the daemon cannot show it.
    pub async fn get_session_detail(
        &self,
        session_id: &str,
    ) -> Result<Option<wenlan_types::SessionDetailResponse>, String> {
        let path = format!("/api/sessions/{}", percent_encode_path_segment(session_id));
        self.get_optional_json(&path).await
    }

    /// `GET /api/pages/{id}/sessions`.
    pub async fn get_page_sessions(
        &self,
        page_id: &str,
    ) -> Result<wenlan_types::PageSessionsResponse, String> {
        let path = format!(
            "/api/pages/{}/sessions",
            percent_encode_path_segment(page_id)
        );
        self.get_json(&path).await
    }

    pub async fn get_page_links(
        &self,
        page_id: &str,
    ) -> Result<wenlan_types::responses::PageLinksResponse, String> {
        let path = format!("/api/pages/{}/links", percent_encode_path_segment(page_id));
        self.get_json(&path).await
    }

    pub async fn set_page_review_required(
        &self,
        page_id: &str,
        required: bool,
    ) -> Result<wenlan_types::responses::PageReviewRequiredResponse, String> {
        let path = format!(
            "/api/pages/{}/review-required",
            percent_encode_path_segment(page_id)
        );
        self.put_json(
            &path,
            &wenlan_types::requests::SetPageReviewRequiredRequest { required },
        )
        .await
    }

    pub async fn get_page_revisions(&self, page_id: &str) -> Result<serde_json::Value, String> {
        let path = format!(
            "/api/pages/{}/revisions",
            percent_encode_path_segment(page_id)
        );
        self.get_json(&path).await
    }

    pub async fn list_orphan_links(
        &self,
        min_count: Option<usize>,
    ) -> Result<wenlan_types::responses::OrphanLinksResponse, String> {
        let path = match min_count {
            Some(min_count) => format!("/api/pages/orphan-links?min_count={}", min_count),
            None => "/api/pages/orphan-links".to_string(),
        };
        self.get_json(&path).await
    }

    // ── Page map ─────────────────────────────────────────────────────
    // `serde_json::Value` carries every body and response. The typed shapes do
    // exist now that the app is in the workspace — `wenlan_types::page_map`
    // (crates/wenlan-types/src/page_map.rs) is the daemon's own wire contract,
    // and the TypeScript interfaces in src/lib/tauri.ts mirror it field for
    // field. Deserializing into them here would only re-serialize on the way to
    // the frontend, and would silently drop any field the daemon adds before the
    // app learns about it. Passing the value through keeps the bridge honest;
    // typing it belongs on the day the Rust side needs to read a field.

    /// Page-map calls need the HTTP status in the client (409 → refetch+reapply,
    /// 404/405 → "daemon too old", 422 → invariant violation), which the shared
    /// helpers flatten into an opaque error string. On non-2xx this returns
    /// Err(json) where json is `{"status":<u16>,"error":"<body text>"}` so the
    /// TypeScript layer can branch on the code.
    async fn page_map_call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let method_str = method.to_string();
        let mut req = self.client.request(method, self.url(path));
        if let Some(body) = &body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("HTTP {} {}: {}", method_str, path, e))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(
                serde_json::json!({ "status": status.as_u16(), "error": text }).to_string(),
            );
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    /// Like `get_json`, but on non-2xx returns `Err(json)` where json is
    /// `{"status":<u16>,"error":"<body text>"}` (same envelope as
    /// `page_map_call`) instead of a formatted string. The community
    /// readiness engine needs the raw status to distinguish an old daemon
    /// (404/405 — route doesn't exist yet) from any other failure.
    async fn get_json_with_status<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(
                serde_json::json!({ "status": status.as_u16(), "error": text }).to_string(),
            );
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    pub async fn get_page_map(&self, page_id: &str) -> Result<serde_json::Value, String> {
        let path = format!("/api/pages/{}/map", percent_encode_path_segment(page_id));
        self.page_map_call(reqwest::Method::GET, &path, None).await
    }

    pub async fn improve_page_map(&self, page_id: &str) -> Result<serde_json::Value, String> {
        let path = format!(
            "/api/pages/{}/map/improve",
            percent_encode_path_segment(page_id)
        );
        self.page_map_call(reqwest::Method::POST, &path, Some(serde_json::json!({})))
            .await
    }

    pub async fn create_page_map_node(
        &self,
        page_id: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let path = format!(
            "/api/pages/{}/map/nodes",
            percent_encode_path_segment(page_id)
        );
        self.page_map_call(reqwest::Method::POST, &path, Some(body))
            .await
    }

    pub async fn patch_page_map_node(
        &self,
        page_id: &str,
        node_id: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let path = format!(
            "/api/pages/{}/map/nodes/{}",
            percent_encode_path_segment(page_id),
            percent_encode_path_segment(node_id)
        );
        self.page_map_call(reqwest::Method::PATCH, &path, Some(body))
            .await
    }

    pub async fn delete_page_map_node(
        &self,
        page_id: &str,
        node_id: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let path = format!(
            "/api/pages/{}/map/nodes/{}",
            percent_encode_path_segment(page_id),
            percent_encode_path_segment(node_id)
        );
        self.page_map_call(reqwest::Method::DELETE, &path, Some(body))
            .await
    }

    pub async fn put_page_map_layout(
        &self,
        page_id: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let path = format!(
            "/api/pages/{}/map/layout",
            percent_encode_path_segment(page_id)
        );
        self.page_map_call(reqwest::Method::PUT, &path, Some(body))
            .await
    }

    // ── Communities (M6 cartography) ────────────────────────────────────

    pub async fn list_communities(
        &self,
        space: &str,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<CommunityListResponse, String> {
        let mut path = format!(
            "/api/communities?space={}",
            percent_encode_path_segment(space)
        );
        if let Some(cursor) = cursor {
            path.push_str(&format!("&cursor={}", percent_encode_path_segment(cursor)));
        }
        if let Some(limit) = limit {
            path.push_str(&format!("&limit={}", limit));
        }
        self.get_json_with_status(&path).await
    }

    pub async fn list_community_members(
        &self,
        space: &str,
        cursor: Option<&CommunityMemberCursor>,
        limit: Option<usize>,
    ) -> Result<CommunityMembersResponse, String> {
        let mut path = format!(
            "/api/communities/members?space={}",
            percent_encode_path_segment(space)
        );
        if let Some(cursor) = cursor {
            path.push_str(&format!(
                "&cursor_space={}&cursor_node_id={}",
                percent_encode_path_segment(&cursor.space),
                percent_encode_path_segment(&cursor.node_id)
            ));
        }
        if let Some(limit) = limit {
            path.push_str(&format!("&limit={}", limit));
        }
        self.get_json_with_status(&path).await
    }

    pub async fn test_llm(
        &self,
        endpoint: String,
        model: String,
        api_key: Option<String>,
    ) -> Result<wenlan_types::requests::TestLlmResponse, String> {
        // Hand-built body: pinned wenlan-types 0.12 TestLlmRequest has no
        // api_key field; the daemon (≥0.13) reads it, 0.12 never receives it
        // because the UI omits the key below 0.13.
        let mut body = serde_json::json!({ "endpoint": endpoint, "model": model });
        if let Some(key) = api_key.filter(|k| !k.is_empty()) {
            body["api_key"] = serde_json::Value::String(key);
        }
        self.post_json("/api/llm/test", &body).await
    }

    pub async fn get_on_device_model(&self) -> Result<OnDeviceModelResponse, String> {
        self.get_json("/api/on-device-model").await
    }

    /// Bounded by [`MODEL_DOWNLOAD_TIMEOUT`], not the client's
    /// [`REQUEST_TIMEOUT`]: the daemon answers only after the download and
    /// engine init finish.
    pub async fn download_on_device_model(&self, model_id: String) -> Result<(), String> {
        let path = "/api/on-device-model/download";
        let req = OnDeviceModelRequest { model_id };
        let resp = self
            .client
            .post(self.url(path))
            .timeout(MODEL_DOWNLOAD_TIMEOUT)
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("HTTP POST {}: {}", path, e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP POST {} returned {}: {}", path, status, text));
        }
        let _resp: wenlan_types::responses::SuccessResponse = resp
            .json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))?;
        Ok(())
    }

    // ── Refinery queue ─────────────────────────────────────────────────────

    pub async fn list_refinements(
        &self,
        limit: Option<usize>,
    ) -> Result<wenlan_types::responses::ListRefinementsResponse, String> {
        let mut path = "/api/refinery/queue".to_string();
        if let Some(limit) = limit {
            path.push_str(&format!("?limit={limit}"));
        }
        // The daemon can run ahead of our pinned wenlan-types and emit proposal
        // actions this build doesn't know yet. Parse proposals one-by-one so an
        // unknown variant degrades to a skip instead of emptying the whole queue.
        let raw: serde_json::Value = self.get_json(&path).await?;
        let all = raw
            .get("proposals")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let proposals: Vec<wenlan_types::responses::RefinementProposalSummary> = raw
            .get("proposals")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| serde_json::from_value(p.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        if proposals.len() < all {
            tracing::warn!(
                skipped = all - proposals.len(),
                "list_refinements: skipped refinement proposals with unknown action variants"
            );
        }
        Ok(wenlan_types::responses::ListRefinementsResponse { proposals })
    }

    pub async fn accept_refinement(
        &self,
        id: &str,
    ) -> Result<wenlan_types::responses::AcceptRefinementResponse, String> {
        self.post_empty(&format!(
            "/api/refinery/queue/{}/accept",
            percent_encode_path_segment(id)
        ))
        .await
    }

    pub async fn reject_refinement(
        &self,
        id: &str,
    ) -> Result<wenlan_types::responses::RejectRefinementResponse, String> {
        self.post_empty(&format!(
            "/api/refinery/queue/{}/reject",
            percent_encode_path_segment(id)
        ))
        .await
    }

    // ── Config ─────────────────────────────────────────────────────────────

    /// GET /api/config — return the daemon's current config.
    pub async fn get_config(&self) -> Result<wenlan_types::responses::ConfigResponse, String> {
        self.get_json("/api/config").await
    }

    /// PUT /api/config — update one or more fields and return the new config.
    /// Pass `Option<T>` fields; `None` leaves a field unchanged.
    pub async fn update_config(
        &self,
        req: wenlan_types::requests::UpdateConfigRequest,
    ) -> Result<wenlan_types::responses::ConfigResponse, String> {
        let body = sparse_update_config(req)?;
        self.put_json("/api/config", &body).await
    }

    pub async fn background_ai_enabled(&self) -> Result<bool, String> {
        Ok(self
            .get_config()
            .await?
            .background_ai_enabled
            .unwrap_or(true))
    }

    pub async fn set_background_ai_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut req = empty_update();
        req.background_ai_enabled = Some(enabled);
        let updated = self.update_config(req).await?;
        if updated.background_ai_enabled != Some(enabled) {
            return Err("The daemon does not support the background AI setting yet".into());
        }
        Ok(())
    }

    /// GET /api/setup/status — return daemon-owned setup/model/key state.
    ///
    /// Bounded by [`HEALTH_TIMEOUT`], not the ingest-sized [`REQUEST_TIMEOUT`]:
    /// the app's first-run gate (`should_show_wizard`, consumed by
    /// `src/App.tsx`) calls this on every launch, while the daemon may have
    /// bound its socket but not finished initializing. Inheriting the 600s
    /// backstop lets one attempt hold the window on a status screen for
    /// minutes. The caller retries, so a bounded failure is recoverable and a
    /// hang is not.
    pub async fn get_setup_status(&self) -> Result<SetupStatusResponse, String> {
        let path = "/api/setup/status";
        let resp = self
            .client
            .get(self.url(path))
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP GET {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    /// Mark setup complete/incomplete through the daemon config endpoint.
    pub async fn set_setup_completed(&self, completed: bool) -> Result<(), String> {
        self.update_config(empty_update().with_setup_completed(completed))
            .await
            .map(|_| ())
    }

    pub async fn get_model_choice(&self) -> Result<(Option<String>, Option<String>), String> {
        let cfg = self.get_config().await?;
        Ok((cfg.routine_model, cfg.synthesis_model))
    }

    /// Patch daemon model selection. `None` preserves the existing daemon value.
    pub async fn set_model_choice(
        &self,
        routine_model: Option<String>,
        synthesis_model: Option<String>,
    ) -> Result<(), String> {
        self.update_config(empty_update().with_model_choice(routine_model, synthesis_model))
            .await
            .map(|_| ())
    }

    pub async fn get_external_llm(&self) -> Result<(Option<String>, Option<String>), String> {
        let cfg = self.get_config().await?;
        Ok((cfg.external_llm_endpoint, cfg.external_llm_model))
    }

    /// Presence flag from daemon ≥ 0.13 (`external_llm_api_key_configured`,
    /// spec §7.2). Reads raw JSON because pinned 0.12 ConfigResponse lacks
    /// the field; absent ⇒ false.
    pub async fn external_llm_key_configured(&self) -> Result<bool, String> {
        let cfg: serde_json::Value = self.get_json("/api/config").await?;
        Ok(cfg
            .get("external_llm_api_key_configured")
            .and_then(|b| b.as_bool())
            .unwrap_or(false))
    }

    /// Patch daemon external LLM config. `None` endpoint/model preserves the
    /// existing daemon value. `api_key`: `None` = omit (preserve stored key),
    /// `Some("")` = clear, `Some(key)` = replace (spec §7.2 tri-state).
    pub async fn set_external_llm(
        &self,
        endpoint: Option<String>,
        model: Option<String>,
        api_key: Option<String>,
    ) -> Result<(), String> {
        let mut body = sparse_update_config(empty_update().with_external_llm(endpoint, model))?;
        if let Some(key) = api_key {
            body["external_llm_api_key"] = serde_json::Value::String(key);
        }
        let _resp: wenlan_types::responses::ConfigResponse =
            self.put_json("/api/config", &body).await?;
        Ok(())
    }

    pub async fn set_remote_access_enabled(
        &self,
        enabled: bool,
    ) -> Result<wenlan_types::responses::ConfigResponse, String> {
        self.update_config(empty_update().with_remote_access_enabled(enabled))
            .await
    }

    /// GET /api/config/routing — resolved per-job routing (daemon ≥ PR #357).
    /// Feature-detection point: a daemon without the endpoint answers 404, which
    /// maps to `Ok(None)` (LEGACY mode) rather than an error. Any other non-2xx
    /// is a real failure and surfaces as `Err`.
    pub async fn get_resolved_routing(&self) -> Result<Option<ResolvedRouting>, String> {
        let path = "/api/config/routing";
        let resp = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| format!("HTTP GET {}: {}", path, e))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!("HTTP GET {} returned {}", path, resp.status()));
        }
        resp.json()
            .await
            .map(Some)
            .map_err(|e| format!("Parse {}: {}", path, e))
    }

    /// PATCH the per-job source pins (daemon ≥ PR #357). Each arg: `None` leaves
    /// the pin untouched, `Some("")` clears it (auto chain), `Some(source)` pins.
    /// Written as raw JSON because the pinned `UpdateConfigRequest` predates these
    /// fields (mirrors `set_external_llm`). Only call once the routing endpoint is
    /// known present — never PATCH unknown fields at an old daemon.
    ///
    /// `only_if_unset` asks the daemon to write a named pin only for a job that
    /// holds none, which is what a fill-the-blanks caller means: it read routing
    /// first, and a pin the user chose since that read must win. The field is
    /// sent only when it is true, so an unflagged write is byte-identical to
    /// what this client sent before the flag existed.
    pub async fn set_source_pin(
        &self,
        everyday_source: Option<String>,
        synthesis_source: Option<String>,
        only_if_unset: bool,
    ) -> Result<(), String> {
        let mut body = sparse_update_config(empty_update())?;
        if let Some(v) = everyday_source {
            body["everyday_source"] = serde_json::Value::String(v);
        }
        if let Some(v) = synthesis_source {
            body["synthesis_source"] = serde_json::Value::String(v);
        }
        if only_if_unset {
            body["only_if_unset"] = serde_json::Value::Bool(true);
        }
        let _resp: serde_json::Value = self.put_json("/api/config", &body).await?;
        Ok(())
    }
}

fn daemon_port() -> u16 {
    std::env::var("WENLAN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            std::env::var("ORIGIN_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(7878)
}

/// Percent-encode one URL path segment or query value (RFC 3986 unreserved set).
/// Names reaching the daemon are user-supplied — a space called `Archive#2026` or
/// a local-file `source_id` that is an absolute path would otherwise rewrite the
/// request target. Shared with `search.rs`; keep this the only copy.
pub(crate) fn percent_encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn lint_query_path(query: &wenlan_types::lint::LintRequestQuery) -> String {
    let mut params = Vec::new();
    if let Some(profile) = query.lint().profile {
        params.push(format!("profile={profile}"));
    }
    if let Some(space) = query.lint().space.as_deref() {
        params.push(format!("space={}", percent_encode_path_segment(space)));
    }
    if query.external_egress() {
        params.push("external_egress=true".to_string());
    }
    if query.agent_assist() {
        params.push("agent_assist=true".to_string());
    }
    if params.is_empty() {
        "/api/lint".to_string()
    } else {
        format!("/api/lint?{}", params.join("&"))
    }
}

// `UpdateConfigRequest` does not derive `Default` in wenlan-types, so
// build a baseline with every field set to `None` here. When/if wenlan-types
// adds the derive, these helpers can be deleted in favor of
// `UpdateConfigRequest { skip_apps: Some(...), ..Default::default() }`.
fn empty_update() -> wenlan_types::requests::UpdateConfigRequest {
    wenlan_types::requests::UpdateConfigRequest {
        skip_apps: None,
        skip_title_patterns: None,
        private_browsing_detection: None,
        setup_completed: None,
        clipboard_enabled: None,
        screen_capture_enabled: None,
        remote_access_enabled: None,
        routine_model: None,
        synthesis_model: None,
        external_llm_endpoint: None,
        external_llm_model: None,
        everyday_source: None,
        synthesis_source: None,
        background_ai_enabled: None,
        page_map_auto_suggest: None,
        only_if_unset: None,
        // Tri-state (Option<Option<_>>): outer None = omit from JSON =
        // preserve the stored key. Note sparse_update_config strips nulls,
        // so a future caller passing Some(None) to clear the key must bypass
        // that helper.
        external_llm_api_key: None,
    }
}

fn sparse_update_config(
    req: wenlan_types::requests::UpdateConfigRequest,
) -> Result<serde_json::Value, String> {
    let mut value =
        serde_json::to_value(req).map_err(|e| format!("Serialize config update: {}", e))?;
    if let serde_json::Value::Object(ref mut map) = value {
        map.retain(|_, value| !value.is_null());
    }
    Ok(value)
}

trait UpdateConfigBuilder {
    fn with_remote_access_enabled(self, v: bool) -> Self;
    fn with_setup_completed(self, v: bool) -> Self;
    fn with_model_choice(
        self,
        routine_model: Option<String>,
        synthesis_model: Option<String>,
    ) -> Self;
    fn with_external_llm(self, endpoint: Option<String>, model: Option<String>) -> Self;
}

impl UpdateConfigBuilder for wenlan_types::requests::UpdateConfigRequest {
    fn with_remote_access_enabled(mut self, v: bool) -> Self {
        self.remote_access_enabled = Some(v);
        self
    }
    fn with_setup_completed(mut self, v: bool) -> Self {
        self.setup_completed = Some(v);
        self
    }
    fn with_model_choice(
        mut self,
        routine_model: Option<String>,
        synthesis_model: Option<String>,
    ) -> Self {
        self.routine_model = routine_model;
        self.synthesis_model = synthesis_model;
        self
    }
    fn with_external_llm(mut self, endpoint: Option<String>, model: Option<String>) -> Self {
        self.external_llm_endpoint = endpoint;
        self.external_llm_model = model;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct PortEnvGuard {
        wenlan_port: Option<std::ffi::OsString>,
        origin_port: Option<std::ffi::OsString>,
    }

    impl PortEnvGuard {
        fn capture() -> Self {
            Self {
                wenlan_port: std::env::var_os("WENLAN_PORT"),
                origin_port: std::env::var_os("ORIGIN_PORT"),
            }
        }
    }

    impl Drop for PortEnvGuard {
        fn drop(&mut self) {
            match &self.wenlan_port {
                Some(value) => std::env::set_var("WENLAN_PORT", value),
                None => std::env::remove_var("WENLAN_PORT"),
            }
            match &self.origin_port {
                Some(value) => std::env::set_var("ORIGIN_PORT", value),
                None => std::env::remove_var("ORIGIN_PORT"),
            }
        }
    }

    async fn serve_json_once(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{}", addr), handle)
    }

    async fn serve_response_once(
        status: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len(),
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn repair_runtime_http_refusal_is_distinct_from_a_lost_response() {
        use wenlan_types::repair::{ApplyRepairRequest, RepairDigest};
        use wenlan_types::repair_runtime::ResumeRepairRuntimeRequest;
        let id = "repair_550e8400-e29b-41d4-a716-446655440001";
        let digest = RepairDigest::parse(&"ab".repeat(32)).unwrap();
        let request = ResumeRepairRuntimeRequest {
            instance_id: "instance".into(),
            apply: ApplyRepairRequest::try_new(
                id.into(),
                digest.clone(),
                format!("apply repair {id} {}", digest.as_str()),
            )
            .unwrap(),
            verification_receipt_digest: RepairDigest::parse(&"cd".repeat(32)).unwrap(),
        };
        for (status, body, definitive) in [
            (
                "409 Conflict",
                r#"{"error":"repair_runtime_pending_repairs"}"#,
                true,
            ),
            (
                "409 Conflict",
                r#"{"error":"repair_runtime_shutting_down"}"#,
                false,
            ),
            ("200 OK", "truncated JSON", false),
        ] {
            let (base_url, server) = serve_response_once(status, body).await;
            let client = WenlanClient {
                client: reqwest::Client::new(),
                base_url,
            };
            let error = client
                .resume_repair_runtime(request.clone())
                .await
                .unwrap_err();
            assert_eq!(error.definitive_refusal(false), definitive);
            assert!(server
                .await
                .unwrap()
                .starts_with("POST /api/repairs/runtime/resume "));
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            // The daemon can commit shutdown and close before the HTTP response arrives.
        });
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let error = client.resume_repair_runtime(request).await.unwrap_err();
        assert!(matches!(error, RepairRuntimeRequestError::Uncertain(_)));
        assert!(!error.definitive_refusal(false));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn post_page_update_serializes_the_complete_lossless_request() {
        let (base_url, request) =
            serve_response_once("200 OK", r#"{"ok":true,"projection_status":"synced"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let update = wenlan_types::requests::ManualPageEditRequest {
            edit: wenlan_types::requests::UpdatePageRequest {
                content: "  # Exact source\r\n\r\nBody  \r\n".to_string(),
                source_memory_ids: Vec::new(),
                expected_version: Some(7),
                caller_id: Some("wenlan-app".to_string()),
                operation_id: Some("operation-123".to_string()),
            },
            minor: None,
            confirm_basis: None,
        };

        let response = client
            .post_page_update("page-1", &update)
            .await
            .expect("page update succeeds");

        assert!(response.ok);
        assert_eq!(response.basis_confirm, None);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/memory/page-1/update-page HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({
                "content": "  # Exact source\r\n\r\nBody  \r\n",
                "source_memory_ids": [],
                "expected_version": 7,
                "caller_id": "wenlan-app",
                "operation_id": "operation-123",
            })
        );
    }

    #[tokio::test]
    async fn post_page_update_retains_http_status_and_body() {
        let (base_url, request) =
            serve_response_once("409 Conflict", r#"{"error":"page changed"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let update = wenlan_types::requests::ManualPageEditRequest {
            edit: wenlan_types::requests::UpdatePageRequest {
                content: "# Draft".to_string(),
                source_memory_ids: Vec::new(),
                expected_version: Some(7),
                caller_id: Some("wenlan-app".to_string()),
                operation_id: Some("operation-123".to_string()),
            },
            minor: None,
            confirm_basis: None,
        };

        let error = client
            .post_page_update("page-1", &update)
            .await
            .expect_err("409 must remain typed");

        assert_eq!(
            error,
            PageUpdateRequestError::Http {
                status: 409,
                body: r#"{"error":"page changed"}"#.to_string(),
            }
        );
        request.await.unwrap();
    }

    /// Serve one canned response per connection, in order, and hand back each
    /// request as it arrived.
    async fn serve_sequence(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();
                requests.push(String::from_utf8_lossy(&buf[..n]).to_string());
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len(),
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{}", addr), handle)
    }

    fn client_at(base_url: String) -> WenlanClient {
        WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    #[tokio::test]
    async fn post_page_update_carries_minor_and_confirm_basis_beside_the_page_fields() {
        let (base_url, request) = serve_response_once(
            "200 OK",
            r#"{"ok":true,"projection_status":"synced","basis_confirm":"moved_on"}"#,
        )
        .await;
        let update = wenlan_types::requests::ManualPageEditRequest {
            edit: wenlan_types::requests::UpdatePageRequest {
                content: "# Plan".to_string(),
                source_memory_ids: Vec::new(),
                expected_version: Some(3),
                caller_id: Some("wenlan-app".to_string()),
                operation_id: Some("operation-9".to_string()),
            },
            minor: Some(true),
            confirm_basis: Some(wenlan_types::page_basis::ConfirmBasisInput {
                basis_page_id: "pricing".to_string(),
                version: 5,
            }),
        };

        let response = client_at(base_url)
            .post_page_update("plan", &update)
            .await
            .expect("save succeeds");

        assert_eq!(
            response.basis_confirm,
            Some(wenlan_types::page_basis::BasisConfirmOutcome::MovedOn)
        );
        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({
                "content": "# Plan",
                "source_memory_ids": [],
                "expected_version": 3,
                "caller_id": "wenlan-app",
                "operation_id": "operation-9",
                "minor": true,
                "confirm_basis": {"basis_page_id": "pricing", "version": 5},
            })
        );
    }

    #[tokio::test]
    async fn mark_page_basis_reads_the_basis_version_then_puts_it() {
        let (base_url, requests) = serve_sequence(vec![
            ("200 OK", r#"{"page":{"id":"pricing","version":7}}"#),
            (
                "200 OK",
                r#"{"page_id":"pricing","title":"Pricing","state":"ok","from_version":7,"to_version":7,"minor_only":false}"#,
            ),
        ])
        .await;

        let basis = client_at(base_url)
            .mark_page_basis("plan", "pricing")
            .await
            .unwrap();

        assert_eq!((basis.state.as_str(), basis.from_version), ("ok", 7));
        let requests = requests.await.unwrap();
        assert_eq!(
            requests[0].lines().next().unwrap_or_default(),
            "GET /api/pages/pricing HTTP/1.1"
        );
        assert_eq!(
            requests[1].lines().next().unwrap_or_default(),
            "PUT /api/pages/plan/basis/pricing HTTP/1.1"
        );
        assert_eq!(
            request_body(&requests[1]),
            serde_json::json!({"version": 7})
        );
    }

    #[tokio::test]
    async fn unmark_page_basis_deletes_the_pair() {
        let (base_url, request) = serve_response_once("200 OK", r#"{"removed":true}"#).await;

        let receipt = client_at(base_url)
            .unmark_page_basis("plan", "pricing")
            .await
            .unwrap();

        assert!(receipt.removed);
        assert_eq!(
            request.await.unwrap().lines().next().unwrap_or_default(),
            "DELETE /api/pages/plan/basis/pricing HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn confirm_page_basis_answers_a_409_as_conflict_not_an_error() {
        let (base_url, requests) = serve_sequence(vec![
            ("409 Conflict", r#"{"error":"the basis is at version 6"}"#),
            (
                "200 OK",
                r#"{"page_id":"pricing","title":"Pricing","state":"ok","from_version":6,"to_version":6}"#,
            ),
        ])
        .await;
        let client = client_at(base_url);

        let refused = client
            .confirm_page_basis("plan", "pricing", 5)
            .await
            .unwrap();
        assert_eq!(
            refused,
            BasisConfirmResult::Conflict {
                message: "the basis is at version 6".to_string()
            }
        );
        let confirmed = client
            .confirm_page_basis("plan", "pricing", 6)
            .await
            .unwrap();
        assert!(matches!(confirmed, BasisConfirmResult::Confirmed { .. }));

        let requests = requests.await.unwrap();
        assert_eq!(
            requests[0].lines().next().unwrap_or_default(),
            "POST /api/pages/plan/basis/pricing/confirm HTTP/1.1"
        );
        assert_eq!(
            request_body(&requests[0]),
            serde_json::json!({"version": 5})
        );
    }

    #[tokio::test]
    async fn confirm_page_basis_keeps_other_failures_as_errors() {
        let (base_url, _request) =
            serve_response_once("500 Internal Server Error", r#"{"error":"boom"}"#).await;
        let error = client_at(base_url)
            .confirm_page_basis("plan", "pricing", 5)
            .await
            .unwrap_err();
        assert!(error.contains("500"), "{error}");
    }

    #[tokio::test]
    async fn basis_diff_returns_the_comparison_or_the_reason_there_is_none() {
        let (base_url, requests) = serve_sequence(vec![
            (
                "200 OK",
                r#"{"from_version":1,"to_version":2,"from_content":"a","to_content":"b","edits":[{"version":2,"edited_by":"manual_edit","at":1,"minor":false,"delta_summary":"x"}]}"#,
            ),
            (
                "409 Conflict",
                r#"{"error":"the comparison is not available: archived","reason":"archived"}"#,
            ),
        ])
        .await;
        let client = client_at(base_url);

        match client.get_page_basis_diff("plan", "pricing").await.unwrap() {
            BasisDiffResult::Available { diff } => {
                assert_eq!((diff.from_version, diff.to_version), (1, 2));
                assert_eq!(diff.edits.len(), 1);
            }
            other => panic!("expected a comparison, got {other:?}"),
        }
        match client.get_page_basis_diff("plan", "pricing").await.unwrap() {
            BasisDiffResult::Unavailable { reason, .. } => assert_eq!(reason, "archived"),
            other => panic!("expected a reason, got {other:?}"),
        }
        assert_eq!(
            requests.await.unwrap()[0]
                .lines()
                .next()
                .unwrap_or_default(),
            "GET /api/pages/plan/basis/pricing/diff HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn list_basis_pending_encodes_the_space_and_omits_it_when_blank() {
        let (base_url, requests) = serve_sequence(vec![
            (
                "200 OK",
                r#"[{"basis_page_id":"pricing","title":"Pricing","changed_version":2,"state":"pending","citers":[{"page_id":"plan","title":"Plan","minor_only":false}]}]"#,
            ),
            ("200 OK", "[]"),
        ])
        .await;
        let client = client_at(base_url);

        let entries = client.list_basis_pending(Some("Team / R&D")).await.unwrap();
        assert_eq!(entries[0].citers.len(), 1);
        assert!(client
            .list_basis_pending(Some("  "))
            .await
            .unwrap()
            .is_empty());

        let requests = requests.await.unwrap();
        assert_eq!(
            requests[0].lines().next().unwrap_or_default(),
            "GET /api/pages/basis-pending?space=Team%20%2F%20R%26D HTTP/1.1"
        );
        assert_eq!(
            requests[1].lines().next().unwrap_or_default(),
            "GET /api/pages/basis-pending HTTP/1.1"
        );
    }

    #[test]
    fn redistill_response_carries_the_preview_fields_and_defaults_them() {
        let staged: PageRedistillResponse = serde_json::from_str(
            r#"{"status":"ok","updated":false,"hint":null,"reason":null,"gated":true,"revision_card_id":"rev-3"}"#,
        )
        .unwrap();
        assert!(staged.gated && !staged.unchanged);
        assert_eq!(staged.revision_card_id.as_deref(), Some("rev-3"));
        let same: PageRedistillResponse = serde_json::from_str(
            r#"{"status":"ok","updated":false,"hint":null,"reason":null,"unchanged":true}"#,
        )
        .unwrap();
        assert!(same.unchanged && !same.gated);
        assert_eq!(same.revision_card_id, None);
    }

    async fn serve_status_once(
        status: u16,
        reason: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{}", addr), handle)
    }

    #[tokio::test]
    async fn optional_get_maps_only_not_found_to_none() {
        let (base_url, request) =
            serve_status_once(404, "Not Found", r#"{"error":"missing"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let value: Option<serde_json::Value> = client
            .get_optional_json("/api/memory/missing/detail")
            .await
            .unwrap();

        assert_eq!(value, None);
        assert_eq!(
            request.await.unwrap().lines().next().unwrap_or_default(),
            "GET /api/memory/missing/detail HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn optional_get_preserves_other_http_failures() {
        let (base_url, _request) =
            serve_status_once(500, "Internal Server Error", r#"{"error":"failed"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let error = client
            .get_optional_json::<serde_json::Value>("/api/memory/id/detail")
            .await
            .unwrap_err();

        assert!(error.contains("returned 500"), "{error}");
    }

    #[tokio::test]
    async fn optional_get_preserves_malformed_success_failures() {
        let (base_url, _request) = serve_status_once(200, "OK", "not-json").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let error = client
            .get_optional_json::<serde_json::Value>("/api/memory/id/detail")
            .await
            .unwrap_err();

        assert!(error.contains("Parse /api/memory/id/detail"), "{error}");
    }

    #[tokio::test]
    async fn optional_get_deserializes_present_values() {
        let (base_url, _request) = serve_status_once(
            200,
            "OK",
            r#"{"memory":{"source_id":"memory-present","title":"Present memory","content":"body","confirmed":true,"pinned":false,"last_modified":1,"chunk_count":1,"enrichment_status":"enriched","supersede_mode":"hide","access_count":0}}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let value = client
            .get_optional_json::<wenlan_types::responses::MemoryDetailResponse>(
                "/api/memory/id/detail",
            )
            .await
            .unwrap()
            .and_then(|response| response.memory);

        assert_eq!(
            value.map(|memory| memory.source_id),
            Some("memory-present".to_string())
        );
    }

    async fn serve_source_scope_once(
        body: &'static str,
        scope: Option<&'static str>,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let n = stream.read(&mut buf).await.unwrap();
            let header = scope.map_or(String::new(), |scope| {
                format!("x-wenlan-source-scope: {scope}\r\n")
            });
            let response = format!("HTTP/1.1 200 OK\r\n{header}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        (format!("http://{addr}"), server)
    }

    #[tokio::test]
    async fn scoped_get_rejects_missing_or_mismatched_scope_ack_before_json() {
        for scope in [None, Some("work")] {
            // An older daemon may ignore the query and return a global success.
            let (base_url, request) =
                serve_source_scope_once(r#"{"files":[{"title":"Global private source"}]}"#, scope)
                    .await;
            let client = WenlanClient::with_base_url(base_url);
            let error = client
                .get_json_scoped::<serde_json::Value>("/api/indexed-files", Some("專案 & R+D"))
                .await
                .unwrap_err();
            assert!(
                error.contains("did not confirm the requested source Space scope"),
                "{error}"
            );
            request.await.unwrap();
        }
    }

    #[tokio::test]
    async fn scoped_get_encodes_space_query_for_source_routes() {
        for (path, body) in [
            ("/api/indexed-files", r#"{"files":[]}"#),
            ("/api/chunks/document%2F1", "[]"),
            ("/api/webpage-chunks/https%3A%2F%2Fexample.com", "[]"),
        ] {
            let (base_url, request) =
                serve_source_scope_once(body, Some("%E5%B0%88%E6%A1%88%20%26%20R%2BD")).await;
            let client = WenlanClient::with_base_url(base_url);
            let response: serde_json::Value = client
                .get_json_scoped(path, Some("專案 & R+D"))
                .await
                .unwrap();
            assert_eq!(
                response,
                serde_json::from_str::<serde_json::Value>(body).unwrap()
            );
            let request = request.await.unwrap();
            assert_eq!(
                request.lines().next().unwrap(),
                format!("GET {path}?space=%E5%B0%88%E6%A1%88+%26+R%2BD HTTP/1.1")
            );
            let headers = request.to_ascii_lowercase();
            assert!(!headers.contains("x-wenlan-space:"), "{request}");
            assert!(!headers.contains("x-wenlan-truth-contract:"), "{request}");
            assert!(!headers.contains("x-wenlan-reader-intent:"), "{request}");
        }
    }

    #[tokio::test]
    async fn scoped_query_values_keep_full_source_urls_inside_the_query() {
        let source_id = "https://例.com/path?q=one&other=二#fragment";
        let space = "專案 & R+D";
        let (base_url, request) =
            serve_source_scope_once("{}", Some("%E5%B0%88%E6%A1%88%20%26%20R%2BD")).await;
        let client = WenlanClient::with_base_url(base_url);

        let _: serde_json::Value = client
            .get_json_scoped_with_query(
                "/api/source-annotations",
                &[("source", "webpage"), ("source_id", source_id)],
                Some(space),
            )
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap(),
            "GET /api/source-annotations?source=webpage&source_id=https%3A%2F%2F%E4%BE%8B.com%2Fpath%3Fq%3Done%26other%3D%E4%BA%8C%23fragment&space=%E5%B0%88%E6%A1%88+%26+R%2BD HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn source_annotation_post_serializes_typed_request_and_preserves_conflicts() {
        use wenlan_types::source_annotations::{SourceAnnotation, UpdateSourceAnnotationRequest};

        let req = UpdateSourceAnnotationRequest {
            source: "webpage".into(),
            source_id: "https://example.com/a?x=1#part".into(),
            space: Some("Work".into()),
            expected_version: 7,
            notes: Some("Keep the caveat".into()),
            page_ids: Some(vec!["page-1".into(), "page-2".into()]),
        };
        let (base_url, request) = serve_response_once(
            "200 OK",
            r#"{"source":"webpage","source_id":"https://example.com/a?x=1#part","notes":"Keep the caveat","page_ids":["page-1","page-2"],"version":8,"updated_at":123,"pages":[{"id":"page-1","title":"First"},{"id":"page-2","title":"Second"}]}"#,
        )
        .await;
        let client = WenlanClient::with_base_url(base_url);
        let response: SourceAnnotation = client
            .post_json("/api/source-annotations", &req)
            .await
            .unwrap();
        assert_eq!(response.version, 8);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap(),
            "POST /api/source-annotations HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({
                "source": "webpage",
                "source_id": "https://example.com/a?x=1#part",
                "space": "Work",
                "expected_version": 7,
                "notes": "Keep the caveat",
                "page_ids": ["page-1", "page-2"]
            })
        );

        let (base_url, request) = serve_response_once(
            "409 Conflict",
            r#"{"error":"source_annotation_version_conflict"}"#,
        )
        .await;
        let client = WenlanClient::with_base_url(base_url);
        let error = client
            .post_json::<_, SourceAnnotation>("/api/source-annotations", &req)
            .await
            .unwrap_err();
        assert!(
            error.contains("HTTP POST /api/source-annotations returned 409 Conflict"),
            "{error}"
        );
        assert!(
            error.contains("source_annotation_version_conflict"),
            "{error}"
        );
        request.await.unwrap();
    }

    #[tokio::test]
    async fn scoped_get_omitted_space_preserves_global_request() {
        let (base_url, request) = serve_json_once(r#"{"files":[]}"#).await;
        let client = WenlanClient::with_base_url(base_url);
        let response: wenlan_types::responses::IndexedFilesResponse = client
            .get_json_scoped("/api/indexed-files", None)
            .await
            .unwrap();
        assert!(response.files.is_empty());
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap(),
            "GET /api/indexed-files HTTP/1.1"
        );
        let headers = request.to_ascii_lowercase();
        assert!(!headers.contains("x-wenlan-space:"), "{headers}");
        assert!(!headers.contains("x-wenlan-truth-contract:"), "{headers}");
        assert!(!headers.contains("x-wenlan-reader-intent:"), "{headers}");
    }

    #[tokio::test]
    async fn scoped_get_rejects_blank_space_before_network() {
        // An unreachable target makes an accidental HTTP fallback observable.
        let client = WenlanClient::with_base_url("http://127.0.0.1:0".to_string());
        for space in ["", " ", "\t\r\n"] {
            let error = client
                .get_json_scoped::<serde_json::Value>("/api/indexed-files", Some(space))
                .await
                .unwrap_err();
            assert_eq!(error, "Space scope must not be blank");
        }
    }

    #[tokio::test]
    async fn scoped_get_preserves_unknown_scope_rejection() {
        let (base_url, request) = serve_status_once(
            422,
            "Unprocessable Entity",
            r#"{"error":"unknown Space: missing"}"#,
        )
        .await;
        let client = WenlanClient::with_base_url(base_url);
        let error = client
            .get_json_scoped::<wenlan_types::responses::IndexedFilesResponse>(
                "/api/indexed-files",
                Some("missing"),
            )
            .await
            .unwrap_err();
        assert!(error.contains("returned 422"), "{error}");
        assert!(error.contains("unknown Space: missing"), "{error}");
        assert!(request
            .await
            .unwrap()
            .starts_with("GET /api/indexed-files?space=missing HTTP/1.1"));
    }

    #[tokio::test]
    async fn get_json_explicit_browse_attaches_the_truth_contract_headers() {
        let (base_url, request) = serve_json_once(r#"{"pages":[]}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let _: serde_json::Value = client
            .get_json_explicit_browse("/api/pages/page-1")
            .await
            .unwrap();

        let request = request.await.unwrap();
        let headers = request.to_ascii_lowercase();
        assert!(
            headers.contains("x-wenlan-truth-contract: 1"),
            "missing contract header: {request}"
        );
        assert!(
            headers.contains("x-wenlan-reader-intent: explicit"),
            "missing intent header: {request}"
        );
    }

    #[tokio::test]
    async fn truth_status_reads_the_status_response_field() {
        let (base_url, _request) = serve_json_once(
            r#"{"is_running":true,"files_indexed":0,"files_total":0,"sources_connected":[],"truth":{"cutover_generation":3,"contract_version":1}}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let truth = client.truth_status().await.unwrap();

        assert_eq!(
            truth,
            Some(TruthStatus {
                cutover_generation: 3,
                contract_version: 1,
            })
        );
    }

    #[tokio::test]
    async fn truth_status_is_none_before_the_cutover_ships_the_field() {
        let (base_url, _request) = serve_json_once(
            r#"{"is_running":true,"files_indexed":0,"files_total":0,"sources_connected":[]}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let truth = client.truth_status().await.unwrap();

        assert_eq!(truth, None);
    }

    /// Accepts one connection, reads the request, then never responds.
    async fn serve_hang_once() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(stream);
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn requests_error_instead_of_hanging_when_daemon_never_responds() {
        let base_url = serve_hang_once().await;
        let client = WenlanClient {
            client: build_http_client(
                std::time::Duration::from_millis(200),
                std::time::Duration::from_millis(300),
            ),
            base_url,
        };

        let started = std::time::Instant::now();
        let result: Result<HealthResponse, String> = client.get_json("/api/health").await;

        let err = result.unwrap_err();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "request did not fail fast: {:?}",
            started.elapsed()
        );
        assert!(
            err.contains("HTTP GET /api/health"),
            "unexpected error shape: {err}"
        );
    }

    #[tokio::test]
    async fn health_probe_fails_fast_even_with_production_timeouts() {
        let base_url = serve_hang_once().await;
        let client = WenlanClient {
            client: build_http_client(CONNECT_TIMEOUT, REQUEST_TIMEOUT),
            base_url,
        };

        let started = std::time::Instant::now();
        let err = client.health().await.unwrap_err();

        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "health probe waited on the ingest-sized backstop: {:?}",
            started.elapsed()
        );
        assert!(
            err.contains("HTTP GET /api/health"),
            "unexpected error shape: {err}"
        );
    }

    /// The launch gate's request, against a daemon that accepts and then goes
    /// quiet. Without the explicit per-request timeout this inherits
    /// `REQUEST_TIMEOUT` (600s) and the app sits on its starting-runtime
    /// screen for ten minutes instead of retrying.
    #[tokio::test]
    async fn setup_status_fails_fast_even_with_production_timeouts() {
        let base_url = serve_hang_once().await;
        let client = WenlanClient {
            client: build_http_client(CONNECT_TIMEOUT, REQUEST_TIMEOUT),
            base_url,
        };

        let started = std::time::Instant::now();
        let err = client.get_setup_status().await.unwrap_err();

        // The bound is 5s, so 8s is slack for a loaded machine rather than a
        // second ceiling — anything near 10s means the timeout is not applied.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(8),
            "setup status waited on the ingest-sized backstop: {:?}",
            started.elapsed()
        );
        assert!(
            err.contains("HTTP GET /api/setup/status"),
            "unexpected error shape: {err}"
        );
    }

    /// Accepts one connection, waits `delay`, then answers 200 with `body`.
    /// The write result is ignored: the timeout control below drops its end
    /// first, and a reset on a deliberately abandoned socket is not a failure.
    async fn serve_json_after_once(
        body: &'static str,
        delay: std::time::Duration,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            tokio::time::sleep(delay).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            request
        });
        (format!("http://{}", addr), handle)
    }

    /// A large chat-export import legitimately outlives the shared 600s
    /// default (measured: 625.2s server-side for 10000 messages). The import
    /// carries its own 8h per-request bound instead; the generic helper keeps
    /// the short default. Scaled down: 300ms default vs a 1500ms response.
    #[tokio::test]
    async fn chat_export_import_outlives_the_default_request_timeout() {
        assert_eq!(
            CHAT_EXPORT_IMPORT_TIMEOUT,
            std::time::Duration::from_secs(8 * 60 * 60),
            "the import bound is eight hours, not the shared default"
        );
        let body = r#"{"import_id":"import-1","vendor":"chatgpt","conversations_total":10,"conversations_new":8,"conversations_skipped_existing":2,"memories_stored":42}"#;
        let delay = std::time::Duration::from_millis(1500);
        let default_timeout = std::time::Duration::from_millis(300);

        // Control: the generic helper inherits the short default and times out.
        let (control_url, control_server) = serve_json_after_once(body, delay).await;
        let control = WenlanClient {
            client: build_http_client(CONNECT_TIMEOUT, default_timeout),
            base_url: control_url,
        };
        let req = wenlan_types::import::ImportChatExportRequest {
            path: "/tmp/export.zip".to_string(),
        };
        let err = control
            .post_json::<wenlan_types::import::ImportChatExportRequest, serde_json::Value>(
                "/api/import/chat-export",
                &req,
            )
            .await
            .expect_err("the 300ms default must fire before the 1500ms response");
        assert!(
            err.contains("HTTP POST /api/import/chat-export"),
            "unexpected error shape: {err}"
        );
        control_server.abort();

        // Actual: the import's own bound survives the same slow response, and
        // the typed fields plus the request path are unchanged.
        let (base_url, request) = serve_json_after_once(body, delay).await;
        let client = WenlanClient {
            client: build_http_client(CONNECT_TIMEOUT, default_timeout),
            base_url,
        };
        let resp = client
            .import_chat_export("/tmp/export.zip")
            .await
            .expect("import outlives the short default");
        assert_eq!(resp.import_id, "import-1");
        assert_eq!(resp.vendor, "chatgpt");
        assert_eq!(resp.conversations_total, 10);
        assert_eq!(resp.conversations_new, 8);
        assert_eq!(resp.conversations_skipped_existing, 2);
        assert_eq!(resp.memories_stored, 42);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/import/chat-export HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({"path": "/tmp/export.zip"})
        );
    }

    /// Leaving `post_json` means this method owns its error mapping now: raw
    /// daemon failures and typed decode failures must read exactly as before.
    #[tokio::test]
    async fn import_chat_export_propagates_http_errors_and_decode_failures() {
        let (base_url, _request) = serve_response_once("409 Conflict", r#"{"error":"busy"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let err = client
            .import_chat_export("/tmp/export.zip")
            .await
            .expect_err("the fixture deliberately returns a conflict");
        assert!(
            err.contains("HTTP POST /api/import/chat-export returned 409"),
            "{err}"
        );
        assert!(err.contains("busy"), "{err}");

        let (base_url, _request) = serve_response_once("200 OK", "{}").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let err = client
            .import_chat_export("/tmp/export.zip")
            .await
            .expect_err("an empty object is not an ImportChatExportResponse");
        assert!(err.contains("Parse /api/import/chat-export"), "{err}");
    }

    #[tokio::test]
    async fn register_agent_posts_the_name_and_decodes_the_agent() {
        let (base_url, request) = serve_response_once(
            "200 OK",
            r#"{"id":"a-1","name":"claude-code","display_name":"Claude Code","agent_type":"api","description":null,"enabled":true,"trust_level":"full","last_seen_at":null,"memory_count":0,"created_at":1,"updated_at":1}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let agent = client.register_agent("claude-code").await.unwrap();

        assert_eq!(agent.name, "claude-code");
        assert_eq!(agent.trust_level, "full");
        assert_eq!(agent.last_seen_at, None);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/agents HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({"name": "claude-code"})
        );
    }

    #[tokio::test]
    async fn register_agent_propagates_http_errors_and_decode_failures() {
        let (base_url, _request) =
            serve_response_once("400 Bad Request", r#"{"error":"name must be valid"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let err = client
            .register_agent("unknown")
            .await
            .expect_err("the fixture deliberately rejects the name");
        assert!(err.contains("HTTP POST /api/agents returned 400"), "{err}");
        assert!(err.contains("name must be valid"), "{err}");

        let (base_url, _request) = serve_response_once("200 OK", "{}").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let err = client
            .register_agent("claude-code")
            .await
            .expect_err("an empty object is not an AgentResponse");
        assert!(err.contains("Parse /api/agents"), "{err}");
    }

    /// Accepts one connection, reads the request, waits `delay`, then answers
    /// 200 with `body`.
    async fn serve_json_after(delay: std::time::Duration, body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(delay).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        format!("http://{}", addr)
    }

    /// The download request stays open until the whole model is fetched and
    /// loaded, which outlasts the client's total backstop on a slow link. A
    /// client whose backstop is shorter than the daemon's reply must still see
    /// the download succeed; the control request on the same client shape
    /// proves the delay really exceeds that backstop.
    #[tokio::test]
    async fn model_download_is_not_cut_off_by_the_client_request_timeout() {
        let base_timeout = std::time::Duration::from_millis(300);
        let reply_delay = std::time::Duration::from_millis(1500);

        let control = WenlanClient {
            client: build_http_client(base_timeout, base_timeout),
            base_url: serve_json_after(reply_delay, r#"{"ok":true}"#).await,
        };
        let control_result: Result<wenlan_types::responses::SuccessResponse, String> = control
            .post_json(
                "/api/on-device-model/download",
                &OnDeviceModelRequest {
                    model_id: "qwen3-4b".into(),
                },
            )
            .await;
        assert!(
            control_result.is_err(),
            "control request should hit the short client timeout"
        );

        let client = WenlanClient {
            client: build_http_client(base_timeout, base_timeout),
            base_url: serve_json_after(reply_delay, r#"{"ok":true}"#).await,
        };
        let result = client.download_on_device_model("qwen3-4b".into()).await;
        assert!(
            result.is_ok(),
            "model download inherited the client request timeout: {result:?}"
        );
    }

    #[test]
    fn update_config_builder_can_set_setup_completed() {
        let req = empty_update().with_setup_completed(true);

        assert_eq!(req.setup_completed, Some(true));
        assert_eq!(req.skip_apps, None);
        assert_eq!(req.skip_title_patterns, None);
    }

    #[test]
    fn update_config_builder_patches_remote_access_without_touching_other_config() {
        let remote_req = empty_update().with_remote_access_enabled(true);
        assert_eq!(remote_req.remote_access_enabled, Some(true));
        assert_eq!(remote_req.setup_completed, None);
        assert_eq!(remote_req.skip_apps, None);
    }

    #[tokio::test]
    async fn export_pages_okf_posts_the_okf_format_for_every_space() {
        let (base_url, request) = serve_json_once(r#"{"exported":3,"skipped":1,"failed":0}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let stats = client
            .export_pages_okf("/Users/someone/okf-bundle".to_string())
            .await
            .unwrap();

        assert_eq!((stats.exported, stats.skipped, stats.failed), (3, 1, 0));
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/pages/export HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({ "vault_path": "/Users/someone/okf-bundle", "format": "okf" })
        );
        // All Spaces: a Space header would narrow the bundle to one Space.
        assert!(
            !request.to_ascii_lowercase().contains("x-wenlan-space"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn export_pages_okf_keeps_the_daemon_refusal_in_the_error() {
        let (base_url, request) = serve_response_once(
            "409 Conflict",
            r#"{"error":"directory is not empty and is not a Wenlan OKF export"}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let error = client
            .export_pages_okf("/Users/someone/Documents".to_string())
            .await
            .expect_err("the fixture refuses the target");

        assert!(
            error.contains("HTTP POST /api/pages/export returned 409"),
            "{error}"
        );
        assert!(
            error.contains(r#"{"error":"directory is not empty and is not a Wenlan OKF export"}"#),
            "{error}"
        );
        request.await.unwrap();
    }

    fn request_body(request: &str) -> serde_json::Value {
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    #[tokio::test]
    async fn remote_access_config_field_uses_daemon_patch_endpoint() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":true}"#;
        let (base_url, request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let config = client.set_remote_access_enabled(true).await.unwrap();

        assert!(config.remote_access_enabled);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "PUT /api/config HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({"remote_access_enabled": true})
        );
    }

    #[test]
    fn update_config_builder_patches_model_fields_without_touching_other_config() {
        let req = empty_update().with_model_choice(Some("claude-haiku-4-5-20251001".into()), None);

        assert_eq!(
            req.routine_model,
            Some("claude-haiku-4-5-20251001".to_string())
        );
        assert_eq!(req.synthesis_model, None);
        assert_eq!(req.external_llm_endpoint, None);
        assert_eq!(req.external_llm_model, None);
    }

    #[test]
    fn update_config_builder_patches_external_llm_fields_without_touching_models() {
        let req = empty_update().with_external_llm(None, Some("qwen3".into()));

        assert_eq!(req.external_llm_endpoint, None);
        assert_eq!(req.external_llm_model, Some("qwen3".to_string()));
        assert_eq!(req.routine_model, None);
        assert_eq!(req.synthesis_model, None);
    }

    #[tokio::test]
    async fn test_llm_omits_api_key_field_when_none_or_empty() {
        let (base_url, request) = serve_json_once(r#"{"response":"ok"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .test_llm("http://localhost:11434/v1".into(), "qwen3".into(), None)
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({"endpoint": "http://localhost:11434/v1", "model": "qwen3"})
        );

        let (base_url, request) = serve_json_once(r#"{"response":"ok"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .test_llm(
                "http://localhost:11434/v1".into(),
                "qwen3".into(),
                Some(String::new()),
            )
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({"endpoint": "http://localhost:11434/v1", "model": "qwen3"})
        );
    }

    #[tokio::test]
    async fn test_llm_includes_api_key_field_when_supplied() {
        let (base_url, request) = serve_json_once(r#"{"response":"ok"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .test_llm(
                "http://localhost:11434/v1".into(),
                "qwen3".into(),
                Some("sk-secret".into()),
            )
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({
                "endpoint": "http://localhost:11434/v1",
                "model": "qwen3",
                "api_key": "sk-secret"
            })
        );
    }

    #[tokio::test]
    async fn set_external_llm_omits_api_key_field_when_none() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false}"#;
        let (base_url, request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .set_external_llm(Some("http://localhost:11434/v1".into()), None, None)
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({"external_llm_endpoint": "http://localhost:11434/v1"})
        );
    }

    #[tokio::test]
    async fn set_external_llm_clears_key_with_empty_string() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false}"#;
        let (base_url, request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .set_external_llm(None, None, Some(String::new()))
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({"external_llm_api_key": ""})
        );
    }

    #[tokio::test]
    async fn set_external_llm_replaces_key_when_supplied() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false}"#;
        let (base_url, request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .set_external_llm(None, None, Some("sk-secret".into()))
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({"external_llm_api_key": "sk-secret"})
        );
    }

    #[tokio::test]
    async fn get_resolved_routing_maps_404_to_none() {
        // Feature detection: an old daemon without the endpoint 404s, and that
        // must read as LEGACY mode (Ok(None)), never as an error.
        let (base_url, _request) = serve_status_once(404, "Not Found", "not found").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        assert_eq!(client.get_resolved_routing().await.unwrap(), None);
    }

    #[tokio::test]
    async fn get_resolved_routing_parses_200_body() {
        let body = r#"{"everyday":{"source":"on_device","model":"qwen3-4b","mode":"pinned","pin":"on_device"},"synthesis":{"source":"external","model":"gpt-5.2","mode":"auto"},"pool":{"anthropic":{"configured":false,"everyday_model":null,"synthesis_model":null},"external":{"endpoint":"https://api.openai.com/v1","model":"gpt-5.2"},"on_device":{"selected":"qwen3-4b","loaded":true}}}"#;
        let (base_url, _request) = serve_json_once(body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let routing = client.get_resolved_routing().await.unwrap().unwrap();
        assert_eq!(routing.everyday.source, "on_device");
        assert_eq!(routing.everyday.mode, "pinned");
        assert_eq!(routing.everyday.pin.as_deref(), Some("on_device"));
        assert_eq!(routing.synthesis.model.as_deref(), Some("gpt-5.2"));
        // synthesis's JSON omits "pin" entirely — a daemon that predates
        // #357's pin field must still deserialize, defaulting to None.
        assert_eq!(routing.synthesis.pin, None);
        assert!(!routing.pool.anthropic.configured);
        assert!(routing.pool.on_device.unwrap().loaded);
    }

    #[tokio::test]
    async fn set_source_pin_sends_only_named_pins() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false}"#;
        let (base_url, request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .set_source_pin(Some("external".into()), None, false)
            .await
            .unwrap();

        let request = request.await.unwrap();
        let body = request_body(&request);
        assert_eq!(body, serde_json::json!({"everyday_source": "external"}));
        // Stated on its own because it is the compatibility promise: an
        // unflagged call is byte-identical to what shipped before the flag, so
        // a daemon that predates it sees nothing new.
        assert!(body.get("only_if_unset").is_none());
    }

    #[tokio::test]
    async fn set_source_pin_sends_only_if_unset_when_filling_blanks() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false}"#;
        let (base_url, request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .set_source_pin(Some("on_device".into()), None, true)
            .await
            .unwrap();

        let request = request.await.unwrap();
        assert_eq!(
            request_body(&request),
            serde_json::json!({"everyday_source": "on_device", "only_if_unset": true})
        );
    }

    #[tokio::test]
    async fn external_llm_key_configured_reads_presence_flag_from_raw_json() {
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false,"external_llm_api_key_configured":true}"#;
        let (base_url, _request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        assert!(client.external_llm_key_configured().await.unwrap());
    }

    #[tokio::test]
    async fn external_llm_key_configured_defaults_false_when_field_absent() {
        // 0.12 daemon shape: no `external_llm_api_key_configured` field at all.
        let config_body = r#"{"skip_apps":[],"skip_title_patterns":[],"private_browsing_detection":true,"setup_completed":true,"clipboard_enabled":true,"screen_capture_enabled":false,"remote_access_enabled":false}"#;
        let (base_url, _request) = serve_json_once(config_body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        assert!(!client.external_llm_key_configured().await.unwrap());
    }

    #[test]
    fn setup_status_response_deserializes_daemon_payload() {
        let status: SetupStatusResponse = serde_json::from_value(serde_json::json!({
            "setup_completed": false,
            "mode": "basic-memory",
            "anthropic_key_configured": false,
            "local_model_selected": null,
            "local_model_loaded": null,
            "local_model_cached": false
        }))
        .expect("daemon setup status payload should deserialize");

        assert!(!status.setup_completed);
        assert_eq!(status.mode, "basic-memory");
        assert!(!status.anthropic_key_configured);
        assert_eq!(status.local_model_selected, None);
        assert_eq!(status.local_model_loaded, None);
        assert!(!status.local_model_cached);
    }

    #[test]
    fn wenlan_client_prefers_wenlan_port_over_legacy_origin_port() {
        let _guard = env_lock();
        let _env = PortEnvGuard::capture();
        std::env::set_var("WENLAN_PORT", "8787");
        std::env::set_var("ORIGIN_PORT", "9898");

        let client = WenlanClient::new();

        assert_eq!(
            client.url("/api/health"),
            "http://127.0.0.1:8787/api/health"
        );
    }

    #[test]
    fn wenlan_client_falls_back_to_legacy_origin_port() {
        let _guard = env_lock();
        let _env = PortEnvGuard::capture();
        std::env::remove_var("WENLAN_PORT");
        std::env::set_var("ORIGIN_PORT", "9898");

        let client = WenlanClient::new();

        assert_eq!(
            client.url("/api/health"),
            "http://127.0.0.1:9898/api/health"
        );
    }

    #[tokio::test]
    async fn capture_stats_uses_daemon_capture_stats_endpoint() {
        let (base_url, request) = serve_json_once(r#"{"total_chunks":42}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let stats = client.get_capture_stats().await.unwrap();

        assert_eq!(stats.get("total"), Some(&42));
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/capture-stats HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn activity_uses_daemon_activity_endpoint() {
        let body = r#"{"state":"organizing","last_activity_at":1700000100,"assets":[{"kind":"memories","state":"running","done":7,"total":10,"blocked":0,"steps":[]}],"everyday":{"job":"everyday","lane":"on_device","model":"qwen3-4b","mode":"pinned","available":true},"synthesis":{"job":"synthesis","lane":"none","model":null,"mode":"unconfigured","available":false}}"#;
        let (base_url, request) = serve_json_once(body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let activity = client.activity().await.unwrap();

        assert_eq!(
            activity.state,
            wenlan_types::activity::ActivityState::Organizing
        );
        assert_eq!(activity.last_activity_at, Some(1_700_000_100));
        assert_eq!(activity.assets.len(), 1);
        assert_eq!(
            activity.assets[0].kind,
            wenlan_types::activity::ActivityAssetKind::Memories
        );
        assert_eq!(
            activity.everyday.lane,
            wenlan_types::activity::ActivityLane::OnDevice
        );
        assert!(!activity.synthesis.available);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/activity HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn list_tags_uses_daemon_tags_endpoint() {
        let (base_url, request) = serve_json_once(r#"{"tags":["ai","rust"]}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let tags = client.list_tags().await.unwrap();

        assert_eq!(tags, vec!["ai".to_string(), "rust".to_string()]);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/tags HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn list_tag_inventory_preserves_document_tag_map() {
        let (base_url, request) = serve_json_once(
            r#"{"tags":["ai","rust"],"document_tags":{"memory::mem1":["ai"],"page::page1":["rust"]}}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let inventory = client.list_tag_inventory().await.unwrap();

        assert_eq!(inventory.tags, vec!["ai".to_string(), "rust".to_string()]);
        assert_eq!(
            inventory.document_tags.get("memory::mem1"),
            Some(&vec!["ai".to_string()])
        );
        assert_eq!(
            inventory.document_tags.get("page::page1"),
            Some(&vec!["rust".to_string()])
        );
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/tags HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn ingest_webpage_uses_daemon_webpage_ingest_endpoint() {
        let (base_url, request) =
            serve_json_once(r#"{"chunks_created":3,"document_id":"https://example.com/post"}"#)
                .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let mut metadata = HashMap::new();
        metadata.insert("source".to_string(), "manual-url".to_string());
        let req = wenlan_types::requests::IngestWebpageRequest {
            url: "https://example.com/post".to_string(),
            title: "Example Post".to_string(),
            content: "A durable article body.".to_string(),
            metadata: Some(metadata),
            create_only: false,
            space: Default::default(),
        };

        let resp = client.ingest_webpage(req).await.unwrap();

        assert_eq!(resp.chunks_created, 3);
        assert_eq!(resp.document_id, "https://example.com/post");
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/ingest/webpage HTTP/1.1"
        );
        assert_eq!(
            request_body(&request),
            serde_json::json!({
                "url": "https://example.com/post",
                "title": "Example Post",
                "content": "A durable article body.",
                "metadata": {"source": "manual-url"}
            })
        );
    }

    fn webpage_create_request(create_only: bool) -> wenlan_types::requests::IngestWebpageRequest {
        wenlan_types::requests::IngestWebpageRequest {
            url: "https://example.com/create".into(),
            title: "New excerpt".into(),
            content: "Exact supplied excerpt.".into(),
            metadata: None,
            create_only,
            space: Default::default(),
        }
    }

    #[tokio::test]
    async fn ingest_webpage_create_uses_atomic_route_and_maps_only_create_conflict() {
        for create_only in [true, false] {
            let (base_url, request) =
                serve_response_once("409 Conflict", r#"{"error":"already exists"}"#).await;
            let client = WenlanClient {
                client: reqwest::Client::new(),
                base_url,
            };
            let error = client
                .ingest_webpage(webpage_create_request(create_only))
                .await
                .unwrap_err();
            let request = request.await.unwrap();
            if create_only {
                assert_eq!(error, "WEBPAGE_ALREADY_EXISTS");
                assert_eq!(
                    request.lines().next().unwrap(),
                    "POST /api/ingest/webpage/create HTTP/1.1"
                );
                assert_eq!(request_body(&request)["create_only"], true);
            } else {
                assert!(error.contains("409 Conflict"));
                assert_ne!(error, "WEBPAGE_ALREADY_EXISTS");
                assert_eq!(
                    request.lines().next().unwrap(),
                    "POST /api/ingest/webpage HTTP/1.1"
                );
                assert!(request_body(&request).get("create_only").is_none());
            }
        }
    }

    #[tokio::test]
    async fn ingest_webpage_create_404_never_retries_legacy_replacement() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let first = String::from_utf8_lossy(&buf[..n]).to_string();
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            drop(stream);
            let second =
                tokio::time::timeout(std::time::Duration::from_millis(150), listener.accept())
                    .await;
            assert!(
                second.is_err(),
                "Create must not issue a fallback write after 404"
            );
            first
        });
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let error = client
            .ingest_webpage(webpage_create_request(true))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "HTTP POST /api/ingest/webpage/create returned 404 Not Found: "
        );
        let request = requests.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap(),
            "POST /api/ingest/webpage/create HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn move_space_uses_daemon_space_move_endpoint() {
        let (base_url, request) = serve_json_once(r#"{"affected":7}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let resp = client.move_space("Inbox", "Archive").await.unwrap();

        assert_eq!(resp.affected, 7);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/spaces/Inbox/move-to/Archive HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn move_space_percent_encodes_space_names_as_path_segments() {
        let (base_url, request) = serve_json_once(r#"{"affected":2}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let resp = client
            .move_space("Work/Clients?old=true", "Archive#2026")
            .await
            .unwrap();

        assert_eq!(resp.affected, 2);
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/spaces/Work%2FClients%3Fold%3Dtrue/move-to/Archive%232026 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn distill_review_posts_empty_global_request_to_daemon() {
        let body = r#"{"pages_created":0,"scoped":false,"created_ids":[],"pending":[],"stale_pages":[],"stale_truncated":false,"orphan_topics":[],"clusters_found":0,"hint":"No page-sized cluster formed in this scope: nothing grouped into 3 or more related memories that fit one page. Distilling does not need a model. Capture more related memories on the same topic, then distill again. Choosing a model source in `wenlan setup` / `/wenlan setup` also links memories to topics automatically, which helps them group."}"#;
        let (base_url, request) = serve_json_once(body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let resp = client.distill_review().await.unwrap();

        assert_eq!(resp.pages_created, 0);
        assert!(!resp.scoped);
        assert_eq!(resp.clusters_found, Some(0));
        assert_eq!(
            resp.hint.as_deref(),
            Some("No page-sized cluster formed in this scope: nothing grouped into 3 or more related memories that fit one page. Distilling does not need a model. Capture more related memories on the same topic, then distill again. Choosing a model source in `wenlan setup` / `/wenlan setup` also links memories to topics automatically, which helps them group.")
        );
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/distill HTTP/1.1"
        );
        assert_eq!(request_body(&request), serde_json::json!({}));
    }

    #[tokio::test]
    async fn redistill_page_posts_empty_page_specific_request_to_daemon() {
        let body = r#"{"status":"skipped","updated":false,"hint":"page re-distill needs an LLM in the daemon"}"#;
        let (base_url, request) = serve_json_once(body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let resp = client.redistill_page("page_refresh").await.unwrap();

        assert_eq!(resp.status, "skipped");
        assert!(!resp.updated);
        assert_eq!(
            resp.hint.as_deref(),
            Some("page re-distill needs an LLM in the daemon")
        );
        assert_eq!(resp.reason, None, "absent reason reads as None");
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/distill/page_refresh HTTP/1.1"
        );
        assert_eq!(request_body(&request), serde_json::json!({}));

        // A citation-gate discard carries `reason` alongside updated=false.
        let blocked: PageRedistillResponse = serde_json::from_str(
            r#"{"status":"ok","updated":false,"hint":null,"reason":"citation verification failed (8 verified, 13 unverified, 0 stripped)"}"#,
        )
        .unwrap();
        assert_eq!(
            blocked.reason.as_deref(),
            Some("citation verification failed (8 verified, 13 unverified, 0 stripped)")
        );
    }

    #[tokio::test]
    async fn repair_bridge_lint_uses_the_typed_query_and_percent_encodes_values() {
        let (base_url, request) = serve_json_once("{}").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let query = wenlan_types::lint::LintRequestQuery::new(
            wenlan_types::lint::LintQuery::new(
                Some(wenlan_types::lint::LintProfile::Deep),
                Some("Team / R&D".to_string()),
            ),
            true,
            true,
        );

        let error = client
            .lint(query)
            .await
            .expect_err("the deliberately empty fixture is not a LintReport");

        assert!(error.contains("Parse /api/lint?"), "{error}");
        assert_eq!(
            request.await.unwrap().lines().next().unwrap_or_default(),
            "GET /api/lint?profile=deep&space=Team%20%2F%20R%26D&external_egress=true&agent_assist=true HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn repair_bridge_lint_submit_posts_the_typed_submission_and_propagates_decode_errors() {
        let (base_url, request) = serve_json_once("{}").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let query = wenlan_types::lint::LintRequestQuery::new(
            wenlan_types::lint::LintQuery::new(
                Some(wenlan_types::lint::LintProfile::Deep),
                Some("agent work".to_string()),
            ),
            false,
            true,
        );
        let submission = wenlan_types::lint::LintAgentSubmission::try_new(
            wenlan_types::lint::LintDigest::from_u64(7),
            Vec::new(),
        )
        .unwrap();
        let expected_body = serde_json::to_value(&submission).unwrap();

        let error = client
            .lint_submit(query, submission)
            .await
            .expect_err("the deliberately empty fixture is not a LintReport");

        assert!(error.contains("Parse /api/lint?"), "{error}");
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/lint?profile=deep&space=agent%20work&agent_assist=true HTTP/1.1"
        );
        assert_eq!(request_body(&request), expected_body);
    }

    #[tokio::test]
    async fn repair_bridge_apply_repair_posts_typed_request_and_propagates_http_errors() {
        let (base_url, request) =
            serve_response_once("409 Conflict", r#"{"error":"repair is stale"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let digest = wenlan_types::repair::RepairDigest::parse(
            "0000000000000000000000000000000000000000000000000000000000000009",
        )
        .unwrap();
        let manifest_id = "repair_00000000-0000-0000-0000-000000000009".to_string();
        let approval = format!("apply repair {manifest_id} {}", digest.as_str());
        let request_body_value =
            wenlan_types::repair::ApplyRepairRequest::try_new(manifest_id, digest, approval)
                .unwrap();
        let expected_body = serde_json::to_value(&request_body_value).unwrap();

        let error = client
            .apply_repair(request_body_value)
            .await
            .expect_err("the fixture deliberately returns a conflict");

        assert!(
            error.contains("HTTP POST /api/repairs/apply returned 409"),
            "{error}"
        );
        assert!(error.contains("repair is stale"), "{error}");
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/repairs/apply HTTP/1.1"
        );
        assert_eq!(request_body(&request), expected_body);
    }

    #[tokio::test]
    async fn repair_bridge_recovery_gets_encoded_review_id_and_accepts_empty_recovery() {
        let (base_url, request) = serve_json_once("null").await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let recovery = client.repair_recovery("review/id #1").await.unwrap();

        assert_eq!(recovery, None);
        assert_eq!(
            request.await.unwrap().lines().next().unwrap_or_default(),
            "GET /api/repairs/recovery/review%2Fid%20%231 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn repair_bridge_recovery_propagates_http_errors_instead_of_returning_none() {
        let (base_url, request) = serve_response_once(
            "409 Conflict",
            r#"{"error":"repair_recovery_review_conflict"}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let error = client
            .repair_recovery("review/id")
            .await
            .expect_err("the fixture deliberately returns a conflict");

        assert!(
            error.contains("HTTP GET /api/repairs/recovery/review%2Fid returned 409"),
            "{error}"
        );
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/repairs/recovery/review%2Fid HTTP/1.1"
        );
    }

    #[test]
    fn distill_review_deserializes_daemon_review_payload_with_centroid_embedding() {
        let payload = serde_json::json!({
            "pages_created": 1,
            "scoped": false,
            "created_ids": ["page_new"],
            "pending": [{
                "source_ids": ["mem_1", "mem_2"],
                "contents": ["First source", "Second source"],
                "entity_id": "entity_rust",
                "entity_name": "Rust",
                "space": "Engineering",
                "estimated_tokens": 220,
                "centroid_embedding": [0.1, 0.2],
                "existing_page_id": "page_rust",
                "existing_page_title": "Rust notes",
                "new_memory_count": 1
            }],
            "stale_pages": [{
                "page_id": "page_old",
                "title": "Old page",
                "summary": "Needs source review",
                "source_memory_ids": ["mem_old"],
                "sources_updated_count": 3,
                "stale_reason": "source_updated",
                "user_edited": false
            }],
            "stale_truncated": true,
            "orphan_topics": [{"label": "Vector clocks", "count": 4}]
        });

        let resp: DistillReviewResponse = serde_json::from_value(payload).unwrap();

        assert_eq!(resp.created_ids, vec!["page_new"]);
        assert_eq!(resp.pending[0].centroid_embedding, Some(vec![0.1, 0.2]));
        assert_eq!(
            resp.pending[0].existing_page_title.as_deref(),
            Some("Rust notes")
        );
        assert_eq!(resp.pending[0].new_memory_count, Some(1));
        assert_eq!(resp.stale_pages[0].page_id, "page_old");
        assert_eq!(resp.orphan_topics[0].label, "Vector clocks");
        assert!(resp.stale_truncated);
    }

    #[test]
    fn distill_review_rejects_unresolved_target_hint_shape() {
        let payload = serde_json::json!({
            "pages_created": 0,
            "pages_updated": 0,
            "unresolved": "unknown target",
            "hint": "target must be a page id"
        });

        let err = serde_json::from_value::<DistillReviewResponse>(payload).unwrap_err();

        assert!(
            err.to_string().contains("missing field") || err.to_string().contains("unknown field")
        );
    }

    #[tokio::test]
    async fn status_uses_daemon_status_endpoint() {
        let (base_url, request) = serve_json_once(
            r#"{"is_running":true,"files_indexed":42,"files_total":0,"sources_connected":[],"reranker":{"state":"disabled"},"reranker_light":{"state":"active","model_id":"bge"},"reranker_mode":"lite"}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let status = client.status().await.unwrap();

        assert_eq!(status.files_indexed, 42);
        assert_eq!(status.reranker_mode, "lite");
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/status HTTP/1.1"
        );
    }

    #[test]
    fn wenlan_client_exposes_enrichment_status_method() {
        let _get_enrichment_status = WenlanClient::get_enrichment_status;
    }

    #[test]
    fn enrichment_status_response_deserializes_daemon_payload() {
        let status: wenlan_types::EnrichmentStatusResponse =
            serde_json::from_value(serde_json::json!({
                "source_id": "mem-1",
                "summary": "complete",
                "steps": [
                    { "step": "classify", "status": "done", "error": null, "attempts": 1 }
                ]
            }))
            .unwrap();

        assert_eq!(status.source_id, "mem-1");
        assert_eq!(status.steps[0].step, "classify");
    }

    #[test]
    fn wenlan_client_exposes_refinery_queue_methods() {
        let _list = WenlanClient::list_refinements;
        let _accept = WenlanClient::accept_refinement;
        let _reject = WenlanClient::reject_refinement;
    }

    #[test]
    fn wenlan_client_exposes_daemon_model_config_methods() {
        let _get_model_choice = WenlanClient::get_model_choice;
        let _set_model_choice = WenlanClient::set_model_choice;
        let _get_external_llm = WenlanClient::get_external_llm;
        let _set_external_llm = WenlanClient::set_external_llm;
        let _external_llm_key_configured = WenlanClient::external_llm_key_configured;
    }

    #[allow(dead_code)]
    async fn test_llm_uses_daemon_response_envelope(client: WenlanClient) {
        let _: Result<wenlan_types::requests::TestLlmResponse, String> =
            client.test_llm(String::new(), String::new(), None).await;
    }

    #[test]
    fn test_llm_response_type_is_checked() {}

    #[allow(dead_code)]
    async fn on_device_model_uses_typed_response(client: WenlanClient) {
        let _: Result<OnDeviceModelResponse, String> = client.get_on_device_model().await;
    }

    #[allow(dead_code)]
    async fn download_on_device_model_uses_typed_request(client: WenlanClient) {
        let _: Result<(), String> = client.download_on_device_model(String::new()).await;
    }

    #[test]
    fn on_device_model_response_type_is_checked() {}

    #[test]
    fn wenlan_client_exposes_source_registry_methods() {
        let _list = WenlanClient::list_sources;
        let _add = WenlanClient::add_source;
        let _remove = WenlanClient::remove_source;
        let _sync = WenlanClient::sync_source;
    }

    #[test]
    fn wenlan_client_exposes_page_link_methods() {
        let _get = WenlanClient::get_page_links;
        let _list = WenlanClient::list_orphan_links;
        let _lock = WenlanClient::set_page_review_required;
        let _mark = WenlanClient::mark_page_basis;
        let _unmark = WenlanClient::unmark_page_basis;
        let _confirm = WenlanClient::confirm_page_basis;
        let _diff = WenlanClient::get_page_basis_diff;
        let _pending = WenlanClient::list_basis_pending;
    }

    #[test]
    fn wenlan_client_exposes_session_methods() {
        let _detail = WenlanClient::get_session_detail;
        let _page = WenlanClient::get_page_sessions;
    }

    #[test]
    fn session_responses_deserialize_daemon_payloads() {
        let detail: wenlan_types::SessionDetailResponse =
            serde_json::from_value(serde_json::json!({
                "session": {
                    "id": "ses_1",
                    "tool": "claude-code",
                    "external_id": "abc",
                    "first_write_at": 10,
                    "last_write_at": 20
                },
                "memories": [],
                "pages": [],
                "handoffs": []
            }))
            .unwrap();
        // Older daemons omit the field; the UI reads it as "no transcript".
        assert!(!detail.transcript_available);
        let pages: wenlan_types::PageSessionsResponse = serde_json::from_value(serde_json::json!({
            "sessions": [{
                "session": {
                    "id": "ses_1",
                    "tool": "codex",
                    "external_id": "abc",
                    "first_write_at": 10,
                    "last_write_at": 20
                },
                "via": "source"
            }]
        }))
        .unwrap();
        assert_eq!(pages.sessions[0].via, wenlan_types::PageSessionVia::Source);
    }

    #[test]
    fn wenlan_client_exposes_revision_history_methods() {
        let _memory = WenlanClient::get_memory_revisions;
        let _page = WenlanClient::get_page_revisions;
    }

    #[test]
    fn revision_history_responses_deserialize_daemon_payloads() {
        let memory: wenlan_types::responses::ListMemoryRevisionsResponse =
            serde_json::from_value(serde_json::json!({
                "current_source_id": "mem-1",
                "chain_depth": 1,
                "entries": [
                    {
                        "source_id": "mem-1",
                        "depth": 0,
                        "title": "Current",
                        "content_preview": "Current version",
                        "last_modified": 10,
                        "source_agent": "claude-code",
                        "supersede_mode": "protected_revision",
                        "delta_summary": "Clarified wording"
                    }
                ]
            }))
            .unwrap();
        assert_eq!(memory.current_source_id, "mem-1");
        assert_eq!(
            memory.entries[0].delta_summary.as_deref(),
            Some("Clarified wording")
        );

        let page: wenlan_types::responses::ListPageRevisionsResponse =
            serde_json::from_value(serde_json::json!({
                "page_id": "page-1",
                "current_version": 2,
                "user_edited": false,
                "stale_reason": null,
                "entries": [
                    {
                        "version": 2,
                        "at": 1782490000000i64,
                        "edited_by": "distill",
                        "delta_summary": "Added backlinks",
                        "incoming_source_ids": ["mem-1"]
                    }
                ]
            }))
            .unwrap();
        assert_eq!(page.page_id, "page-1");
        assert_eq!(
            page.entries[0].incoming_source_ids.as_ref().unwrap(),
            &vec!["mem-1".to_string()]
        );
    }

    #[tokio::test]
    async fn list_communities_builds_scoped_query_and_parses_the_response() {
        let (base_url, request) = serve_json_once(
            r#"{"schema_version":"community-read-v1","scope":{"kind":"space","name":"Work"},"communities":[{"community_id":"c1","space":"Work","display_name":"Core","member_count":5,"published_generation":3,"algo_version":"v1","projection_version":"v1"}],"next_cursor":"c1"}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let response = client
            .list_communities("Work", None, None)
            .await
            .expect("list_communities succeeds");

        assert_eq!(response.schema_version, "community-read-v1");
        assert_eq!(response.communities.len(), 1);
        assert_eq!(response.communities[0].community_id, "c1");
        assert_eq!(response.communities[0].published_generation, 3);
        assert_eq!(response.next_cursor.as_deref(), Some("c1"));
        // The daemon's `scope` must survive the round trip — the Tauri
        // command re-serializes this struct verbatim for the frontend, so a
        // field the struct doesn't carry is silently dropped, not merely
        // unread.
        assert_eq!(
            response.scope,
            Some(CommunityScope {
                kind: "space".to_string(),
                name: Some("Work".to_string()),
            })
        );
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/communities?space=Work HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn list_communities_parses_the_nameless_scopes() {
        // `CommunityReadScope` is internally tagged: only the `space` variant
        // carries a name. A required `name` here failed the entire read for a
        // global or uncategorized scope, which surfaced as a permanent
        // `partial-error` for that space rather than as anything diagnosable.
        const BODIES: [(&str, &str); 2] = [
            (
                "global",
                r#"{"schema_version":"community-read-v1","scope":{"kind":"global"},"communities":[],"next_cursor":null}"#,
            ),
            (
                "uncategorized",
                r#"{"schema_version":"community-read-v1","scope":{"kind":"uncategorized"},"communities":[],"next_cursor":null}"#,
            ),
        ];

        for (kind, body) in BODIES {
            let (base_url, _request) = serve_json_once(body).await;
            let client = WenlanClient {
                client: reqwest::Client::new(),
                base_url,
            };

            let response = client
                .list_communities("uncategorized", None, None)
                .await
                .unwrap_or_else(|e| panic!("{kind} scope parses: {e}"));

            assert_eq!(
                response.scope,
                Some(CommunityScope {
                    kind: kind.to_string(),
                    name: None,
                })
            );
        }
    }

    #[tokio::test]
    async fn list_community_members_tolerates_a_response_with_no_scope_field() {
        // Read-tolerant: an older daemon (or a members route that never
        // added scope) omitting the field entirely must still parse.
        let (base_url, _request) = serve_json_once(
            r#"{"schema_version":"community-read-v1","members":[],"next_cursor":null}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let response = client
            .list_community_members("Work", None, None)
            .await
            .expect("list_community_members succeeds");

        assert_eq!(response.scope, None);
    }

    #[tokio::test]
    async fn list_communities_percent_encodes_space_and_cursor_and_appends_limit() {
        let (base_url, request) = serve_json_once(
            r#"{"schema_version":"community-read-v1","communities":[],"next_cursor":null}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        client
            .list_communities("Work/Clients", Some("cursor?"), Some(50))
            .await
            .expect("list_communities succeeds");

        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/communities?space=Work%2FClients&cursor=cursor%3F&limit=50 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn list_community_members_builds_scoped_query_with_cursor_and_parses_the_response() {
        let (base_url, request) = serve_json_once(
            r#"{"schema_version":"community-read-v1","members":[{"space":"Work","node_id":"e1","node_kind":"entity","community_id":"c1","published_generation":3,"attachment":"core"}],"next_cursor":{"space":"Work","node_id":"e2"}}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let cursor = CommunityMemberCursor {
            space: "Work".to_string(),
            node_id: "e0".to_string(),
        };
        let response = client
            .list_community_members("Work", Some(&cursor), Some(25))
            .await
            .expect("list_community_members succeeds");

        assert_eq!(response.members.len(), 1);
        assert_eq!(response.members[0].node_id, "e1");
        assert_eq!(response.members[0].published_generation, 3);
        assert_eq!(
            response.next_cursor,
            Some(CommunityMemberCursor {
                space: "Work".to_string(),
                node_id: "e2".to_string(),
            })
        );
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "GET /api/communities/members?space=Work&cursor_space=Work&cursor_node_id=e0&limit=25 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn list_communities_surfaces_status_for_an_old_daemon_missing_the_route() {
        let (base_url, _request) =
            serve_response_once("404 Not Found", r#"{"error":"no route"}"#).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };

        let err = client
            .list_communities("Work", None, None)
            .await
            .expect_err("404 surfaces as an error");
        let parsed: serde_json::Value =
            serde_json::from_str(&err).expect("status envelope is JSON");
        assert_eq!(parsed["status"], 404);
    }

    /// A body signed by a secret this test owns. What it signs is irrelevant
    /// to every test below — none of them run a daemon that could check it.
    fn review_body(dir: &tempfile::TempDir) -> crate::presence::ReviewPageBody {
        crate::presence::test_review_body(dir.path(), "p1")
    }

    async fn review_outcome_for(
        status: &'static str,
        body: &'static str,
    ) -> Result<PageReviewOutcome, String> {
        let (base_url, _request) = serve_response_once(status, body).await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let dir = tempfile::tempdir().unwrap();
        client.review_page("p1", &review_body(&dir)).await
    }

    #[tokio::test]
    async fn review_page_posts_the_presence_envelope_to_the_page_review_route() {
        let (base_url, request) = serve_json_once(
            r#"{"page_id":"p1","human_reviewed":true,"reviewed_page_version":4,"reviewed_page_digest":"d","protocol_version":1,"nonce_digest":"nd","verified_at":7,"caller_id":"wenlan-app","operation_id":"op-1"}"#,
        )
        .await;
        let client = WenlanClient {
            client: reqwest::Client::new(),
            base_url,
        };
        let dir = tempfile::tempdir().unwrap();

        let outcome = client
            .review_page("p1", &review_body(&dir))
            .await
            .expect("review_page succeeds");

        assert_eq!(
            outcome,
            PageReviewOutcome::Applied {
                reviewed_page_version: 4
            }
        );
        let request = request.await.unwrap();
        assert_eq!(
            request.lines().next().unwrap_or_default(),
            "POST /api/pages/p1/review HTTP/1.1"
        );
        // The daemon deserializes `ReviewPageRequest { presence }`; a body
        // that nested it anywhere else would be a 422 nobody could read.
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        let sent: serde_json::Value = serde_json::from_str(body).expect("the body is JSON");
        assert_eq!(sent["presence"]["action"], "review_page");
        assert_eq!(sent["presence"]["target_ids"], serde_json::json!(["p1"]));
    }

    #[tokio::test]
    async fn each_refusal_code_becomes_the_outcome_the_ui_renders() {
        // The whole point of not using `post_json` here. Four of these tell
        // the user to do something different; collapsing them into one error
        // string would make the difference unreachable.
        for (status, code, expected) in [
            (
                "403 Forbidden",
                r#"{"error":"presence_invalid"}"#,
                PageReviewOutcome::Invalid,
            ),
            (
                "403 Forbidden",
                r#"{"error":"presence_expired"}"#,
                PageReviewOutcome::Expired,
            ),
            (
                "409 Conflict",
                r#"{"error":"presence_replayed"}"#,
                PageReviewOutcome::Replayed,
            ),
            (
                "409 Conflict",
                r#"{"error":"presence_conflict"}"#,
                PageReviewOutcome::Stale,
            ),
            (
                "503 Service Unavailable",
                r#"{"error":"presence_unavailable"}"#,
                PageReviewOutcome::Unavailable,
            ),
        ] {
            let outcome = review_outcome_for(status, code)
                .await
                .unwrap_or_else(|e| panic!("{code} must be an outcome, not an error: {e}"));
            assert_eq!(outcome, expected, "for {code}");
        }
    }

    #[tokio::test]
    async fn a_daemon_without_the_review_route_is_unsupported_not_a_refusal() {
        // The handler answers `presence_invalid` for a page it cannot find, so
        // a 404 can only be the router having no such path — an upgrade, not a
        // repair, and a different thing to tell the user.
        let outcome = review_outcome_for("404 Not Found", r#"{"error":"not found"}"#)
            .await
            .expect("a 404 is an outcome");

        assert_eq!(outcome, PageReviewOutcome::Unsupported);
    }

    #[tokio::test]
    async fn an_unknown_refusal_code_is_an_error_rather_than_the_nearest_familiar_one() {
        let result = review_outcome_for("403 Forbidden", r#"{"error":"presence_future"}"#).await;

        assert!(
            result.is_err(),
            "a newer daemon's refusal must not be renamed into a reason that is not the reason",
        );
    }

    #[tokio::test]
    async fn a_review_outcome_never_carries_capability_material() {
        // D7 §7 facing the UI. The command's answer is the one thing that
        // crosses into JavaScript, so it is the one place a leak would land.
        let dir = tempfile::tempdir().unwrap();
        let body = review_body(&dir);
        let minted = serde_json::to_value(&body).unwrap();
        let mac = minted["presence"]["mac"].as_str().unwrap().to_string();
        let nonce = minted["presence"]["nonce"].as_str().unwrap().to_string();

        let outcome = serde_json::to_string(&PageReviewOutcome::Applied {
            reviewed_page_version: 4,
        })
        .unwrap();

        assert!(
            !outcome.contains(&mac),
            "the MAC reached JavaScript; nothing on PageReviewOutcome may carry capability material (D7 §7)",
        );
        assert!(
            !outcome.contains(&nonce),
            "the nonce reached JavaScript; nothing on PageReviewOutcome may carry capability material (D7 §7)",
        );
    }
}
