// SPDX-License-Identifier: Apache-2.0
//! HTTP client for talking to the origin daemon.
//!
//! Mirrors the relevant slice of `app/src/api.rs::WenlanClient` but uses
//! `anyhow::Result` (this is a CLI binary, not a Tauri command surface),
//! and reads `WENLAN_HOST` (full URL) instead of `WENLAN_PORT` so users can
//! point the CLI at a remote daemon over a tunnel.
//!
//! The methods exposed here are the subset the CLI subcommands need:
//! status, ping, search, recall, brief, store, list, agents.

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wenlan_types::{
    repo_wiki::ProjectRuleProposal,
    requests::{
        AddEntityAliasRequest, ArchiveEntitiesRequest, EntitySelection, ExportFormat,
        ExportPagesRequest, ListEntitiesRequest, ListMemoriesRequest, MergeEntityRequest,
        RestoreEntitiesRequest, SearchMemoryRequest, SetDefaultSpaceRequest, StoreMemoryRequest,
        UpdateAgentRequest,
    },
    responses::{
        AgentResponse, DefaultSpaceResponse, EntityAliasesResponse, EntityBulkResponse,
        ExportStats, HealthResponse, KnowledgePathResponse, ListEntitiesResponse,
        ListMemoriesResponse, MemoryDetailResponse, MergeEntityResponse, PageReadResponse,
        PendingRevisionItem, RevisionDismissResponse, SearchMemoryResponse, StoreMemoryResponse,
    },
    rulesync::{
        AdoptPreviewRequest, AdoptRequest, AdoptionPlan, PullbackDecision, PullbackPreview,
        RulesSyncReport, SharingPlan,
    },
    sources::Source,
    BriefHistoryResponse, BriefReadRequest, BriefReadResponse, BriefUpdateReceipt,
    BriefUpdateRequest, BriefVersionResponse, EntityDetail, OutboxDrainReport,
    PageSourceWithMemory, SessionRef, SESSION_HEADER,
};

mod lint;
pub(crate) mod recovery;
pub use lint::origin_host_from_env;

const DEFAULT_HOST: &str = "http://127.0.0.1:7878";

/// Local mirror of the daemon's `SyncStatsResponse` (defined in `wenlan-server`,
/// which the CLI must not depend on). Typed so envelope drift fails loud rather
/// than silently deserializing into `serde_json::Value`. The trailing optional
/// fields carry `#[serde(default)]` so older/leaner daemon responses parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStats {
    pub files_found: usize,
    pub ingested: usize,
    pub skipped: usize,
    pub errors: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<String>,
    /// OKF sources only: queued files not yet prepared after this sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_files: Option<u64>,
    /// OKF sources only: files not yet handed to the queue after this sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_files: Option<u64>,
}

/// Local mirror of the daemon's `AmbientSweepReport`/`AmbientJobSweepResult`
/// (defined in `wenlan-server`, which the CLI must not depend on). Typed so
/// envelope drift fails loud rather than silently deserializing into
/// `serde_json::Value`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientSweepReport {
    pub phases: Vec<AmbientJobSweepResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientJobSweepResult {
    pub job: String,
    pub attempted: bool,
    pub selected: bool,
    pub llm_calls: usize,
    pub panicked: bool,
    pub elapsed_ms: u128,
}

pub struct WenlanClient {
    base_url: String,
    http: reqwest::Client,
    headers: HeaderMap,
    recovery_enabled: bool,
    /// The AI session this process writes for, sent as `x-wenlan-session`.
    session: Option<SessionRef>,
    /// Set once the daemon has been told where the session ran.
    session_announced: tokio::sync::OnceCell<()>,
    /// Where the person key is read; `None` is the data root this command
    /// uses (`wenlan_core::config::data_root`).
    person_key_root: Option<std::path::PathBuf>,
}

impl WenlanClient {
    /// Create a client using `WENLAN_HOST` env var, or default to `http://127.0.0.1:7878`.
    pub fn from_env() -> Self {
        Self::from_env_with_context(None, None)
            .expect("empty Wenlan client context always builds valid headers")
    }

    /// Create a client that consistently identifies the caller and supplies
    /// client-local Space context through the common request headers.
    pub fn from_env_with_context(agent_name: Option<&str>, space: Option<&str>) -> Result<Self> {
        Self::from_env_with_options(agent_name, space, false)
    }

    /// Personal setup and project proposals require a direct connection to
    /// the local daemon. Its key lookup must never start the daemon.
    pub fn for_local_rules() -> Result<Self> {
        Self::from_env_with_options(None, None, true)
    }

    pub fn for_local_rules_with_identity(
        agent_name: Option<&str>,
        session: Option<&SessionRef>,
    ) -> Result<Self> {
        let mut client = Self::from_env_with_options(agent_name, None, true)?;
        if let Some(session) = session {
            client.headers.insert(
                SESSION_HEADER,
                HeaderValue::from_str(&session.to_string()).context("invalid session value")?,
            );
            client.http = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .default_headers(client.headers.clone())
                .build()
                .context("building Wenlan HTTP client")?;
        }
        Ok(client)
    }

    fn from_env_with_options(
        agent_name: Option<&str>,
        space: Option<&str>,
        local_rules: bool,
    ) -> Result<Self> {
        let base_url = origin_host_from_env();
        let mut http = reqwest::Client::builder();
        if local_rules {
            anyhow::ensure!(
                recovery::is_local_daemon_url(&base_url),
                "Use the Wenlan daemon on this computer for this rules command."
            );
            http = http.no_proxy().redirect(reqwest::redirect::Policy::none());
        }
        let mut headers = HeaderMap::new();
        if let Some(agent_name) = agent_name {
            headers.insert(
                "x-agent-name",
                HeaderValue::from_str(agent_name).context("invalid --agent-name header value")?,
            );
        }
        if let Some(space) = space {
            headers.insert(
                "x-wenlan-space",
                HeaderValue::from_str(space).context("invalid Space header value")?,
            );
        }
        Ok(Self {
            base_url,
            http: http
                .default_headers(headers.clone())
                .build()
                .context("building Wenlan HTTP client")?,
            headers,
            recovery_enabled: !local_rules,
            session: None,
            session_announced: tokio::sync::OnceCell::new(),
            person_key_root: None,
        })
    }

    /// A client for `base_url` with no identity headers, for tests in other
    /// modules that must never reach the real daemon.
    #[cfg(test)]
    pub(crate) fn for_test(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            http: reqwest::Client::new(),
            headers: HeaderMap::new(),
            recovery_enabled: false,
            session: None,
            session_announced: tokio::sync::OnceCell::new(),
            person_key_root: None,
        }
    }

    /// Read the person key from `root` instead of the data root, for tests.
    #[cfg(test)]
    pub(crate) fn with_person_key_root(mut self, root: &std::path::Path) -> Self {
        self.person_key_root = Some(root.to_path_buf());
        self
    }

    /// Attribute this client's writes to an AI session by sending
    /// `x-wenlan-session` on every request (the daemon reads it on writes and on
    /// the lookups the session card lists: search, recall, Brief and note reads).
    pub fn with_session(mut self, session: Option<SessionRef>) -> Result<Self> {
        if let Some(session) = &session {
            self.headers.insert(
                SESSION_HEADER,
                HeaderValue::from_str(&session.to_string()).context("invalid session value")?,
            );
            self.http = reqwest::Client::builder()
                .default_headers(self.headers.clone())
                .build()
                .context("building Wenlan HTTP client")?;
        }
        self.session = session;
        Ok(self)
    }

    /// The session this client writes for, if any.
    pub fn session(&self) -> Option<&SessionRef> {
        self.session.as_ref()
    }

    /// Tell the daemon where the session ran, once per process, before the
    /// first write. Best effort and short: failure never fails the write, and
    /// the daemon creates the session from the write's header regardless.
    async fn announce_session(&self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let _ = self
            .session_announced
            .get_or_try_init(|| async {
                let session = session.clone();
                let request = tokio::task::spawn_blocking(move || {
                    wenlan_types::session_context::build_upsert_request(
                        &session,
                        wenlan_types::session_context::process_work_context(),
                        &wenlan_types::session_context::TranscriptRoots::from_env(dirs::home_dir()),
                    )
                })
                .await
                .map_err(|_| ())?;
                let url = format!("{}/api/sessions", self.base_url);
                match self
                    .http
                    .post(&url)
                    .json(&request)
                    .timeout(std::time::Duration::from_secs(3))
                    .send()
                    .await
                {
                    // Any answer means the daemon was reachable; a connect
                    // failure stays retryable by a later write.
                    Ok(_) => Ok(()),
                    Err(_) => Err(()),
                }
            })
            .await;
    }

    /// Enable or disable recovery by starting the registered daemon service
    /// after a connection failure. Environment opt-out still applies when
    /// recovery is enabled here.
    pub fn with_recovery(mut self, enabled: bool) -> Self {
        self.recovery_enabled = enabled;
        self
    }

    /// True when this client's base URL is a loopback daemon. Callers must
    /// gate offline fallbacks (outbox queueing, offline Space resolution) on
    /// this — a connect failure against a remote host is a real error, never
    /// grounds to queue locally or guess a Space.
    pub fn is_local(&self) -> bool {
        recovery::is_local_daemon_url(&self.base_url)
    }

    /// The daemon URL this client talks to (from `WENLAN_HOST`, else the default).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// True when this client talks to the default local daemon, i.e. `WENLAN_HOST`
    /// does not select another one.
    pub fn is_default_host(&self) -> bool {
        self.base_url == DEFAULT_HOST
    }

    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response> {
        let retry = if recovery::autostart_allowed_from_env(self.recovery_enabled)
            && recovery::is_local_daemon_url(&self.base_url)
        {
            req.try_clone()
        } else {
            None
        };
        let response = req.send().await;
        match response {
            Ok(response) => Ok(response),
            Err(error) if error.is_connect() => {
                let Some(retry) = retry else {
                    return Err(error)
                        .with_context(|| what.to_owned())
                        .with_context(|| recovery::connect_failure_hint(&self.base_url));
                };
                let original = anyhow::Error::new(error).context(what.to_owned());
                if let Err(recovery_error) = recovery::recover(&self.base_url).await {
                    return Err(original.context(format!("{recovery_error:#}")).context(
                        "the daemon did not come up on its own — run `wenlan doctor` to see why",
                    ));
                }
                retry
                    .send()
                    .await
                    .with_context(|| what.to_owned())
                    .with_context(|| {
                        format!(
                            "the daemon was started but {} still does not answer — run `wenlan doctor`",
                            self.base_url
                        )
                    })
            }
            Err(error) => Err(error).with_context(|| what.to_owned()),
        }
    }

    // ===== Generic request helpers =====
    //
    // Every endpoint below is build-url / send / error_for_status / deserialize,
    // differing only in verb, path, request body, and response type. These
    // helpers hold that sequence once; each public method is a path expression
    // plus one delegating call. `hint_daemon_running` on `get_json` preserves
    // the "(is the daemon running?)" hint that health/list_sources/
    // list_pending_revisions/get_memory_detail carried before this existed —
    // the other GETs did not have it and still don't. The `.context("parsing
    // ... response")` text below is now generic ("parsing {path} response")
    // rather than each endpoint's previous literal wording; nothing in
    // crates/wenlan-cli/tests asserts the old text.

    /// GET `path` (e.g. "/api/health") and deserialize the JSON response.
    async fn get_json<R: DeserializeOwned>(
        &self,
        path: &str,
        hint_daemon_running: bool,
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let what = if hint_daemon_running {
            format!("GET {} failed (is the daemon running?)", url)
        } else {
            format!("GET {} failed", url)
        };
        let resp = self.send(self.http.get(&url), &what).await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .with_context(|| format!("parsing {} response", path))
    }

    /// POST `path` with a JSON `body` and deserialize the JSON response.
    async fn post_json<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send(
                self.http.post(&url).json(body),
                &format!("POST {} failed", url),
            )
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .with_context(|| format!("parsing {} response", path))
    }

    /// POST `path` with no body and deserialize the JSON response.
    async fn post_empty_json<R: DeserializeOwned>(&self, path: &str) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send(self.http.post(&url), &format!("POST {} failed", url))
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .with_context(|| format!("parsing {} response", path))
    }

    /// PUT `path` with a JSON `body` and deserialize the JSON response.
    async fn put_json<B: Serialize, R: DeserializeOwned>(&self, path: &str, body: &B) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send(
                self.http.put(&url).json(body),
                &format!("PUT {} failed", url),
            )
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .with_context(|| format!("parsing {} response", path))
    }

    /// PATCH `path` with a JSON `body` and deserialize the JSON response.
    async fn patch_json<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send(
                self.http.patch(&url).json(body),
                &format!("PATCH {} failed", url),
            )
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .with_context(|| format!("parsing {} response", path))
    }

    /// POST `path` with a JSON `body`, treating any 2xx as success and
    /// discarding the response body.
    async fn post_ok<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send(
                self.http.post(&url).json(body),
                &format!("POST {} failed", url),
            )
            .await?;
        ensure_ok(resp, &url).await?;
        Ok(())
    }

    /// DELETE `path`, treating any 2xx as success and discarding the response body.
    async fn delete_ok(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .send(self.http.delete(&url), &format!("DELETE {} failed", url))
            .await?;
        ensure_ok(resp, &url).await?;
        Ok(())
    }

    /// GET /api/health — daemon liveness + version.
    pub async fn health(&self) -> Result<HealthResponse> {
        self.get_json("/api/health", true).await
    }

    /// POST /api/memory/search — semantic recall for one query.
    pub async fn recall(&self, query: String, limit: usize) -> Result<SearchMemoryResponse> {
        let request = SearchMemoryRequest {
            query,
            limit,
            memory_type: None,
            space: None,
            source_agent: None,
            rerank: false,
        };
        self.announce_session().await;
        self.post_json("/api/memory/search", &request).await
    }

    /// Whether this client reads one Space (it sends `x-wenlan-space`).
    pub fn is_space_scoped(&self) -> bool {
        self.headers.contains_key("x-wenlan-space")
    }

    /// The same recall with no Space (an empty `x-wenlan-space` reads as none),
    /// to say whether other Spaces hold matches when this one holds none.
    pub async fn recall_outside_space(
        &self,
        query: String,
        limit: usize,
    ) -> Result<SearchMemoryResponse> {
        let request = SearchMemoryRequest {
            query,
            limit,
            memory_type: None,
            space: None,
            source_agent: None,
            rerank: false,
        };
        let url = format!("{}/api/memory/search", self.base_url);
        let resp = self
            .send(
                self.http
                    .post(&url)
                    .header("x-wenlan-space", "")
                    .json(&request),
                &format!("POST {url} failed"),
            )
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .context("parsing /api/memory/search response")
    }

    /// POST /api/brief — complete Brief plus optional same-Space recall.
    pub async fn brief(&self, request: &BriefReadRequest) -> Result<BriefReadResponse> {
        self.announce_session().await;
        self.post_json("/api/brief", request).await
    }

    /// PATCH /api/brief — item-level handoff deltas.
    pub async fn update_brief(&self, request: &BriefUpdateRequest) -> Result<BriefUpdateReceipt> {
        self.announce_session().await;
        self.patch_json("/api/brief", request).await
    }

    /// POST /api/handoff — the canonical handoff write contract.
    pub async fn handoff(
        &self,
        request: &wenlan_types::HandoffRequest,
        session: Option<&SessionRef>,
    ) -> Result<wenlan_types::HandoffReceipt> {
        if session == self.session.as_ref() {
            self.announce_session().await;
        }
        let url = format!("{}/api/handoff", self.base_url);
        let mut builder = self.http.post(&url).json(request);
        if let Some(session) = session {
            builder = builder.header(SESSION_HEADER, session.to_string());
        }
        let response = self.send(builder, &format!("POST {url} failed")).await?;
        ensure_ok(response, &url)
            .await?
            .json()
            .await
            .context("parsing /api/handoff response")
    }

    /// GET /api/brief/history — past Brief updates, newest first.
    pub async fn brief_history(&self, limit: usize) -> Result<BriefHistoryResponse> {
        self.get_json(&format!("/api/brief/history?limit={limit}"), true)
            .await
    }

    /// GET /api/brief/history/{version} — the Brief as of one past update.
    pub async fn brief_version(&self, version: i64) -> Result<BriefVersionResponse> {
        self.get_json(&format!("/api/brief/history/{version}"), true)
            .await
    }

    /// POST /api/memory/store — write a memory.
    pub async fn store(
        &self,
        content: String,
        memory_type: Option<String>,
    ) -> Result<StoreMemoryResponse> {
        let req = Self::store_request(content, memory_type);
        self.announce_session().await;
        self.post_json("/api/memory/store", &req).await
    }

    pub fn store_request(content: String, memory_type: Option<String>) -> StoreMemoryRequest {
        StoreMemoryRequest {
            content,
            memory_type,
            space: (None).into(),
            source_agent: None,
            title: None,
            confidence: None,
            supersedes: None,
            entity: None,
            entity_type: None,
            entity_id: None,
            structured_fields: None,
            retrieval_cue: None,
            said: None,
        }
    }

    /// POST /api/outbox/drain — ask the daemon to replay queued envelopes.
    pub async fn drain_outbox(&self) -> Result<OutboxDrainReport> {
        self.post_empty_json("/api/outbox/drain").await
    }

    /// POST /api/ambient/sweep — force one bounded pass over every ambient
    /// job type, bypassing the idle/resource gate (each job's own per-call
    /// slice bound is unchanged). Document and Citation phases can call the
    /// on-device LLM, so a full lap can legitimately take minutes; this
    /// request gets its own generous timeout rather than the client's
    /// default, which is sized for ordinary sub-second daemon calls.
    pub async fn sweep_ambient(&self) -> Result<AmbientSweepReport> {
        const SWEEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
        let url = format!("{}/api/ambient/sweep", self.base_url);
        let resp = self
            .send(
                self.http.post(&url).timeout(SWEEP_TIMEOUT),
                &format!("POST {url} failed"),
            )
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .context("parsing /api/ambient/sweep response")
    }

    /// POST /api/steep — run every Steep phase once now, page writing
    /// included, with the models chosen in `wenlan steep configure`. A full
    /// pass on an on-device model can take many minutes (the daemon's own
    /// deadline for it is 20), so the request waits longer than that.
    pub async fn run_steep(&self) -> Result<wenlan_types::responses::SteepResponse> {
        const STEEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25 * 60);
        let url = format!("{}/api/steep", self.base_url);
        let resp = self
            .send(
                self.http.post(&url).timeout(STEEP_TIMEOUT),
                &format!("POST {url} failed"),
            )
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json().await.context("parsing /api/steep response")
    }

    /// GET /api/sources — list registered sources.
    pub async fn list_sources(&self) -> Result<Vec<Source>> {
        self.get_json("/api/sources", true).await
    }

    /// POST /api/sources — register a source. Returns the created `Source`.
    pub async fn add_source(&self, source_type: &str, path: &str) -> Result<Source> {
        let req = serde_json::json!({ "source_type": source_type, "path": path });
        self.post_json("/api/sources", &req).await
    }

    /// POST /api/ingest/webpage (or `/create`) — save a document under its link.
    pub async fn save_source(
        &self,
        req: &wenlan_types::requests::IngestWebpageRequest,
    ) -> Result<wenlan_types::responses::IngestResponse> {
        let path = if req.create_only {
            "/api/ingest/webpage/create"
        } else {
            "/api/ingest/webpage"
        };
        self.post_json(path, req).await
    }

    /// GET /api/steep/new — how many items of material in the caller's scope
    /// no page uses yet.
    pub async fn new_material_total(&self) -> Result<u32> {
        let list: wenlan_types::SteepNewListResponse =
            self.get_json("/api/steep/new?limit=1", false).await?;
        Ok(list.total)
    }

    /// POST /api/sources/{id}/sync — sync a registered source, returning stats.
    pub async fn sync_source(&self, id: &str) -> Result<SyncStats> {
        self.post_empty_json(&format!("/api/sources/{}/sync", id))
            .await
    }

    /// POST /api/memory/list — list memories with optional filters.
    pub async fn list(
        &self,
        limit: Option<usize>,
        memory_type: Option<String>,
        confirmed: Option<bool>,
    ) -> Result<ListMemoriesResponse> {
        let req = build_list_request(limit, memory_type, confirmed);
        self.post_json("/api/memory/list", &req).await
    }

    /// GET /api/agents — list registered agents.
    pub async fn list_agents(&self) -> Result<Vec<AgentResponse>> {
        self.get_json("/api/agents", false).await
    }

    /// GET /api/agents/{name} — fetch a single agent.
    pub async fn get_agent(&self, name: &str) -> Result<AgentResponse> {
        self.get_json(&format!("/api/agents/{}", name), false).await
    }

    /// POST /api/spaces — register a new space.
    pub async fn create_space(&self, name: &str) -> Result<()> {
        self.post_ok("/api/spaces", &serde_json::json!({"name": name}))
            .await
    }

    /// POST /api/spaces/{from}/move-to/{to} — bulk reassign memories from one space to another.
    pub async fn move_space(&self, from: &str, to: &str) -> Result<usize> {
        let url = format!("{}/api/spaces/{}/move-to/{}", self.base_url, from, to);
        let resp = self
            .send(self.http.post(&url), &format!("POST {} failed", url))
            .await?;
        let resp = ensure_ok(resp, &url).await?;
        let json: serde_json::Value = resp.json().await.context("parsing move-to response")?;
        Ok(json["affected"].as_u64().unwrap_or(0) as usize)
    }

    /// GET /api/spaces — fetch a single space by name (filters from list).
    pub async fn get_space(&self, name: &str) -> Result<wenlan_types::Space> {
        let spaces = self.list_spaces().await?;
        spaces
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| anyhow::anyhow!("space '{}' not found", name))
    }

    /// GET /api/spaces — list all spaces.
    pub async fn list_spaces(&self) -> Result<Vec<wenlan_types::Space>> {
        self.get_json("/api/spaces", false).await
    }

    /// GET /api/spaces/default — fetch the daemon-owned default save Space.
    pub async fn get_default_space(&self) -> Result<DefaultSpaceResponse> {
        self.get_json("/api/spaces/default", false).await
    }

    /// PUT /api/spaces/default — replace the daemon-owned default save Space.
    pub async fn set_default_space(&self, space_id: String) -> Result<DefaultSpaceResponse> {
        self.put_json("/api/spaces/default", &SetDefaultSpaceRequest { space_id })
            .await
    }

    /// DELETE /api/spaces/default — clear the daemon-owned default save Space.
    pub async fn clear_default_space(&self) -> Result<()> {
        self.delete_ok("/api/spaces/default").await
    }

    /// PUT /api/pages/{id}/review-required — set or clear a page's review
    /// lock. While set, every machine write to the page is staged as a
    /// revision card; your own edits are unaffected.
    pub async fn set_page_review_required(
        &self,
        page_id: &str,
        required: bool,
    ) -> Result<wenlan_types::responses::PageReviewRequiredResponse> {
        self.put_json(
            &format!("/api/pages/{page_id}/review-required"),
            &wenlan_types::requests::SetPageReviewRequiredRequest { required },
        )
        .await
    }

    /// POST /api/agents — register a tool the user just connected.
    pub async fn register_agent(&self, name: &str) -> Result<AgentResponse> {
        self.post_json(
            "/api/agents",
            &wenlan_types::requests::RegisterAgentRequest {
                name: name.to_string(),
            },
        )
        .await
    }

    /// PUT /api/agents/{name} — update an agent's metadata.
    pub async fn update_agent(&self, name: &str, req: UpdateAgentRequest) -> Result<AgentResponse> {
        self.put_json(&format!("/api/agents/{}", name), &req).await
    }

    /// GET /api/memory/pending-revisions — staged revisions awaiting human accept/dismiss.
    pub async fn list_pending_revisions(&self, limit: usize) -> Result<Vec<PendingRevisionItem>> {
        self.get_json(
            &format!("/api/memory/pending-revisions?limit={}", limit),
            true,
        )
        .await
    }

    /// POST /api/memory/revision/{id}/dismiss — unstage the false revision: keep BOTH it and the original.
    /// `id` is the revision's own `source_id` (the daemon also accepts a target id, legacy).
    pub async fn dismiss_revision(&self, id: &str) -> Result<RevisionDismissResponse> {
        self.post_empty_json(&format!("/api/memory/revision/{}/dismiss", id))
            .await
    }

    /// GET /api/memory/{id}/detail — the assembled (chunks-joined) memory by source_id.
    /// Used by `wenlan curate` to fetch the ORIGINAL a revision would replace, so the
    /// card can show an original->revision diff.
    pub async fn get_memory_detail(&self, source_id: &str) -> Result<MemoryDetailResponse> {
        self.get_json(&format!("/api/memory/{}/detail", source_id), true)
            .await
    }

    /// GET /api/memory/entities/{id} — full entity detail by id. `Ok(None)` on 404
    /// (used by the CLI's id-or-name resolver to fall back to a name search).
    pub async fn get_entity(&self, id: &str) -> Result<Option<EntityDetail>> {
        let url = format!("{}/api/memory/entities/{}", self.base_url, id);
        let resp = self
            .send(
                self.http.get(&url),
                &format!("GET {} failed (is the daemon running?)", url),
            )
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .map(Some)
            .context("parsing /api/memory/entities/{id} response")
    }

    /// POST /api/memory/entities/list — every live entity in scope, unfiltered
    /// by name. Used for exact-name lookup: the daemon has no by-name filter
    /// on this route, so callers filter the returned list client-side.
    pub async fn list_entities(&self) -> Result<ListEntitiesResponse> {
        let url = format!("{}/api/memory/entities/list", self.base_url);
        let req = ListEntitiesRequest::default();
        let resp = self
            .send(
                self.http.post(&url).json(&req),
                &format!("POST {} failed", url),
            )
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json()
            .await
            .context("parsing /api/memory/entities/list response")
    }

    /// POST /api/memory/entities/query — filtered, paged entity list in all
    /// three lifecycle states.
    pub async fn query_entities(&self, req: &ListEntitiesRequest) -> Result<ListEntitiesResponse> {
        self.post_json("/api/memory/entities/query", req).await
    }

    /// POST /api/memory/entities/archive — archive the entities named by `ids`
    /// (or only count them when `dry_run`).
    pub async fn archive_entities(
        &self,
        ids: Vec<String>,
        dry_run: bool,
    ) -> Result<EntityBulkResponse> {
        let req = ArchiveEntitiesRequest {
            selection: EntitySelection {
                ids: Some(ids),
                filter: None,
            },
            dry_run,
        };
        self.post_json("/api/memory/entities/archive", &req).await
    }

    /// POST /api/memory/entities/restore — restore archived entities named by `ids`
    /// (or only count them when `dry_run`).
    pub async fn restore_entities(
        &self,
        ids: Vec<String>,
        dry_run: bool,
    ) -> Result<EntityBulkResponse> {
        let req = RestoreEntitiesRequest {
            selection: EntitySelection {
                ids: Some(ids),
                filter: None,
            },
            dry_run,
        };
        self.post_json("/api/memory/entities/restore", &req).await
    }

    /// GET /api/knowledge/path — the folder the daemon keeps its page files in.
    pub async fn knowledge_path(&self) -> Result<KnowledgePathResponse> {
        self.get_json("/api/knowledge/path", true).await
    }

    /// GET /api/pages/{id} — one page with its live-file metadata.
    pub async fn read_page(&self, id: &str) -> Result<PageReadResponse> {
        self.announce_session().await;
        self.get_json(&format!("/api/pages/{}", id), true).await
    }

    pub async fn team_library_terminal(
        &self,
        action: &wenlan_types::team_library::TeamTerminalAction,
    ) -> Result<wenlan_types::team_library::TeamTerminalReply> {
        anyhow::ensure!(
            self.is_local() && !self.names_an_agent() && self.session().is_none(),
            "The host's person must approve this from their own terminal."
        );
        Self::for_local_rules()?
            .post_json_as_person("/api/team/library/terminal", action, None, true)
            .await
    }

    pub async fn team_library_draft(
        &self,
        kind: wenlan_types::team_library::TeamItemKind,
        id: &str,
    ) -> Result<wenlan_types::team_library::TeamDraft> {
        anyhow::ensure!(
            self.is_local() && !self.names_an_agent() && self.session().is_none(),
            "The host's person must prepare this from their own terminal."
        );
        anyhow::ensure!(
            wenlan_core::shared_space::version::is_safe_id(id),
            "Use a Wenlan item ID."
        );
        let kind = match kind {
            wenlan_types::team_library::TeamItemKind::Page => "page",
            wenlan_types::team_library::TeamItemKind::Memory => "memory",
        };
        self.get_local_as_person(&format!("/api/team/library/draft?kind={kind}&id={id}"))
            .await
    }

    /// GET /api/skills/rules — read approved rules for the resolved Space.
    pub async fn read_space_rules(&self) -> Result<wenlan_types::SpaceRulesResponse> {
        self.get_json("/api/skills/rules", true).await
    }

    /// POST /api/skills/rules — save one rule the person asked for. Sent as
    /// this process is: under a tool's name it is that tool's save, with
    /// none it is the terminal's.
    pub async fn save_rule(
        &self,
        request: &wenlan_types::SaveRuleRequest,
    ) -> Result<wenlan_types::SaveRuleResponse> {
        self.post_json("/api/skills/rules", request).await
    }

    /// POST /api/skills/rules/{id}/remove — take a saved rule out.
    pub async fn remove_rule(
        &self,
        id: &str,
        request: &wenlan_types::RemoveRuleRequest,
    ) -> Result<wenlan_types::RemoveRuleResponse> {
        anyhow::ensure!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()),
            "that is not a rule id; `wenlan rules` prints each saved rule's id"
        );
        self.post_json(&format!("/api/skills/rules/{id}/remove"), request)
            .await
    }

    pub(crate) async fn preview_adoption_as_person(
        &self,
        request: &AdoptPreviewRequest,
    ) -> Result<AdoptionPlan> {
        self.post_rules_adoption("/api/rules/setup/adopt/preview", request)
            .await
    }

    pub(crate) async fn adopt_rules_as_person(
        &self,
        request: &AdoptRequest,
    ) -> Result<RulesSyncReport> {
        self.post_rules_adoption("/api/rules/setup/adopt", request)
            .await
    }

    pub(crate) async fn preview_share_as_person(
        &self,
        request: &wenlan_types::rulesync::SharePreviewRequest,
    ) -> Result<SharingPlan> {
        self.post_rules_adoption("/api/rules/setup/share/preview", request)
            .await
    }

    pub(crate) async fn share_rules_as_person(
        &self,
        request: &wenlan_types::rulesync::ShareRequest,
    ) -> Result<RulesSyncReport> {
        self.post_rules_adoption("/api/rules/setup/share", request)
            .await
    }

    pub(crate) async fn restore_rules_as_person(&self) -> Result<RulesSyncReport> {
        self.post_json_as_person(
            "/api/rules/setup/restore",
            &serde_json::json!({}),
            None,
            true,
        )
        .await
    }

    pub(crate) async fn sync_rules_as_person(&self) -> Result<RulesSyncReport> {
        self.rules_request("/api/rules/setup/sync", true).await
    }

    pub(crate) async fn rules_setup_status(
        &self,
    ) -> Result<wenlan_types::rulesync::RulesSetupStatus> {
        self.rules_request("/api/rules/setup/status", false).await
    }

    pub(crate) async fn shared_rule_edits(
        &self,
    ) -> Result<Vec<wenlan_types::rulesync::SharedEditWaiting>> {
        self.rules_request("/api/rules/setup/shared-edits", false)
            .await
    }

    pub(crate) async fn decide_shared_rule_edit(
        &self,
        request: &wenlan_types::rulesync::SharedEditDecisionRequest,
    ) -> Result<wenlan_types::rulesync::SharedEditDecision> {
        self.post_json_as_person("/api/rules/setup/shared-edits/decide", request, None, true)
            .await
    }

    pub(crate) async fn rules_setup_review(
        &self,
    ) -> Result<wenlan_types::rulesync::RulesReviewList> {
        self.rules_request("/api/rules/setup/review", false).await
    }

    pub(crate) async fn show_personal_rule(
        &self,
        request: &wenlan_types::personal_rules::PersonalRuleShowRequest,
    ) -> Result<wenlan_types::personal_rules::PersonalRuleShow> {
        self.post_json("/api/rules/personal/show", request).await
    }

    pub(crate) async fn propose_personal_rules(
        &self,
        request: &wenlan_types::personal_rules::PersonalRuleProposeRequest,
    ) -> Result<wenlan_types::personal_rules::PersonalRulePlan> {
        self.post_json("/api/rules/personal/propose", request).await
    }

    pub(crate) async fn personal_rule_plan(
        &self,
        id: &str,
    ) -> Result<wenlan_types::personal_rules::PersonalRulePlan> {
        validate_personal_rule_id(id)?;
        self.get_json(&format!("/api/rules/personal/plan/{id}"), false)
            .await
    }

    pub(crate) async fn preview_merge_page_rules(
        &self,
    ) -> Result<Option<wenlan_types::personal_rules::PersonalRulePlan>> {
        self.get_json("/api/rules/personal/merge-pages", false)
            .await
    }

    pub(crate) async fn decline_merge_page_rules(
        &self,
        request: &wenlan_types::personal_rules::PersonalRuleApplyRequest,
    ) -> Result<()> {
        self.post_json_as_person("/api/rules/personal/merge-pages", request, None, true)
            .await
    }

    pub(crate) async fn apply_personal_rule_plan(
        &self,
        request: &wenlan_types::personal_rules::PersonalRuleApplyRequest,
    ) -> Result<wenlan_types::personal_rules::PersonalRuleVersion> {
        self.post_json_as_person("/api/rules/personal/apply", request, None, true)
            .await
    }

    pub(crate) async fn personal_rule_history(
        &self,
    ) -> Result<Vec<wenlan_types::personal_rules::PersonalRuleVersionSummary>> {
        self.get_json("/api/rules/personal/history", false).await
    }

    pub(crate) async fn personal_rule_version(
        &self,
        id: &str,
    ) -> Result<wenlan_types::personal_rules::PersonalRuleVersion> {
        validate_personal_rule_id(id)?;
        self.get_json(&format!("/api/rules/personal/history/{id}"), false)
            .await
    }

    pub(crate) async fn propose_personal_rule_restore(
        &self,
        id: &str,
    ) -> Result<wenlan_types::personal_rules::PersonalRulePlan> {
        validate_personal_rule_id(id)?;
        self.post_json(
            &format!("/api/rules/personal/history/{id}/propose"),
            &serde_json::json!({}),
        )
        .await
    }

    async fn rules_request<R: DeserializeOwned>(&self, path: &str, post: bool) -> Result<R> {
        anyhow::ensure!(
            self.is_local(),
            "Use the Wenlan daemon on this computer for this rules command."
        );
        let key = self.person_key().await.context("Wenlan could not read the person key. Run this from your own account with the local daemon running.")?;
        let url = format!("{}{path}", self.base_url);
        let request = if post {
            self.http.post(&url).json(&serde_json::json!({}))
        } else {
            self.http.get(&url)
        }
        .header(
            wenlan_types::requests::CALLER_HEADER,
            wenlan_types::requests::PERSON_CALLER_VALUE,
        )
        .header(wenlan_types::requests::PERSON_KEY_HEADER, key);
        let response = self
            .send(request, &format!("rules request {url} failed"))
            .await?;
        ensure_ok(response, &url)
            .await?
            .json()
            .await
            .with_context(|| format!("parsing {path} response"))
    }

    pub(crate) async fn dismiss_rules_skill_update(&self, id: &str) -> Result<()> {
        self.post_json_as_person(
            "/api/rules/setup/review/dismiss-update",
            &wenlan_types::rulesync::DismissSkillUpdate { id: id.into() },
            None,
            true,
        )
        .await
    }

    pub(crate) async fn review_rules_as_person(
        &self,
        request: &PullbackDecision,
    ) -> Result<RulesSyncReport> {
        self.post_json_as_person("/api/rules/setup/review", request, None, true)
            .await
    }

    pub(crate) async fn preview_rules_review_as_person(
        &self,
        request: &PullbackDecision,
    ) -> Result<PullbackPreview> {
        self.post_json_as_person("/api/rules/setup/review/preview", request, None, true)
            .await
    }

    pub(crate) async fn preview_settings_as_person(
        &self,
        request: &wenlan_types::rulesync_settings::SettingsPreviewRequest,
    ) -> Result<wenlan_types::rulesync_settings::SettingsPlan> {
        self.post_json_as_person("/api/rules/setup/settings/preview", request, None, true)
            .await
    }

    pub(crate) async fn apply_settings_as_person(
        &self,
        request: &wenlan_types::rulesync_settings::SettingsApplyRequest,
    ) -> Result<RulesSyncReport> {
        self.post_json_as_person("/api/rules/setup/settings/apply", request, None, true)
            .await
    }

    /// GET /api/skills/registry/installed — the skills Wenlan holds.
    pub async fn list_skills(
        &self,
    ) -> Result<wenlan_types::skill_registry::InstalledSkillsResponse> {
        self.get_json("/api/skills/registry/installed", true).await
    }

    /// GET /api/skills/registry/search — skills.sh results with their checks.
    pub async fn search_skills(
        &self,
        query: &str,
    ) -> Result<wenlan_types::skill_registry::SkillSearchResponse> {
        self.get_json_with_query("/api/skills/registry/search", &[("q", query)])
            .await
    }

    /// GET /api/skills/registry/preview — every file of one skill and its
    /// safety checks, before anything is kept.
    pub async fn preview_skill(
        &self,
        source: &str,
        skill_id: &str,
    ) -> Result<wenlan_types::skill_registry::SkillPreview> {
        self.get_json_with_query(
            "/api/skills/registry/preview",
            &[("source", source), ("skillId", skill_id)],
        )
        .await
    }

    /// GET /api/skills/registry/local — skills in the AI tools' own folders
    /// that Wenlan does not hold yet.
    pub async fn scan_local_skills(
        &self,
    ) -> Result<wenlan_types::skill_registry::LocalSkillsResponse> {
        self.get_json("/api/skills/registry/local", true).await
    }

    /// POST /api/skills/registry/import, as the person.
    pub async fn import_local_skills_as_person(
        &self,
        names: Vec<String>,
    ) -> Result<wenlan_types::skill_registry::SkillImportResult> {
        let request = wenlan_types::skill_registry::SkillImportRequest { names };
        self.post_json_as_person("/api/skills/registry/import", &request, None, false)
            .await
    }

    /// POST /api/skills/registry/install, as the person.
    pub async fn install_skill_as_person(
        &self,
        request: &wenlan_types::skill_registry::SkillInstallRequest,
    ) -> Result<wenlan_types::skill_registry::SkillInstallResult> {
        self.post_json_as_person("/api/skills/registry/install", request, None, false)
            .await
    }

    /// POST /api/skills/registry/remove, as the person.
    pub async fn remove_skill_as_person(
        &self,
        name: &str,
    ) -> Result<wenlan_types::skill_registry::SkillRemoveResponse> {
        self.post_json_as_person(
            "/api/skills/registry/remove",
            &wenlan_types::skill_registry::SkillRemoveRequest {
                name: name.to_string(),
            },
            None,
            false,
        )
        .await
    }

    pub(crate) async fn save_personal_skill(
        &self,
        request: &wenlan_types::rulesync_skill::SaveSkillRequest,
    ) -> Result<wenlan_types::rulesync_skill::SaveSkillResult> {
        self.post_json_as_person("/api/skills/registry/save", request, None, true)
            .await
    }

    pub(crate) async fn pending_saved_skills(
        &self,
    ) -> Result<wenlan_types::rulesync_skill::SkillProposalsResponse> {
        self.get_local_as_person("/api/skills/registry/proposals")
            .await
    }

    async fn get_local_as_person<R: DeserializeOwned>(&self, path: &str) -> Result<R> {
        anyhow::ensure!(
            self.is_local() && !self.names_an_agent() && self.session().is_none(),
            "Only you can see the personal tool setup on this computer."
        );
        let key = self.person_key().await.context(
            "Wenlan could not read the person key. Run this from your own account with the local daemon running.",
        )?;
        let url = format!("{}{path}", self.base_url);
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building Wenlan HTTP client")?;
        let request = http
            .get(&url)
            .header(
                wenlan_types::requests::CALLER_HEADER,
                wenlan_types::requests::PERSON_CALLER_VALUE,
            )
            .header(wenlan_types::requests::PERSON_KEY_HEADER, key);
        let response = self.send(request, &format!("GET {url} failed")).await?;
        ensure_ok(response, &url)
            .await?
            .json()
            .await
            .with_context(|| format!("parsing {path} response"))
    }

    pub(crate) async fn review_saved_skill(
        &self,
        request: &wenlan_types::rulesync_skill::SkillReviewRequest,
    ) -> Result<wenlan_types::rulesync_skill::SaveSkillResult> {
        self.post_json_as_person("/api/skills/registry/review", request, None, true)
            .await
    }

    /// POST /api/person/browser-pass, as the person: a one-time pass that
    /// opens Wenlan in the browser with the person key (`wenlan open`).
    pub async fn browser_pass_as_person(
        &self,
    ) -> Result<wenlan_types::person_key::BrowserPassResponse> {
        // Short: a daemon still starting must not hold up opening the page.
        self.post_json_as_person(
            "/api/person/browser-pass",
            &serde_json::json!({}),
            Some(std::time::Duration::from_secs(3)),
            false,
        )
        .await
    }

    /// Whether this process told the daemon it is an AI tool.
    pub fn names_an_agent(&self) -> bool {
        self.headers.contains_key("x-agent-name")
    }

    /// GET `path` with query parameters, each value encoded.
    async fn get_json_with_query<R: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let what = format!("GET {url} failed (is the daemon running?)");
        let resp = self.send(self.http.get(&url).query(query), &what).await?;
        let resp = ensure_ok(resp, &url).await?;
        resp.json()
            .await
            .with_context(|| format!("parsing {path} response"))
    }

    /// POST `path` marked as the person's own request. The mark is only
    /// honest from a terminal the person is typing in, so callers ask them
    /// first; a process that named an AI tool cannot send it at all. The
    /// daemon believes the mark only next to the person key
    /// ([`Self::person_key`]). Legacy routes can send the mark alone for an
    /// older daemon that knows no key. Setup routes require a key.
    async fn post_json_as_person<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Option<std::time::Duration>,
        require_key: bool,
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        self.post_url_as_person(&url, body, timeout, require_key)
            .await
    }

    async fn post_rules_adoption<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R> {
        let key = self.person_key().await.context("Wenlan could not read the person key. Run this from your own account with the local daemon running.")?;
        let url = format!("{}{path}", self.base_url);
        let request = self
            .http
            .post(&url)
            .header(
                wenlan_types::requests::CALLER_HEADER,
                wenlan_types::requests::PERSON_CALLER_VALUE,
            )
            .header(wenlan_types::requests::PERSON_KEY_HEADER, key)
            .json(body);
        let response = self.send(request, &format!("POST {url} failed")).await?;
        ensure_ok(response, &url)
            .await?
            .json()
            .await
            .with_context(|| format!("parsing {url} response"))
    }

    async fn post_url_as_person<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
        timeout: Option<std::time::Duration>,
        require_key: bool,
    ) -> Result<R> {
        anyhow::ensure!(
            !self.names_an_agent(),
            "this is the person's to do, and this command is running as an AI tool"
        );
        let key = self.person_key().await;
        anyhow::ensure!(
            !require_key || key.is_some(),
            "Wenlan could not read the person key. Run this from your own account with the local daemon running."
        );
        // A client of its own: no default header may ride along, and the
        // key never goes through a proxy or a redirect.
        let mut http = reqwest::Client::builder();
        if key.is_some() {
            http = http.no_proxy().redirect(reqwest::redirect::Policy::none());
        }
        let http = http.build().context("building Wenlan HTTP client")?;
        let mut request = http
            .post(url)
            .header(
                wenlan_types::requests::CALLER_HEADER,
                wenlan_types::requests::PERSON_CALLER_VALUE,
            )
            .json(body);
        if let Some(key) = &key {
            request = request.header(wenlan_types::requests::PERSON_KEY_HEADER, key);
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let resp = self.send(request, &format!("POST {url} failed")).await?;
        let refused_without_key = key.is_none() && resp.status().is_client_error();
        if url.ends_with("/api/rules/setup/shared-edits/decide")
            && resp.status() == reqwest::StatusCode::CONFLICT
        {
            let body = resp.text().await.unwrap_or_default();
            if shared_edit_stale_revision(&body) {
                return Err(SharedEditStaleRevision.into());
            }
            if refused_without_key {
                anyhow::bail!(NO_PERSON_KEY);
            }
            anyhow::bail!("{}", daemon_error_message(&body));
        }
        let resp = match ensure_ok(resp, url).await {
            Ok(resp) => resp,
            Err(_) if refused_without_key => anyhow::bail!(NO_PERSON_KEY),
            Err(error) => return Err(error),
        };
        resp.json()
            .await
            .with_context(|| format!("parsing {url} response"))
    }

    /// The person key, when this command may send it: only to a daemon on
    /// this computer (`WENLAN_HOST` on loopback), never to another host. It
    /// is read from the data root the daemon itself reports, so the command
    /// and the app find the same key on an older `origin` data root too, and
    /// otherwise from the data root this command uses. Never printed.
    pub(crate) async fn person_key(&self) -> Option<String> {
        if !recovery::is_local_daemon_url(&self.base_url) {
            return None;
        }
        if let Some(root) = &self.person_key_root {
            return wenlan_types::person_key::read(root);
        }
        let reported = self
            .http
            .get(format!("{}/api/health", self.base_url))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .ok()
            .filter(|answer| answer.status().is_success());
        if let Some(answer) = reported {
            if let Ok(health) = answer.json::<HealthResponse>().await {
                if let Some(key) = health
                    .data_root
                    .and_then(|root| wenlan_types::person_key::read(std::path::Path::new(&root)))
                {
                    return Some(key);
                }
            }
        }
        wenlan_types::person_key::read(&wenlan_core::config::data_root())
    }

    /// GET /api/pages/{id}/sources — the memories a page was written from.
    pub async fn get_page_sources(&self, id: &str) -> Result<Vec<PageSourceWithMemory>> {
        self.get_json(&format!("/api/pages/{}/sources", id), true)
            .await
    }

    /// POST /api/memory/entities/{id}/merge — merge `loser_id` into `into`.
    pub async fn merge_entity(
        &self,
        loser_id: &str,
        into: String,
        dry_run: bool,
    ) -> Result<MergeEntityResponse> {
        let url = format!("{}/api/memory/entities/{}/merge", self.base_url, loser_id);
        let req = MergeEntityRequest { into, dry_run };
        let resp = self
            .send(
                self.http.post(&url).json(&req),
                &format!("POST {} failed", url),
            )
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json()
            .await
            .context("parsing /api/memory/entities/{id}/merge response")
    }

    /// POST /api/memory/entities/{id}/aliases — declare `alias` as an additional name.
    pub async fn add_entity_alias(&self, id: &str, alias: String) -> Result<EntityAliasesResponse> {
        let url = format!("{}/api/memory/entities/{}/aliases", self.base_url, id);
        let req = AddEntityAliasRequest { alias };
        let resp = self
            .send(
                self.http.post(&url).json(&req),
                &format!("POST {} failed", url),
            )
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json()
            .await
            .context("parsing /api/memory/entities/{id}/aliases response")
    }

    /// `/api/spaces/{space}/wiki/{tail}`, with the Space name encoded as one
    /// path segment.
    fn space_wiki_url(&self, space: &str, tail: &[&str]) -> Result<String> {
        let mut url = reqwest::Url::parse(&self.base_url).context("parsing the daemon URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("the daemon URL cannot take a path"))?
            .pop_if_empty()
            .extend(["api", "spaces", space, "wiki"])
            .extend(tail.iter().copied());
        Ok(url.into())
    }

    /// `POST /api/spaces/{space}/shared/sync`: one sync pass of `space`'s
    /// pages with its shared folder.
    pub async fn shared_sync(
        &self,
        space: &str,
        req: &wenlan_types::shared_space::SharedSyncRequest,
    ) -> Result<wenlan_types::shared_space::SharedSyncCounts> {
        let mut url = reqwest::Url::parse(&self.base_url).context("parsing the daemon URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("the daemon URL cannot take a path"))?
            .pop_if_empty()
            .extend(["api", "spaces", space, "shared", "sync"]);
        let url: String = url.into();
        let resp = self
            .send(
                self.http.post(&url).json(req),
                &format!("POST {url} failed"),
            )
            .await?;
        let resp = ensure_team_success(resp, &url).await?;
        resp.json().await.context("parsing shared sync response")
    }

    /// A daemon URL from percent-encoded path segments and an optional query.
    fn team_url(&self, segments: &[&str], query: &[(&str, &str)]) -> Result<String> {
        let mut url = reqwest::Url::parse(&self.base_url).context("parsing the daemon URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("the daemon URL cannot take a path"))?
            .pop_if_empty()
            .extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url.into())
    }

    async fn team_call<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        url: String,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut req = self.http.request(method.clone(), &url);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = self.send(req, &format!("{method} {url} failed")).await?;
        let resp = ensure_team_success(resp, &url).await?;
        resp.json().await.context("parsing the team response")
    }

    /// `POST /api/team/start`: make this computer a team's server.
    pub async fn team_start(
        &self,
        req: &wenlan_types::team::TeamStartRequest,
    ) -> Result<wenlan_types::team::TeamStatus> {
        let url = self.team_url(&["api", "team", "start"], &[])?;
        self.team_call(reqwest::Method::POST, url, Some(serde_json::to_value(req)?))
            .await
    }

    /// `POST /api/team/stop`: turn the team listener off.
    pub async fn team_stop(&self) -> Result<wenlan_types::team::TeamStatus> {
        let url = self.team_url(&["api", "team", "stop"], &[])?;
        self.team_call(reqwest::Method::POST, url, None).await
    }

    /// `GET /api/team/firewall`: on Windows, whether other computers get
    /// through to the team listener; `None` elsewhere.
    pub async fn team_firewall(&self) -> Result<Option<wenlan_types::team::TeamFirewall>> {
        let url = self.team_url(&["api", "team", "firewall"], &[])?;
        self.team_call(reqwest::Method::GET, url, None).await
    }

    /// `GET /api/team/membership`: the teams this computer has joined.
    pub async fn team_memberships(&self) -> Result<Vec<wenlan_types::team::TeamMembershipInfo>> {
        let url = self.team_url(&["api", "team", "membership"], &[])?;
        self.team_call(reqwest::Method::GET, url, None).await
    }

    /// `POST /api/team/membership`: keep that this computer is on the team
    /// an invite link is for.
    pub async fn team_remember_membership(&self, code: &str) -> Result<wenlan_types::team::TeamMe> {
        let url = self.team_url(&["api", "team", "membership"], &[])?;
        let body = serde_json::to_value(&wenlan_types::team::TeamMembershipRequest {
            code: code.to_string(),
        })?;
        self.team_call(reqwest::Method::POST, url, Some(body)).await
    }

    /// `DELETE /api/team/membership?url=`: this person left the team at `url`.
    pub async fn team_forget_membership(&self, server: &str) -> Result<()> {
        let url = self.team_url(&["api", "team", "membership"], &[("url", server)])?;
        let _: serde_json::Value = self.team_call(reqwest::Method::DELETE, url, None).await?;
        Ok(())
    }

    /// `GET /api/team/my-tools?url=`: this person's AI tools on the teams
    /// they joined (one team with `server`).
    pub async fn team_my_tools(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<wenlan_types::team::TeamToolsOnServer>> {
        let query: Vec<(&str, &str)> = server.map(|url| ("url", url)).into_iter().collect();
        let url = self.team_url(&["api", "team", "my-tools"], &query)?;
        self.team_call(reqwest::Method::GET, url, None).await
    }

    /// `DELETE /api/team/my-tools/{id}?url=`: remove one of them.
    pub async fn team_remove_my_tool(
        &self,
        id: &str,
        server: Option<&str>,
    ) -> Result<wenlan_types::team::TeamToolsOnServer> {
        let query: Vec<(&str, &str)> = server.map(|url| ("url", url)).into_iter().collect();
        let url = self.team_url(&["api", "team", "my-tools", id], &query)?;
        self.team_call(reqwest::Method::DELETE, url, None).await
    }

    /// `GET /api/shared`: everything this Wenlan shares or syncs.
    pub async fn shared_list(&self) -> Result<Vec<wenlan_types::shared_space::SharedSyncStatus>> {
        let url = self.team_url(&["api", "shared"], &[])?;
        let list: wenlan_types::shared_space::SharedListResponse =
            self.team_call(reqwest::Method::GET, url, None).await?;
        Ok(list.shares)
    }

    /// `DELETE /api/spaces/{space}/shared`: stop syncing `space` from here.
    /// Its pages stay.
    pub async fn shared_leave(&self, space: &str) -> Result<()> {
        let url = self.team_url(&["api", "spaces", space, "shared"], &[])?;
        let _: serde_json::Value = self.team_call(reqwest::Method::DELETE, url, None).await?;
        Ok(())
    }

    /// `GET /api/team`: the member list and hosted Spaces.
    pub async fn team_status(&self) -> Result<wenlan_types::team::TeamStatus> {
        let url = self.team_url(&["api", "team"], &[])?;
        self.team_call(reqwest::Method::GET, url, None).await
    }

    /// `PUT /api/team`: name the team.
    pub async fn team_rename(&self, name: &str) -> Result<wenlan_types::team::TeamInfo> {
        let url = self.team_url(&["api", "team"], &[])?;
        let body = serde_json::to_value(&wenlan_types::team::TeamRenameRequest {
            name: name.to_string(),
        })?;
        self.team_call(reqwest::Method::PUT, url, Some(body)).await
    }

    /// `POST /api/team/members`: add a member; the key is in the answer once.
    pub async fn team_add_member(
        &self,
        req: &wenlan_types::team::TeamAddMemberRequest,
    ) -> Result<wenlan_types::team::TeamMemberKey> {
        let url = self.team_url(&["api", "team", "members"], &[])?;
        self.team_call(reqwest::Method::POST, url, Some(serde_json::to_value(req)?))
            .await
    }

    /// `PUT /api/team/members/{id}`: rename a member or replace their Spaces.
    pub async fn team_update_member(
        &self,
        id: &str,
        req: &wenlan_types::team::TeamUpdateMemberRequest,
    ) -> Result<wenlan_types::team::TeamMember> {
        let url = self.team_url(&["api", "team", "members", id], &[])?;
        self.team_call(reqwest::Method::PUT, url, Some(serde_json::to_value(req)?))
            .await
    }

    /// `DELETE /api/team/members/{id}`: remove a member; their key stops working.
    pub async fn team_remove_member(&self, id: &str) -> Result<wenlan_types::team::TeamMember> {
        let url = self.team_url(&["api", "team", "members", id], &[])?;
        self.team_call(reqwest::Method::DELETE, url, None).await
    }

    /// `POST /api/team/members/{id}/key`: a new key for a member.
    pub async fn team_new_member_key(
        &self,
        id: &str,
        server_url: Option<&str>,
    ) -> Result<wenlan_types::team::TeamMemberKey> {
        let query: Vec<(&str, &str)> = server_url.map(|u| ("url", u)).into_iter().collect();
        let url = self.team_url(&["api", "team", "members", id, "key"], &query)?;
        self.team_call(reqwest::Method::POST, url, None).await
    }

    /// `POST /api/team/spaces`: host a Space for the team.
    pub async fn team_host_space(
        &self,
        space: &str,
        everyone: Option<wenlan_types::team::TeamEveryone>,
    ) -> Result<wenlan_types::shared_space::SharedSyncCounts> {
        let url = self.team_url(&["api", "team", "spaces"], &[])?;
        let body = serde_json::to_value(&wenlan_types::team::TeamHostSpaceRequest {
            space: space.to_string(),
            everyone,
            rename_to: None,
        })?;
        self.team_call(reqwest::Method::POST, url, Some(body)).await
    }

    /// `POST /api/team/join`: read a join code and list the Spaces it opens.
    pub async fn team_join(&self, code: &str) -> Result<wenlan_types::team::TeamJoinResponse> {
        let url = self.team_url(&["api", "team", "join"], &[])?;
        let body = serde_json::to_value(&wenlan_types::team::TeamJoinRequest {
            code: code.to_string(),
            key: None,
        })?;
        self.team_call(reqwest::Method::POST, url, Some(body)).await
    }

    /// `GET /api/spaces/{space}/wiki/repo`: the repository `space` is bound
    /// to, if any.
    pub async fn repo_wiki_binding(
        &self,
        space: &str,
    ) -> Result<Option<wenlan_types::repo_wiki::RepoWikiBinding>> {
        let url = self.space_wiki_url(space, &["repo"])?;
        let resp = self
            .send(self.http.get(&url), &format!("GET {url} failed"))
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json().await.context("parsing wiki repo response")
    }

    /// `PUT /api/spaces/{space}/wiki/repo`: bind `space` to a repository, or
    /// change its folder and daily run.
    pub async fn bind_repo_wiki(
        &self,
        space: &str,
        req: &wenlan_types::repo_wiki::BindRepoWikiRequest,
    ) -> Result<wenlan_types::repo_wiki::RepoWikiBinding> {
        let url = self.space_wiki_url(space, &["repo"])?;
        let resp = self
            .send(self.http.put(&url).json(req), &format!("PUT {url} failed"))
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json().await.context("parsing wiki repo response")
    }

    /// `DELETE /api/spaces/{space}/wiki/repo`. True when a binding was removed.
    pub async fn unbind_repo_wiki(&self, space: &str) -> Result<bool> {
        let url = self.space_wiki_url(space, &["repo"])?;
        let resp = self
            .send(self.http.delete(&url), &format!("DELETE {url} failed"))
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        let body: serde_json::Value = resp.json().await.context("parsing unbind response")?;
        Ok(body
            .get("removed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// `POST /api/spaces/{space}/wiki/docs`: record a doc fix, or change
    /// which docs follow a page.
    pub async fn repo_wiki_docs(
        &self,
        space: &str,
        req: &wenlan_types::repo_wiki::RepoDocsRequest,
    ) -> Result<wenlan_types::repo_wiki::RepoDocsReply> {
        let url = self.space_wiki_url(space, &["docs"])?;
        let resp = self
            .send(
                self.http.post(&url).json(req),
                &format!("POST {url} failed"),
            )
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json().await.context("parsing wiki docs response")
    }

    /// `POST /api/spaces/{space}/wiki/publish`: write the Space's code pages
    /// into its repository and open or update the pull request.
    pub async fn publish_repo_wiki(
        &self,
        space: &str,
    ) -> Result<wenlan_types::repo_wiki::RepoWikiPublishResult> {
        let url = self.space_wiki_url(space, &["publish"])?;
        let resp = self
            .send(self.http.post(&url), &format!("POST {url} failed"))
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json().await.context("parsing wiki publish response")
    }

    pub(crate) async fn project_rule_proposals(
        &self,
        space: &str,
    ) -> Result<Vec<ProjectRuleProposal>> {
        let url = self.space_wiki_url(space, &["rules", "proposals"])?;
        self.post_url_as_person(&url, &serde_json::json!({}), None, true)
            .await
    }

    /// `GET /api/team/joined/library`: best-effort live page counts after a
    /// join. The app remains the authoritative read-through surface if this
    /// optional follow-up cannot reach the team server.
    pub async fn team_joined_library(
        &self,
    ) -> Result<wenlan_types::team_library::JoinedTeamLibraries> {
        self.get_local_as_person("/api/team/joined/library").await
    }

    pub(crate) async fn decide_project_rule_proposal(
        &self,
        space: &str,
        id: &str,
        accept: bool,
    ) -> Result<ProjectRuleProposal> {
        let action = if accept { "accept" } else { "dismiss" };
        let url = self.space_wiki_url(space, &["rules", "proposals", id, action])?;
        self.post_url_as_person(&url, &serde_json::json!({}), None, true)
            .await
    }

    pub(crate) async fn checkout_rules(
        &self,
        repo: std::path::PathBuf,
    ) -> Result<wenlan_types::rulesync::CheckoutRulesReceipt> {
        self.post_json_as_person(
            "/api/rules/setup/checkout",
            &wenlan_types::rulesync::CheckoutRulesRequest { repo },
            None,
            true,
        )
        .await
    }

    pub(crate) async fn use_team_rules_in_repo(
        &self,
        space: &str,
        team: Option<String>,
    ) -> Result<wenlan_types::rulesync::ProjectTeamRulesRequest> {
        let url = self.space_wiki_url(space, &["rules", "team"])?;
        self.post_url_as_person(
            &url,
            &wenlan_types::rulesync::ProjectTeamRulesRequest { team },
            None,
            true,
        )
        .await
    }

    pub(crate) async fn shared_rule_proposals(
        &self,
        space: &str,
    ) -> Result<Vec<wenlan_types::team::TeamRulesProposal>> {
        let url = self.space_wiki_url(space, &["shared-rules", "proposals"])?;
        self.post_url_as_person(&url, &(), None, true).await
    }

    pub(crate) async fn propose_shared_rules(
        &self,
        space: &str,
        markdown: String,
    ) -> Result<String> {
        let url = self.space_wiki_url(space, &["shared-rules", "propose"])?;
        self.post_url_as_person(
            &url,
            &wenlan_types::rulesync::SharedRulesProposalRequest { markdown },
            None,
            true,
        )
        .await
    }

    pub(crate) async fn decide_shared_rules(
        &self,
        space: &str,
        id: &str,
        accept: bool,
    ) -> Result<()> {
        let url = self.space_wiki_url(space, &["shared-rules", "proposals", id, "decision"])?;
        self.post_url_as_person(
            &url,
            &wenlan_types::team::TeamRulesDecisionRequest { accept },
            None,
            true,
        )
        .await
    }

    /// `/api/team/rules/{tail}`: the team host's own Rules review.
    fn team_rules_url(&self, tail: &[&str]) -> Result<String> {
        let mut url = reqwest::Url::parse(&self.base_url).context("parsing the daemon URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("the daemon URL cannot take a path"))?
            .pop_if_empty()
            .extend(["api", "team", "rules"])
            .extend(tail.iter().copied());
        Ok(url.into())
    }

    pub(crate) async fn team_rules_proposals(
        &self,
    ) -> Result<Vec<wenlan_types::team::TeamRulesHostProposal>> {
        let url = self.team_rules_url(&["proposals"])?;
        self.post_url_as_person(&url, &(), None, true).await
    }

    pub(crate) async fn team_rules_propose(&self, space: &str, markdown: String) -> Result<String> {
        let url = self.team_rules_url(&["propose"])?;
        self.post_url_as_person(
            &url,
            &wenlan_types::team::TeamRulesHostProposeRequest {
                space: space.to_string(),
                markdown,
            },
            None,
            true,
        )
        .await
    }

    pub(crate) async fn team_rules_decide(
        &self,
        id: &str,
        accept: bool,
    ) -> Result<wenlan_types::team::TeamRulesHostDecision> {
        let url = self.team_rules_url(&["proposals", id, "decision"])?;
        self.post_url_as_person(
            &url,
            &wenlan_types::team::TeamRulesDecisionRequest { accept },
            None,
            true,
        )
        .await
    }

    /// POST /api/pages/export with `format: "okf"` — write a pure OKF v0.2
    /// bundle of every exportable page into `dir`. Used by
    /// `wenlan export okf`. The daemon owns the safety checks (absolute
    /// path, empty-or-previous-export target); its error text reaches the
    /// caller through `ensure_daemon_success`.
    ///
    /// The daemon version is checked first: a daemon older than
    /// [`OKF_EXPORT_DAEMON_FLOOR`] ignores `format` and writes an Obsidian
    /// export into `dir` while reporting success, so nothing is sent to it.
    pub async fn export_pages_okf(&self, dir: String) -> Result<ExportStats> {
        let version = self.health().await?.version;
        if !daemon_supports_okf_export(&version) {
            anyhow::bail!(
                "OKF export needs Wenlan daemon {OKF_EXPORT_DAEMON_FLOOR} or newer, but the \
                 running daemon is {version}. Run `wenlan restart` so it runs the updated \
                 version (or update the install it starts from), then try again. \
                 Nothing was exported."
            );
        }
        let url = format!("{}/api/pages/export", self.base_url);
        let req = ExportPagesRequest {
            vault_path: Some(dir),
            format: Some(ExportFormat::Okf),
        };
        let resp = self
            .send(
                self.http.post(&url).json(&req),
                &format!("POST {} failed", url),
            )
            .await?;
        let resp = ensure_daemon_success(resp, &url).await?;
        resp.json()
            .await
            .context("parsing /api/pages/export response")
    }
}

fn validate_personal_rule_id(id: &str) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
        "that is not a personal rule version or plan id"
    );
    Ok(())
}

/// A response the daemon answered with a non-success status.
///
/// The sentence a person-only command shows when Wenlan refused it and this
/// account could not read the person key, so Wenlan cannot tell the command
/// is the person's.
pub(crate) const NO_PERSON_KEY: &str = "Wenlan cannot tell this is you: this command could not \
     read Wenlan's key on this computer. Run it from your own account against the Wenlan on this \
     computer (start it once with `wenlan start`), or do it in the Wenlan app.";

/// A 4xx is the daemon telling the person what to change ("too short to
/// save", "page not found"), so when it carries a message that message is
/// the whole error: the status line and the daemon's own address say
/// nothing they can act on. A 4xx with no message, and every other
/// failure, keeps the status and the address.
#[derive(Debug)]
pub struct DaemonRefusal {
    pub status: reqwest::StatusCode,
    url: String,
    message: String,
}

impl std::fmt::Display for DaemonRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status.is_client_error() && !self.message.is_empty() {
            return write!(f, "{}", self.message);
        }
        write!(f, "Wenlan returned {} for {}", self.status, self.url)?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for DaemonRefusal {}

/// The status of a refused request anywhere in `error`'s chain, whether it
/// came through [`ensure_ok`] or from `reqwest` itself.
pub fn refusal_status(error: &anyhow::Error) -> Option<reqwest::StatusCode> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<DaemonRefusal>()
            .map(|refusal| refusal.status)
            .or_else(|| {
                cause
                    .downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
            })
    })
}

/// Non-success response -> a [`DaemonRefusal`] carrying the daemon's own
/// words. Every generic request helper goes through this.
async fn ensure_ok(resp: reqwest::Response, url: &str) -> Result<reqwest::Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    Err(DaemonRefusal {
        status,
        url: url.to_owned(),
        message: daemon_refusal_message(&body),
    }
    .into())
}

/// What to show for a refusal body. `{"error": "..."}` is the shape every
/// `ServerError` uses; a rejected save answers `{"reason", "detail"}`; a
/// body that is neither (a framework's own text) is shown as it is, cut to
/// a line a terminal can hold.
fn daemon_refusal_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct Rejection {
        reason: Option<String>,
        detail: Option<String>,
    }
    const MAX: usize = 300;
    let message = match serde_json::from_str::<Rejection>(body) {
        Ok(Rejection {
            reason: Some(reason),
            detail,
        }) => match reason.as_str() {
            "too_short" => {
                "Not saved: too short to be useful later. Write a full sentence.".to_string()
            }
            "credential_leak" => {
                "Not saved: this looks like it contains a password or key.".to_string()
            }
            _ => match detail {
                Some(detail) if !detail.is_empty() => format!("Not saved: {detail}"),
                _ => format!("Not saved ({reason})."),
            },
        },
        _ => daemon_error_message(body),
    };
    if message.chars().count() > MAX {
        let cut: String = message.chars().take(MAX).collect();
        return format!("{cut}…");
    }
    message
}

/// Non-success response -> `anyhow::Error` carrying the daemon's own error
/// message. Used by `list_entities`, `merge_entity`, `add_entity_alias` and
/// `export_pages_okf`, which keep the status and address in every case.
#[derive(Debug)]
pub(crate) struct SharedEditStaleRevision;

impl std::fmt::Display for SharedEditStaleRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(wenlan_types::rulesync::SHARED_EDIT_STALE_REVISION_MESSAGE)
    }
}

impl std::error::Error for SharedEditStaleRevision {}

pub(crate) fn shared_edit_stale_revision(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|body| {
            body.get("code")
                .and_then(|code| code.as_str())
                .map(str::to_owned)
        })
        .is_some_and(|code| code == wenlan_types::rulesync::SHARED_EDIT_STALE_REVISION_CODE)
}

async fn ensure_daemon_success(resp: reqwest::Response, url: &str) -> Result<reqwest::Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let msg = daemon_error_message(&body);
    anyhow::bail!("daemon returned {status} for {url}: {msg}");
}

/// `ensure_daemon_success` for the team and sharing commands. A 4xx there is
/// the daemon telling the person what to change ("this is not an invite
/// link", "no member has that id"), so its message is the whole error: the
/// status line and the daemon's own address say nothing they can act on. A
/// 4xx with no message, and every other failure, keeps the full detail.
async fn ensure_team_success(resp: reqwest::Response, url: &str) -> Result<reqwest::Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let msg = daemon_error_message(&body);
    if status.is_client_error() && !msg.is_empty() {
        anyhow::bail!("{msg}");
    }
    anyhow::bail!("daemon returned {status} for {url}: {msg}");
}

/// Pull the daemon's message out of an error response body: the `error`
/// string when the body is a `{"error": "..."}` JSON object (the shape
/// every `ServerError` response uses), else the raw body trimmed.
fn daemon_error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrorEnvelope {
        error: String,
    }
    serde_json::from_str::<ErrorEnvelope>(body)
        .map(|envelope| envelope.error)
        .unwrap_or_else(|_| body.trim().to_string())
}

/// First release whose daemon honors `format: "okf"` on
/// `POST /api/pages/export` (#760). The app keeps its own copy in
/// `app/src/search.rs`.
const OKF_EXPORT_DAEMON_FLOOR: &str = "0.18.9";

/// Whether a daemon reporting `version` from `/api/health` writes an OKF
/// bundle. Only a stable `major.minor.patch` counts; a pre-release or an
/// unreadable version is refused, the same rule as the app's daemon floors.
fn daemon_supports_okf_export(version: &str) -> bool {
    fn parse(version: &str) -> Option<(u64, u64, u64)> {
        let core = version.split('+').next()?;
        if core.contains('-') {
            return None;
        }
        let mut parts = core.split('.');
        let parsed = (
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        parts.next().is_none().then_some(parsed)
    }
    let floor = parse(OKF_EXPORT_DAEMON_FLOOR).expect("the OKF export floor is a valid release");
    parse(version).is_some_and(|candidate| candidate >= floor)
}

fn build_list_request(
    limit: Option<usize>,
    memory_type: Option<String>,
    confirmed: Option<bool>,
) -> ListMemoriesRequest {
    ListMemoriesRequest {
        memory_type,
        space: None,
        limit: limit.unwrap_or(100),
        confirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_list_request, daemon_error_message, daemon_refusal_message,
        daemon_supports_okf_export, refusal_status, DaemonRefusal, WenlanClient,
    };

    #[test]
    fn okf_export_floor_is_the_first_release_with_the_format() {
        assert!(daemon_supports_okf_export("0.18.9"));
        assert!(daemon_supports_okf_export("0.18.10"));
        assert!(daemon_supports_okf_export("0.19.0+abc123"));
        assert!(daemon_supports_okf_export("1.0.0"));
        assert!(!daemon_supports_okf_export("0.18.8"));
        assert!(!daemon_supports_okf_export("0.18.9-rc.1"));
        assert!(!daemon_supports_okf_export("0.18"));
        assert!(!daemon_supports_okf_export("0.18.9.1"));
        assert!(!daemon_supports_okf_export(""));
    }

    /// A fake daemon on an ephemeral port that answers `/api/health` with
    /// `version` and the export route with fixed stats, one request per
    /// connection, recording each request line until the test stops it.
    fn fake_daemon(
        version: &'static str,
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<Vec<String>>,
    ) {
        use std::io::{BufRead as _, BufReader, Read as _, Write as _};
        use std::sync::atomic::Ordering;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let base = format!("http://{}", listener.local_addr().expect("local addr"));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop_flag.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept failed: {e}"),
                };
                stream.set_nonblocking(false).expect("blocking stream");
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).expect("request line");
                let mut content_length = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).expect("header line");
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).expect("request body");
                let request_line = request_line.trim_end().to_string();
                let json = if request_line.starts_with("GET /api/health ") {
                    format!(r#"{{"status":"ok","db_initialized":true,"version":"{version}"}}"#)
                } else {
                    r#"{"exported":2,"skipped":0,"failed":0}"#.to_string()
                };
                seen.push(request_line);
                let mut stream = reader.into_inner();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                )
                .expect("write response");
            }
            seen
        });
        (base, stop, handle)
    }

    #[tokio::test]
    async fn okf_export_sends_nothing_to_a_daemon_older_than_the_floor() {
        let (base, stop, handle) = fake_daemon("0.18.8");
        let error = client_for(&base)
            .with_recovery(false)
            .export_pages_okf("/tmp/okf-bundle".to_string())
            .await
            .expect_err("a 0.18.8 daemon would write an Obsidian export");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let seen = handle.join().expect("fake daemon thread");

        let text = format!("{error:#}");
        assert!(
            text.contains("needs Wenlan daemon 0.18.9 or newer"),
            "{text}"
        );
        assert!(text.contains("running daemon is 0.18.8"), "{text}");
        assert!(text.contains("wenlan restart"), "{text}");
        assert_eq!(seen, vec!["GET /api/health HTTP/1.1".to_string()]);
    }

    #[tokio::test]
    async fn okf_export_posts_to_a_daemon_at_the_floor() {
        let (base, stop, handle) = fake_daemon("0.18.9");
        let stats = client_for(&base)
            .with_recovery(false)
            .export_pages_okf("/tmp/okf-bundle".to_string())
            .await
            .expect("a 0.18.9 daemon writes the bundle");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let seen = handle.join().expect("fake daemon thread");

        assert_eq!(stats.exported, 2);
        assert_eq!(
            seen,
            vec![
                "GET /api/health HTTP/1.1".to_string(),
                "POST /api/pages/export HTTP/1.1".to_string(),
            ]
        );
    }

    #[test]
    fn a_refused_save_says_why_in_plain_words() {
        let body = r#"{"detail":"Only 1 meaningful words (below minimum)","reason":"too_short","status":"rejected"}"#;
        assert_eq!(
            daemon_refusal_message(body),
            "Not saved: too short to be useful later. Write a full sentence."
        );
        let other = r#"{"detail":"matched system_prompt_markers","reason":"noise_pattern"}"#;
        assert_eq!(
            daemon_refusal_message(other),
            "Not saved: matched system_prompt_markers"
        );
    }

    #[test]
    fn a_client_error_with_a_message_is_only_that_message() {
        let refusal = DaemonRefusal {
            status: reqwest::StatusCode::NOT_FOUND,
            url: "http://127.0.0.1:7878/api/pages/x".into(),
            message: daemon_refusal_message(r#"{"error":"page not found"}"#),
        };
        assert_eq!(refusal.to_string(), "page not found");
    }

    #[test]
    fn a_server_error_or_an_empty_refusal_keeps_the_status_and_address() {
        let server = DaemonRefusal {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            url: "http://127.0.0.1:7878/api/x".into(),
            message: "boom".into(),
        };
        assert_eq!(
            server.to_string(),
            "Wenlan returned 500 Internal Server Error for http://127.0.0.1:7878/api/x: boom"
        );
        let empty = DaemonRefusal {
            status: reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            url: "http://127.0.0.1:7878/api/x".into(),
            message: String::new(),
        };
        assert_eq!(
            empty.to_string(),
            "Wenlan returned 422 Unprocessable Entity for http://127.0.0.1:7878/api/x"
        );
    }

    #[test]
    fn a_long_refusal_body_is_cut_to_one_line_worth() {
        let message = daemon_refusal_message(&"x".repeat(1000));
        assert_eq!(message.chars().count(), 301);
        assert!(message.ends_with('…'));
    }

    #[test]
    fn refusal_status_is_found_through_added_context() {
        let error = anyhow::Error::from(DaemonRefusal {
            status: reqwest::StatusCode::CONFLICT,
            url: String::new(),
            message: "already there".into(),
        })
        .context("adding the source");
        assert_eq!(refusal_status(&error), Some(reqwest::StatusCode::CONFLICT));
    }

    #[test]
    fn daemon_error_message_reads_error_envelope() {
        let body =
            r#"{"error":"alias \"origin-core\" is the name of live entity e1; use merge instead"}"#;
        assert_eq!(
            daemon_error_message(body),
            "alias \"origin-core\" is the name of live entity e1; use merge instead"
        );
    }

    #[test]
    fn daemon_error_message_falls_back_to_raw_text() {
        assert_eq!(
            daemon_error_message("  Internal Server Error  "),
            "Internal Server Error"
        );
    }

    #[test]
    fn list_request_can_filter_unconfirmed_memories() {
        let request = build_list_request(Some(20), None, Some(false));
        let json = serde_json::to_value(request).expect("serialize list request");
        assert_eq!(json["confirmed"], false);
    }

    #[test]
    fn list_request_omits_confirmation_filter_by_default() {
        let request = build_list_request(Some(20), None, None);
        let json = serde_json::to_value(request).expect("serialize list request");
        assert!(json.get("confirmed").is_none());
    }

    #[tokio::test]
    async fn connect_failure_says_what_to_do_next() {
        // An ephemeral port that was just released: nothing listens there.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        let base = format!("http://{addr}");
        let client = client_for(&base).with_recovery(false);
        let error = tokio::time::timeout(std::time::Duration::from_secs(10), client.health())
            .await
            .expect("connect failure within 10s")
            .expect_err("nothing listens on the released port");
        let text = format!("{error:#}");
        assert!(
            text.contains(&format!("Wenlan is not running at {base}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("GET {base}/api/health failed")),
            "{text}"
        );
    }

    fn client_for(base_url: &str) -> WenlanClient {
        WenlanClient {
            base_url: base_url.to_string(),
            http: reqwest::Client::new(),
            headers: reqwest::header::HeaderMap::new(),
            recovery_enabled: true,
            session: None,
            session_announced: tokio::sync::OnceCell::new(),
            person_key_root: None,
        }
    }

    // F1 regression: a client pointed at a remote host must never be treated
    // as merely offline. The integration-level version of this ("capture
    // against an unreachable *remote* WENLAN_HOST must never queue") is not
    // exercised end-to-end here because the sandboxed test environment
    // denies egress to arbitrary hosts (including a nonexistent
    // `*.invalid` one), so a live connection attempt cannot be made to fail
    // fast and deterministically the way `reqwest::Error::is_connect()`
    // would need. `is_local()` is exactly the gate `store`/`brief`/`main`
    // check before queueing, so pinning its behavior on both loopback and
    // remote URLs covers the same guarantee without depending on sandbox or
    // real-world DNS timing.
    #[test]
    fn is_local_accepts_only_loopback_hosts() {
        assert!(client_for("http://127.0.0.1:7878").is_local());
        assert!(client_for("http://localhost:7878").is_local());
        assert!(client_for("http://[::1]:7878").is_local());
        assert!(!client_for("http://wenlan-outbox-nonlocal.invalid:7878").is_local());
        assert!(!client_for("http://example.com:7878").is_local());
    }
}
